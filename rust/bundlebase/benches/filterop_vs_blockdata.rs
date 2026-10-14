//! FilterOps (A) vs block-level tombstones/overlays (B) — decision-support
//! benchmarks for the UPDATE/DELETE representation question.
//!
//! Every regular UPDATE/DELETE currently persists BOTH a query-level FilterOp
//! (CASE-WHEN projection / `WHERE NOT` filter, one stacked plan layer per
//! edit) AND block-level row-identity data (tombstones / overlay parquet,
//! applied inside `DataBlock::scan`). This target isolates each
//! representation's query-time cost so the owner can decide which one the
//! commands should emit.
//!
//! Representation arms are composed from public `BundleBuilder` APIs — the
//! same composition the always-rules use (attach.rs) — with NO production
//! changes. Keep the arm bodies in sync with:
//!   - DELETE: rust/bundlebase-command/src/builder/delete.rs (execute)
//!   - UPDATE: rust/bundlebase-command/src/builder/update.rs (execute)
//!
//! All query measurements run against committed + reopened bundles: that is
//! the representation-pure state (the manifest carries only the ops the arm
//! emitted) and sidesteps the blocks-only arm's in-session invisibility.
//!
//! Run a single group with e.g.:
//!   BENCH_GROUPS=q1 cargo bench -p bundlebase --bench filterop_vs_blockdata -- q1_
//! Validate setups without timing:
//!   BENCH_GROUPS=q1 cargo bench -p bundlebase --bench filterop_vs_blockdata -- --test q1_
//!
//! IMPORTANT: criterion's name filter (the `q1_` positional arg) only skips
//! MEASUREMENTS — every group function still runs its (expensive) bundle
//! setups. BENCH_GROUPS gates the setups themselves; always set it.

mod bench_data;
mod bench_helpers;
mod data_generator;
mod throttled_store;

use bench_data::Format;
use bench_helpers::{create_runtime, fresh_local_dir};
use bundlebase::bundle::operation::FilterOp;
use bundlebase::bundle::BundleFacade;
use bundlebase::{Bundle, BundleBuilder};
use bundlebase_command::BundleBuilderExt;
use bundlebase_command::BundleFacadeCommandExt;
use bundlebase_common::BundlebaseError;
use criterion::{criterion_group, BenchmarkId, Criterion};
use data_generator::{SCALE_100K, SCALE_10K};
use futures::StreamExt;
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Representations
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Repr {
    /// Production behavior: FilterOp + tombstone/overlay (both).
    Current,
    /// Block-level only: tombstones / overlay parquet, no FilterOp.
    BlocksOnly,
    /// Query-level only: stacked FilterOps, no block data.
    FilterOnly,
}

impl Repr {
    fn name(&self) -> &'static str {
        match self {
            Repr::Current => "current",
            Repr::BlocksOnly => "blocks_only",
            Repr::FilterOnly => "filter_only",
        }
    }
    const ALL: [Repr; 3] = [Repr::Current, Repr::BlocksOnly, Repr::FilterOnly];
}

/// Rows per edit and id spacing between edits. Edits are disjoint id ranges
/// so all three representations are logically identical.
const EDIT_WIDTH: usize = 10;
const EDIT_SPACING: usize = 50;
/// Marker value for updated cells (never produced by the data generator).
const UPDATE_MARKER: f64 = -12345.5;

/// Exact IN-list predicate for edit k. `quoted` for line-oriented formats:
/// CSV/JSONL attaches type every column Utf8 (bb-bh9), so range predicates
/// compare lexicographically — exact (IN) matches are the only type-proof
/// numeric predicate there.
fn edit_pred(k: usize, quoted: bool) -> String {
    let lo = k * EDIT_SPACING;
    let vals: Vec<String> = (lo..lo + EDIT_WIDTH)
        .map(|v| {
            if quoted {
                format!("'{v}'")
            } else {
                v.to_string()
            }
        })
        .collect();
    format!("id IN ({})", vals.join(", "))
}

/// Apply one DELETE in the given representation.
/// Mirrors rust/bundlebase-command/src/builder/delete.rs — keep in sync.
async fn apply_delete(
    b: &Arc<BundleBuilder>,
    repr: Repr,
    pred: &str,
) -> Result<(), BundlebaseError> {
    let wc = b.translate_sql(pred);
    match repr {
        Repr::Current => {
            b.delete(pred).await?;
        }
        Repr::BlocksOnly => {
            b.run_command(format!("DELETE(blocks-only) WHERE {wc}"), async {
                let ids = b.select_row_ids(&wc).await?;
                b.mark_deleted(ids, &wc);
                Ok(())
            })
            .await?;
        }
        Repr::FilterOnly => {
            let filter_sql = format!("SELECT * FROM bundle WHERE NOT ({wc})");
            b.run_command(format!("DELETE(filter-only) WHERE {wc}"), async {
                b.apply_operation(FilterOp::new(&filter_sql, vec![]).into())
                    .await
            })
            .await?;
        }
    }
    Ok(())
}

/// Apply one UPDATE (`SET amount = MARKER WHERE pred`) in the given
/// representation. Mirrors rust/bundlebase-command/src/builder/update.rs.
async fn apply_update(
    b: &Arc<BundleBuilder>,
    repr: Repr,
    pred: &str,
) -> Result<(), BundlebaseError> {
    match repr {
        Repr::Current => {
            b.update(&format!("SET amount = {UPDATE_MARKER} WHERE {pred}"))
                .await?;
        }
        Repr::BlocksOnly => {
            let wc = b.translate_sql(pred);
            let col = b.translate_sql("amount");
            b.run_command(format!("UPDATE(blocks-only) WHERE {wc}"), async {
                b.evaluate_update_cols(
                    std::slice::from_ref(&col),
                    &[UPDATE_MARKER.to_string()],
                    &wc,
                )
                .await?;
                b.flush_pending_updates_to_blocks()?;
                Ok(())
            })
            .await?;
        }
        Repr::FilterOnly => {
            // CASE-WHEN projection over the full (generator) schema, written in
            // user-visible names and translated to internal names wholesale.
            let case_sql = format!(
                "SELECT id, filter_value, \
                 CASE WHEN ({pred}) THEN ({UPDATE_MARKER}) ELSE amount END AS amount, \
                 category, name, region, description FROM bundle"
            );
            let translated = b.translate_sql(&case_sql);
            b.run_command(format!("UPDATE(filter-only) WHERE {pred}"), async {
                b.apply_operation(FilterOp::new(&translated, vec![]).into())
                    .await
            })
            .await?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Delete,
    Update,
}

/// Group gating via the BENCH_GROUPS env var (comma-separated group names,
/// e.g. `BENCH_GROUPS=q1,q4`). Unset/empty = all groups. Criterion's name
/// filter does NOT skip group setup code, so this is the only way to avoid
/// paying every group's bundle-construction cost on a targeted run.
fn group_enabled(group: &str) -> bool {
    match std::env::var("BENCH_GROUPS") {
        Ok(v) if !v.trim().is_empty() => v.split(',').any(|g| g.trim() == group),
        _ => true,
    }
}

// ---------------------------------------------------------------------------
// Bundle construction + correctness guards
// ---------------------------------------------------------------------------

/// Build a bundle with `n_edits` disjoint edits in `repr`, committed and
/// reopened. Panics (fails the benchmark) if the resulting bundle does not
/// have the exact expected semantics — a mistimed benchmark is worse than a
/// loud one.
async fn build_edited_bundle(
    rows: usize,
    format: &Format,
    repr: Repr,
    n_edits: usize,
    kind: EditKind,
    // Throttle the SOURCE data URL (bundle dir stays local): queries then
    // measure throttled reads while setup pays only the attach-time passes.
    throttle_source: bool,
) -> (String, Arc<Bundle>) {
    let t = std::time::Instant::now();
    let mut data_url = bench_data::get_data_url(rows, format);
    if throttle_source {
        if let Some(path) = data_url.strip_prefix("file://") {
            data_url = format!("throttle://{path}");
        }
    }
    let dir_url = fresh_local_dir("ab");
    let b = BundleBuilder::create(&dir_url, None)
        .await
        .expect("bundle create");
    b.attach(&data_url, None).await.expect("attach");
    b.commit("setup").await.expect("setup commit");

    let quoted_ids = !matches!(format, Format::Parquet);
    for k in 0..n_edits {
        let pred = edit_pred(k, quoted_ids);
        match kind {
            EditKind::Delete => apply_delete(&b, repr, &pred).await.expect("delete"),
            EditKind::Update => apply_update(&b, repr, &pred).await.expect("update"),
        }
    }
    if n_edits > 0 {
        b.commit("edits").await.expect("edits commit");
    }
    let url = b.url().to_string();
    drop(b);

    let bundle = Bundle::open(&url, None).await.expect("reopen");
    verify_bundle(&bundle, rows, repr, n_edits, kind).await;
    eprintln!(
        "[setup] {rows}r {} {:?} {n_edits}e throttle_src={throttle_source}: {:?}",
        format.name(),
        repr,
        t.elapsed()
    );
    (url, bundle)
}

/// Stream `SELECT * FROM bundle` and count rows (authoritative row count —
/// COUNT(*) may take statistics shortcuts).
async fn streamed_count(bundle: &Arc<Bundle>, sql: &str) -> usize {
    let mut stream = bundle.query(sql, vec![], None).await.expect("query");
    let mut n = 0usize;
    while let Some(batch) = stream.next().await {
        n += batch.expect("batch").num_rows();
    }
    n
}

async fn verify_bundle(
    bundle: &Arc<Bundle>,
    rows: usize,
    repr: Repr,
    n_edits: usize,
    kind: EditKind,
) {
    // Semantic guard.
    match kind {
        EditKind::Delete => {
            let expected = rows - n_edits * EDIT_WIDTH;
            let got = streamed_count(bundle, "SELECT * FROM bundle").await;
            assert_eq!(
                got, expected,
                "{:?}/{} deletes: wrong visible row count",
                repr, n_edits
            );
        }
        EditKind::Update => {
            let expected = n_edits * EDIT_WIDTH;
            let got = streamed_count(
                bundle,
                &format!("SELECT * FROM bundle WHERE amount = {UPDATE_MARKER}"),
            )
            .await;
            assert_eq!(
                got, expected,
                "{:?}/{} updates: wrong marker row count",
                repr, n_edits
            );
        }
    }

    // Composition guard: the manifest must contain only the ops this arm emits.
    let ops: Vec<String> = bundle
        .operations()
        .iter()
        .map(bundlebase::Operation::describe)
        .collect();
    let filters = ops.iter().filter(|o| o.starts_with("FILTER:")).count();
    let block_ops = ops
        .iter()
        .filter(|o| o.starts_with("DELETE:") || o.starts_with("UPDATE DATA:"))
        .count();
    match repr {
        Repr::BlocksOnly => assert_eq!(
            filters, 0,
            "blocks-only arm leaked FilterOps into the manifest: {ops:?}"
        ),
        Repr::FilterOnly => assert_eq!(
            block_ops, 0,
            "filter-only arm leaked block ops into the manifest: {ops:?}"
        ),
        Repr::Current => {
            // Post ADR-009, the production commands emit ONLY the block-level
            // representation — `current` should now match `blocks_only`.
            if n_edits > 0 {
                assert_eq!(
                    filters, 0,
                    "current arm should no longer emit FilterOps (ADR-009): {ops:?}"
                );
                assert!(block_ops > 0, "current arm should have block ops");
            }
        }
    }
}

/// Report (once per cell, to stderr) which scan strategy the plan uses so the
/// write-up can attribute timings to the path actually measured.
async fn report_plan_path(label: &str, bundle: &Arc<Bundle>, sql: &str) {
    let mut stream = bundle
        .explain(false, false, datafusion::logical_expr::ExplainFormat::Indent, Some(sql))
        .await
        .expect("explain");
    let mut plan = String::new();
    while let Some(batch) = stream.next().await {
        let batch = batch.expect("explain batch");
        let types = batch
            .column(0)
            .as_any()
            .downcast_ref::<arrow::array::StringArray>()
            .expect("plan_type");
        let texts = batch
            .column(1)
            .as_any()
            .downcast_ref::<arrow::array::StringArray>()
            .expect("plan");
        for i in 0..batch.num_rows() {
            if types.value(i) == "physical_plan" {
                plan.push_str(texts.value(i));
            }
        }
    }
    let path = if plan.contains("PageMap") {
        "page_or_rowid_read"
    } else if plan.contains("SchemaRenameDataSource") {
        "block_scan"
    } else if plan.contains("MemorySourceConfig") || plan.contains("DataSourceExec") {
        "cached_or_plain_scan"
    } else {
        "other"
    };
    eprintln!("[plan-path] {label}: {path}");
}

/// Stream a query to completion (the standard measured operation).
async fn run_query(bundle: &Arc<Bundle>, sql: &str) {
    let mut stream = bundle.query(sql, vec![], None).await.expect("query");
    while let Some(batch) = stream.next().await {
        let _ = batch.expect("batch");
    }
}

// ---------------------------------------------------------------------------
// q1: query cost after N edits (parquet, file://)
// ---------------------------------------------------------------------------

fn bench_q1_query_cost(c: &mut Criterion) {
    if !group_enabled("q1") {
        return;
    }
    let rt = create_runtime();
    let mut group = c.benchmark_group("q1_query_cost");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(8));

    for kind in [EditKind::Delete, EditKind::Update] {
        // (rows, n_edits) cells: N sweep at 10K, scale check at 100K.
        // The UPDATE ladder is capped at 16: stacked CASE projections are
        // EXPONENTIAL in layer count (~2x per layer; 20 layers = 57s per
        // query, 100 layers never finishes). See the write-up's stacking
        // curve — the cap keeps the benchmark finite while the pain is
        // still clearly visible at 16.
        let cells: [(usize, usize); 4] = match kind {
            EditKind::Delete => [
                (SCALE_10K, 1),
                (SCALE_10K, 10),
                (SCALE_10K, 100),
                (SCALE_100K, 10),
            ],
            EditKind::Update => [
                (SCALE_10K, 1),
                (SCALE_10K, 10),
                (SCALE_10K, 16),
                (SCALE_100K, 10),
            ],
        };
        let kind_name = match kind {
            EditKind::Delete => "del",
            EditKind::Update => "upd",
        };
        for (rows, n) in cells {
            for repr in Repr::ALL {
                let (_url, bundle) = rt.block_on(build_edited_bundle(
                    rows,
                    &Format::Parquet,
                    repr,
                    n,
                    kind,
                    false,
                ));

                let queries: [(&str, String); 3] = [
                    ("select_star", "SELECT * FROM bundle".to_string()),
                    (
                        "selective_where",
                        "SELECT * FROM bundle WHERE filter_value = 42".to_string(),
                    ),
                    ("count_star", "SELECT COUNT(*) FROM bundle".to_string()),
                ];
                for (qname, sql) in &queries {
                    group.bench_with_input(
                        BenchmarkId::new(
                            format!("{kind_name}_{qname}_{}", repr.name()),
                            format!("{rows}r_{n}e"),
                        ),
                        &sql,
                        |bch, sql| {
                            let bundle = bundle.clone();
                            bch.to_async(&rt).iter(|| {
                                let bundle = bundle.clone();
                                let sql = (*sql).clone();
                                async move { run_query(&bundle, &sql).await }
                            });
                        },
                    );
                }
            }
        }
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// q2: CSV page-skip interplay (file:// and throttle://)
// ---------------------------------------------------------------------------

fn bench_q2_page_skip(c: &mut Criterion) {
    if !group_enabled("q2") {
        return;
    }
    let rt = create_runtime();
    let mut group = c.benchmark_group("q2_page_skip");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(10));

    // Selective range far from the edited ids ([0, 100*50)): pages covering it
    // should be the only ones read when page skipping engages.
    // Quoted range: ids are Utf8 after a JSONL attach (bb-bh9). The
    // lexicographic match set is deterministic and identical across arms,
    // and string page stats still prune.
    let sql = "SELECT * FROM bundle WHERE id >= '90000' AND id < '90100'";
    let n_edits = 10;

    for throttle_source in [false, true] {
        let scheme = if throttle_source { "throttle" } else { "file" };
        // Clean baseline: page-skip upper bound. JSON (not CSV): CSV attach
        // types every column Utf8 (bb-bh9), which breaks numeric predicates;
        // JSONL keeps numeric ids and has the same page-filter machinery.
        {
            let (_url, bundle) = rt.block_on(build_edited_bundle(
                SCALE_100K,
                &Format::Json,
                Repr::Current,
                0,
                EditKind::Delete,
                throttle_source,
            ));
            rt.block_on(report_plan_path(
                &format!("q2_clean_{scheme}"),
                &bundle,
                sql,
            ));
            group.bench_function(BenchmarkId::new("clean", scheme), |bch| {
                let bundle = bundle.clone();
                bch.to_async(&rt).iter(|| {
                    let bundle = bundle.clone();
                    async move { run_query(&bundle, sql).await }
                });
            });
        }
        // Edited arms.
        for repr in [Repr::FilterOnly, Repr::BlocksOnly, Repr::Current] {
            let (_url, bundle) = rt.block_on(build_edited_bundle(
                SCALE_100K,
                &Format::Json,
                repr,
                n_edits,
                EditKind::Delete,
                throttle_source,
            ));
            rt.block_on(report_plan_path(
                &format!("q2_{}_{scheme}", repr.name()),
                &bundle,
                sql,
            ));
            group.bench_function(BenchmarkId::new(repr.name(), scheme), |bch| {
                let bundle = bundle.clone();
                bch.to_async(&rt).iter(|| {
                    let bundle = bundle.clone();
                    async move { run_query(&bundle, sql).await }
                });
            });
        }
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// q3: indexed point query (parquet + csv, 100K, file://)
// ---------------------------------------------------------------------------

fn bench_q3_indexed_point(c: &mut Criterion) {
    if !group_enabled("q3") {
        return;
    }
    let rt = create_runtime();
    let mut group = c.benchmark_group("q3_indexed_point");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(8));

    // Probe outside every edited range (quoted variant for line formats
    // whose columns are Utf8 — bb-bh9).
    let sql_typed = "SELECT * FROM bundle WHERE id = 76543";
    let sql_quoted = "SELECT * FROM bundle WHERE id = '76543'";

    // JSON (not CSV) for the line-oriented format: CSV attach types every
    // column Utf8 (bb-bh9), breaking the numeric edit predicates.
    let cells: [(Format, usize); 3] = [
        (Format::Parquet, 1),
        (Format::Parquet, 100),
        (Format::Json, 1),
    ];

    for (format, n) in cells {
        let quoted_ids = !matches!(format, Format::Parquet);
        let sql = if quoted_ids { sql_quoted } else { sql_typed };
        for repr in Repr::ALL {
            let (_url, bundle) = rt.block_on(async {
                let data_url = bench_data::get_data_url(SCALE_100K, &format);
                let dir_url = fresh_local_dir("ab_idx");
                let b = BundleBuilder::create(&dir_url, None).await.expect("create");
                b.attach(&data_url, None).await.expect("attach");
                b.create_index(&["id"], bundlebase_index::IndexType::BTree, None)
                    .await
                    .expect("index");
                b.commit("setup+index").await.expect("commit");
                for k in 0..n {
                    apply_delete(&b, repr, &edit_pred(k, quoted_ids))
                        .await
                        .expect("delete");
                }
                b.commit("edits").await.expect("edits commit");
                let url = b.url().to_string();
                drop(b);
                let bundle = Bundle::open(&url, None).await.expect("reopen");
                verify_bundle(&bundle, SCALE_100K, repr, n, EditKind::Delete).await;
                (url, bundle)
            });
            rt.block_on(report_plan_path(
                &format!("q3_{}_{}e_{}", format.name(), n, repr.name()),
                &bundle,
                sql,
            ));
            group.bench_with_input(
                BenchmarkId::new(
                    format!("{}_{}", format.name(), repr.name()),
                    format!("{n}e"),
                ),
                &sql,
                |bch, sql| {
                    let bundle = bundle.clone();
                    bch.to_async(&rt).iter(|| {
                        let bundle = bundle.clone();
                        async move { run_query(&bundle, sql).await }
                    });
                },
            );
        }
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// q4: COUNT(*) after deletes (parquet, 100K, file://)
// ---------------------------------------------------------------------------

fn bench_q4_count_after_delete(c: &mut Criterion) {
    if !group_enabled("q4") {
        return;
    }
    let rt = create_runtime();
    let mut group = c.benchmark_group("q4_count_after_delete");
    group.sample_size(10);

    let sql = "SELECT COUNT(*) FROM bundle";

    // Clean metadata-only count = the tombstone representation's upper bound.
    {
        let (_url, bundle) = rt.block_on(build_edited_bundle(
            SCALE_100K,
            &Format::Parquet,
            Repr::Current,
            0,
            EditKind::Delete,
            false,
        ));
        group.bench_function("clean", |bch| {
            let bundle = bundle.clone();
            bch.to_async(&rt).iter(|| {
                let bundle = bundle.clone();
                async move { run_query(&bundle, sql).await }
            });
        });
    }
    for repr in Repr::ALL {
        let (_url, bundle) = rt.block_on(build_edited_bundle(
            SCALE_100K,
            &Format::Parquet,
            repr,
            10,
            EditKind::Delete,
            false,
        ));
        group.bench_function(repr.name(), |bch| {
            let bundle = bundle.clone();
            bch.to_async(&rt).iter(|| {
                let bundle = bundle.clone();
                async move { run_query(&bundle, sql).await }
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// q5: open + first-query cost vs operation count (10K, file://)
// ---------------------------------------------------------------------------

fn bench_q5_open_and_build(c: &mut Criterion) {
    if !group_enabled("q5") {
        return;
    }
    let rt = create_runtime();
    let mut group = c.benchmark_group("q5_open_and_build");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(8));

    for n in [1usize, 10, 100] {
        for repr in Repr::ALL {
            let (url, _bundle) = rt.block_on(build_edited_bundle(
                SCALE_10K,
                &Format::Parquet,
                repr,
                n,
                EditKind::Delete,
                false,
            ));

            group.bench_with_input(
                BenchmarkId::new(format!("open_only_{}", repr.name()), format!("{n}e")),
                &url,
                |bch, url| {
                    let url = url.clone();
                    bch.to_async(&rt).iter(|| {
                        let url = url.clone();
                        async move {
                            let _ = Bundle::open(&url, None).await.expect("open");
                        }
                    });
                },
            );
            group.bench_with_input(
                BenchmarkId::new(format!("open_first_query_{}", repr.name()), format!("{n}e")),
                &url,
                |bch, url| {
                    let url = url.clone();
                    bch.to_async(&rt).iter(|| {
                        let url = url.clone();
                        async move {
                            let bundle = Bundle::open(&url, None).await.expect("open");
                            run_query(&bundle, "SELECT id FROM bundle LIMIT 1").await;
                        }
                    });
                },
            );
        }
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_q1_query_cost,
    bench_q2_page_skip,
    bench_q3_indexed_point,
    bench_q4_count_after_delete,
    bench_q5_open_and_build,
);

bench_helpers::bench_main!(benches);
