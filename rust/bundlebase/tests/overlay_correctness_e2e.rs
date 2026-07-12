//! End-to-end coverage for the UPDATE/DELETE overlay & tombstone application
//! across every `scan()` fast-path (bb-fqm epic).
//!
//! Each fast-path in `DataBlock::scan` (index lookup, page-filtered read, full
//! scan) and the `search()` UDTF must return the SAME logical rows: pending
//! updates applied, deleted rows removed. These tests pin that invariant so a
//! fast-path can't silently diverge from the full-scan reference.

use arrow::array::{Array, StringArray};
use bundlebase::bundle::BundleFacade;
use bundlebase::test_utils::{random_memory_dir, test_datafile};
use bundlebase_command::BundleBuilderExt;
use bundlebase_command::BundleFacadeCommandExt;
use bundlebase_common::BundlebaseError;
use bundlebase_index::IndexType;
use datafusion::logical_expr::ExplainFormat;
use futures::{StreamExt, TryStreamExt};

mod common;

fn init() {
    common::init_catalog();
}

/// Run a query and collect all batches.
async fn query_collect(facade: &dyn BundleFacade, sql: &str) -> Vec<arrow::array::RecordBatch> {
    let stream = facade
        .query(sql, vec![], None)
        .await
        .unwrap_or_else(|e| panic!("query failed for `{}`: {}", sql, e));
    stream
        .try_collect()
        .await
        .unwrap_or_else(|e| panic!("batch collect failed for `{}`: {}", sql, e))
}

/// Run a COUNT query and return the scalar count.
async fn query_count(facade: &dyn BundleFacade, sql: &str) -> i64 {
    let batches = query_collect(facade, sql).await;
    let batch = batches
        .iter()
        .find(|b| b.num_rows() > 0)
        .unwrap_or_else(|| panic!("query `{}` returned no rows", sql));
    batch
        .column(0)
        .as_any()
        .downcast_ref::<arrow::array::Int64Array>()
        .unwrap_or_else(|| panic!("query `{}`: expected Int64 count", sql))
        .value(0)
}

/// Read a single string cell from the first row of a one-row result.
fn single_string(batches: &[arrow::array::RecordBatch], column: &str) -> Option<String> {
    let batch = batches.iter().find(|b| b.num_rows() > 0)?;
    let col = batch.column_by_name(column)?;
    let arr = col.as_any().downcast_ref::<StringArray>()?;
    if arr.is_null(0) {
        None
    } else {
        Some(arr.value(0).to_string())
    }
}

/// Physical plan text for a SQL query (used to confirm which fast-path runs).
async fn physical_plan<T: BundleFacadeCommandExt + Sync>(facade: &T, sql: &str) -> String {
    let mut stream = facade
        .explain(false, false, ExplainFormat::Indent, Some(sql))
        .await
        .expect("explain failed");
    let mut plans = Vec::new();
    while let Some(batch_result) = stream.next().await {
        let batch = batch_result.expect("explain batch");
        let plan_types = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("plan_type column");
        let plan_texts = batch
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("plan column");
        for i in 0..batch.num_rows() {
            if plan_types.value(i) == "physical_plan" {
                plans.push(plan_texts.value(i).to_string());
            }
        }
    }
    plans.join("\n")
}

/// bb-fqm.1: an indexed query must see pending UPDATEs.
///
/// The index is built on the committed data; an UPDATE adds an overlay. A query
/// whose filter is served by the index must still reflect the overlay, not the
/// stale base value.
#[tokio::test]
async fn test_update_visible_through_index_lookup() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;
    // Index the column we filter on, but update a DIFFERENT column so the index
    // still locates the row correctly — the bug is the stale projected value.
    bundle.create_index(&["Email"], IndexType::BTree, None).await?;
    bundle.commit("index on Email").await?;

    let email = "zunigavanessa@smith.info";
    bundle
        .update(&format!("SET City = 'OVERLAY_CITY' WHERE Email = '{email}'"))
        .await?;
    // Commit so the update is persisted as a block-level overlay parquet, then
    // reopen: a fresh Bundle loads the committed overlay via UpdateDataOp and
    // applies it at block-scan time (the path the index fast-path can bypass),
    // rather than the session-level CASE projection the builder keeps.
    bundle.commit("update City via overlay").await?;
    let reopened = bundlebase::Bundle::open(data_dir.url().as_str(), None).await?;

    let sql = format!("SELECT City FROM bundle WHERE Email = '{email}'");

    // Design note (bb-fqm.1): with update overlays on the block, the index
    // fast-path defers to the overlay-applying fallback scans, so the plan is
    // NOT expected to show the indexed-lookup source here. The blocks-only
    // companion test asserts the returned values; this variant additionally
    // exercises the query-level CASE projection the current UPDATE command
    // still records.
    let _plan = physical_plan(&reopened, &sql).await;

    let batches = query_collect(reopened.as_ref(), &sql).await;
    assert_eq!(
        single_string(&batches, "City").as_deref(),
        Some("OVERLAY_CITY"),
        "indexed query returned stale pre-UPDATE City (overlay not applied on index path)"
    );

    Ok(())
}

/// bb-fqm.4: UNDO of a DELETE must not persist the delete.
///
/// A DELETE records a change AND adds the matched RowIds to the builder's
/// `pending_deletes` side-state. `undo()` pops the change and reloads, but if it
/// doesn't also reconcile `pending_deletes`, the next commit still tombstones
/// those rows — silent, permanent data loss.
#[tokio::test]
async fn test_undo_delete_does_not_persist_the_delete() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    builder.commit("attach").await?;

    let total = builder.num_rows().await?;
    assert!(total > 0, "fixture should have rows");

    // Delete a chunk, then immediately undo it.
    builder.delete("salary > 200000").await?;
    let after_delete = builder.num_rows().await?;
    assert!(
        after_delete < total,
        "delete should remove rows in-session ({after_delete} !< {total})"
    );

    builder.undo().await?;

    // In-session, the undo restored the rows.
    let after_undo = builder.num_rows().await?;
    assert_eq!(
        after_undo, total,
        "undo should restore all rows in the live session"
    );

    // Make an unrelated committable change AFTER the undo. At commit, the stale
    // pending-delete tombstone is attached to the last change in the changeset,
    // so this is what surfaces the bug: the undone delete rides along.
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    let expected = builder.num_rows().await?; // in-session truth before commit

    builder.commit("second attach after undo").await?;
    let reopened = bundlebase::Bundle::open(data_dir.url().as_str(), None).await?;
    // Check both the streamed row set and COUNT(*): they exercise different
    // paths (scan-level tombstone filtering vs statistics-based counting) and
    // each has independently lied here before.
    let batches = query_collect(reopened.as_ref(), "SELECT * FROM bundle").await;
    let streamed: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(
        streamed, expected,
        "an undone DELETE must not be persisted at the next commit \
         (pending_deletes not reconciled by undo)"
    );
    let persisted = query_count(reopened.as_ref(), "SELECT COUNT(*) AS cnt FROM bundle").await;
    assert_eq!(persisted, expected as i64, "COUNT(*) must agree");

    Ok(())
}

/// bb-fqm.3: a committed DELETE must stay excluded when the query is served by
/// a scan fast-path (index lookup / page filter), not just the full scan.
#[tokio::test]
async fn test_delete_excluded_through_index_lookup() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;
    bundle.create_index(&["Email"], IndexType::BTree, None).await?;
    bundle.commit("index on Email").await?;

    let deleted_email = "zunigavanessa@smith.info";
    let kept_email = "elizabethbarr@ewing.com";

    bundle.delete(&format!("Email = '{deleted_email}'")).await?;
    bundle.commit("delete one row").await?;

    let reopened = bundlebase::Bundle::open(data_dir.url().as_str(), None).await?;

    // The deleted row must NOT come back through the indexed lookup.
    let gone = query_count(
        reopened.as_ref(),
        &format!("SELECT COUNT(*) AS cnt FROM bundle WHERE Email = '{deleted_email}'"),
    )
    .await;
    assert_eq!(gone, 0, "deleted row reappeared through the index fast-path");

    // A different, non-deleted row is still reachable through the index.
    let kept = query_count(
        reopened.as_ref(),
        &format!("SELECT COUNT(*) AS cnt FROM bundle WHERE Email = '{kept_email}'"),
    )
    .await;
    assert_eq!(kept, 1, "a live row went missing via the index fast-path");

    Ok(())
}

/// Write a ~13MB CSV (120_000 rows × ~110 bytes → 3 pages at the 5MB page
/// size) into `dir`. Ids are fixed-width strings ("r000000".."r119999") so
/// lexicographic ordering equals numeric ordering regardless of CSV type
/// inference, and the numeric suffix equals the block-local row number.
fn write_multipage_csv(dir: &std::path::Path) -> std::path::PathBuf {
    use std::io::Write;
    let csv_path = dir.join("threepages.csv");
    let file = std::fs::File::create(&csv_path).expect("create csv");
    let mut w = std::io::BufWriter::new(file);
    writeln!(w, "id,val").expect("write header");
    let pad = "x".repeat(100);
    for i in 0..120_000 {
        writeln!(w, "r{i:06},{pad}").expect("write row");
    }
    w.flush().expect("flush csv");
    csv_path
}

/// The page-filtered read must actually engage for a selective predicate on a
/// multi-page CSV. It silently died when columns moved to internal names:
/// scan filters say `col_<id>` while the reader's page stats are keyed by the
/// file's physical names, so no page ever pruned and the path always bailed.
#[tokio::test]
async fn test_page_filter_engages_on_multipage_csv() -> Result<(), BundlebaseError> {
    init();

    let tmp = tempfile::tempdir().expect("create tempdir");
    let csv_path = write_multipage_csv(tmp.path());

    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    let csv_url = format!("file://{}", csv_path.to_str().expect("utf8 path"));
    builder.attach(&csv_url, None).await?;
    builder.commit("attach three-page csv").await?;

    let reopened = bundlebase::Bundle::open(data_dir.url().as_str(), None).await?;

    // Selective range in the LAST page: earlier pages must be pruned, so the
    // plan should read via the page-filtered PageMapDataSource.
    let sql = "SELECT id FROM bundle WHERE id >= 'r100000' AND id < 'r118000'";
    let plan = physical_plan(&reopened, sql).await;
    assert!(
        plan.contains("PageMap"),
        "expected the page-filtered read in the plan, got:\n{plan}"
    );

    // And the results coming off the page-filtered read are the right rows.
    let live = query_count(
        reopened.as_ref(),
        "SELECT COUNT(*) AS cnt FROM bundle WHERE id >= 'r100000' AND id < 'r118000'",
    )
    .await;
    assert_eq!(live, 18_000);

    Ok(())
}

/// bb-fqm.3: a page-filtered read must not misapply delete tombstones.
///
/// Tombstones are block-global row ordinals. The page-filtered read skips
/// pages, so applying tombstone ordinals to the shortened stream drops the
/// WRONG rows: live rows from a later page vanish (and the query-level
/// `WHERE NOT` filter above can re-drop rows but never restore them).
///
/// Construction: a 3-page CSV (>2× the 5MB page size) with sequential ids;
/// delete rows 1000..9000 (page 0); query a range that lives entirely in the
/// LAST page. The buggy path prunes pages 0-1, then drops stream ordinals
/// 1000..9000 — which now land on live last-page rows inside the query range.
#[tokio::test]
async fn test_page_filtered_read_respects_tombstones() -> Result<(), BundlebaseError> {
    init();

    let tmp = tempfile::tempdir().expect("create tempdir");
    let csv_path = write_multipage_csv(tmp.path());

    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    let csv_url = format!("file://{}", csv_path.to_str().expect("utf8 path"));
    builder.attach(&csv_url, None).await?;
    builder.commit("attach three-page csv").await?;

    // Delete rows in the FIRST page. Ids are sequential from r000000, so the
    // numeric suffix equals the block-local row number.
    builder
        .delete("id >= 'r001000' AND id < 'r009000'")
        .await?;
    builder.commit("delete first-page rows").await?;

    let reopened = bundlebase::Bundle::open(data_dir.url().as_str(), None).await?;

    // Selective range living entirely in the LAST page (page 2 starts around
    // row ~96-99K for this row width; [100K, 118K) is safely inside it). None
    // of the deleted ids are in this range, so every row must be present.
    let live = query_count(
        reopened.as_ref(),
        "SELECT COUNT(*) AS cnt FROM bundle WHERE id >= 'r100000' AND id < 'r118000'",
    )
    .await;
    assert_eq!(
        live, 18_000,
        "live rows in a later page must survive a page-filtered read with \
         tombstones present (wrong-ordinal deletes drop them)"
    );

    Ok(())
}

/// COUNT(*) must reflect tombstoned rows.
///
/// DeletedRowFilterDataSource forwarded its inner source's EXACT statistics,
/// so DataFusion's AggregateStatistics rule rewrote COUNT(*) to the raw row
/// count without ever executing the delete filter. A tombstone-only delete
/// (no query-level FilterOp above to mask it) exposes the lie: COUNT(*) says
/// no rows are gone while SELECT * streams the reduced set.
#[tokio::test]
async fn test_count_star_reflects_tombstones() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    builder.commit("attach").await?;
    let total = builder.num_rows().await?;

    // Blocks-only delete: tombstone RowIds without the query-level FilterOp
    // (the same composition the always-rules and the representation benchmarks
    // use). The tombstone is persisted by the commit.
    let wc = builder.translate_sql("salary > 200000");
    let ids = builder.select_row_ids(&wc).await?;
    let n_deleted = ids.len();
    assert!(n_deleted > 0);
    builder
        .run_command("blocks-only delete".to_string(), async {
            builder.mark_deleted(ids, &wc);
            Ok(())
        })
        .await?;
    builder.commit("tombstone-only delete").await?;

    let reopened = bundlebase::Bundle::open(data_dir.url().as_str(), None).await?;
    let batches = query_collect(reopened.as_ref(), "SELECT * FROM bundle").await;
    let streamed: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(
        streamed,
        total - n_deleted,
        "SELECT * must exclude tombstoned rows"
    );

    let counted = query_count(reopened.as_ref(), "SELECT COUNT(*) AS cnt FROM bundle").await;
    assert_eq!(
        counted as usize, streamed,
        "COUNT(*) must agree with the streamed row count on a tombstoned block \
         (stats passthrough lets the optimizer count raw rows)"
    );

    Ok(())
}

/// bb-wmu: a DELETE whose predicate no longer matches a row's UPDATED value
/// must not tombstone that row.
///
/// Tombstone selection reads raw file data; if it ignores update overlays, a
/// row updated OUT of the delete range is still tombstoned. The committed
/// tombstone then removes it at scan — and the query-level `WHERE NOT` filter
/// can drop rows but never restore them, so the row silently vanishes.
#[tokio::test]
async fn test_delete_after_update_does_not_lose_rows() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    builder.commit("attach").await?;

    let total = builder.num_rows().await?;
    let low = query_count(
        builder.as_ref(),
        "SELECT COUNT(*) AS cnt FROM bundle WHERE salary < 50000",
    )
    .await;
    assert!(low > 0, "fixture should have low salaries");

    // Move every low-salary row OUT of the delete range, then delete the range.
    builder
        .update("SET salary = 999999.0 WHERE salary < 50000")
        .await?;
    builder.delete("salary < 50000").await?;

    // In-session, nothing should be gone.
    assert_eq!(
        builder.num_rows().await?,
        total,
        "no row's updated value matches the delete predicate"
    );

    builder.commit("update then delete").await?;
    let reopened = bundlebase::Bundle::open(data_dir.url().as_str(), None).await?;
    let persisted = query_count(reopened.as_ref(), "SELECT COUNT(*) AS cnt FROM bundle").await;
    assert_eq!(
        persisted, total as i64,
        "rows updated out of the delete range must survive commit+reopen \
         (wrong tombstones = silent row loss)"
    );

    Ok(())
}

/// Phase 2 (FilterOp removal): a DELETE's tombstones must be visible
/// in-session at the BLOCK level — without any query-level FilterOp above.
/// `mark_deleted` must flush row numbers into the blocks immediately, the way
/// updates are flushed.
#[tokio::test]
async fn test_block_level_delete_visible_in_session() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    builder.commit("attach").await?;
    let total = builder.num_rows().await?;

    // Blocks-only delete: no FilterOp — exactly what the DELETE command does
    // once the duplicate FilterOp is removed.
    let wc = builder.translate_sql("salary > 200000");
    let ids = builder.select_row_ids(&wc).await?;
    let n = ids.len();
    assert!(n > 0);
    builder
        .run_command("blocks-only delete".to_string(), async {
            builder.mark_deleted(ids, &wc);
            Ok(())
        })
        .await?;

    let batches = query_collect(builder.as_ref(), "SELECT * FROM bundle").await;
    let streamed: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(
        streamed,
        total - n,
        "tombstoned rows must disappear from same-session queries at the block level"
    );
    Ok(())
}

/// A command that mutates block-level state (overlays/tombstones) and then
/// FAILS must not leave that state behind. Pre-existing gap for updates: the
/// overlay was flushed to blocks mid-command; on failure the pending
/// accumulators are restored but the block overlay lingered.
#[tokio::test]
async fn test_failed_command_rolls_back_flushed_block_state() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    builder.commit("attach").await?;

    let col = builder.translate_sql("salary");
    let wc = builder.translate_sql("salary > 200000");
    let result = builder
        .run_command("update then fail".to_string(), async {
            builder
                .evaluate_update_cols(
                    std::slice::from_ref(&col),
                    &["-777.0".to_string()],
                    &wc,
                )
                .await?;
            builder.flush_pending_updates_to_blocks();
            Err::<(), BundlebaseError>("simulated failure".into())
        })
        .await;
    assert!(result.is_err());

    let marked = query_count(
        builder.as_ref(),
        "SELECT COUNT(*) AS c FROM bundle WHERE salary = -777.0",
    )
    .await;
    assert_eq!(
        marked, 0,
        "a failed command's flushed overlay must not remain visible"
    );
    Ok(())
}

/// Phase 2 (bb-fqm.1): an indexed query must see committed update overlays at
/// the BLOCK level — with no query-level CASE projection above to mask the
/// index fast-path's overlay skip.
#[tokio::test]
async fn test_block_level_update_visible_through_index() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;
    builder
        .create_index(&["Email"], IndexType::BTree, None)
        .await?;
    builder.commit("index on Email").await?;

    // Blocks-only update: overlay without the CASE FilterOp — exactly what the
    // UPDATE command emits once the duplicate FilterOp is removed.
    let email = "zunigavanessa@smith.info";
    let city_col = builder.translate_sql("City");
    let wc = builder.translate_sql(&format!("Email = '{email}'"));
    let n = builder
        .run_command("blocks-only update".to_string(), async {
            let n = builder
                .evaluate_update_cols(
                    std::slice::from_ref(&city_col),
                    &["'BLOCKS_CITY'".to_string()],
                    &wc,
                )
                .await?;
            builder.flush_pending_updates_to_blocks();
            Ok(n)
        })
        .await?;
    assert_eq!(n, 1);
    builder.commit("blocks-only update").await?;

    let reopened = bundlebase::Bundle::open(data_dir.url().as_str(), None).await?;
    let sql = format!("SELECT City FROM bundle WHERE Email = '{email}'");
    let batches = query_collect(reopened.as_ref(), &sql).await;
    assert_eq!(
        single_string(&batches, "City").as_deref(),
        Some("BLOCKS_CITY"),
        "indexed lookup must reflect the block-level overlay \
         (index fast-path must not skip overlays)"
    );
    Ok(())
}
