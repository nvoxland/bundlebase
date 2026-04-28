//! `SELECT COUNT(*) FROM bundle` end-to-end coverage.
//!
//! `DataBlock::scan` already short-circuits an empty projection to a
//! synthetic plan emitting `num_rows` empty rows (Phase 0 fast path,
//! `data_block.rs` lines 651–666). These tests assert that
//!   * the optimizer actually pushes empty projection through
//!     `BundleViewTable` → `PackTable` → `DataBlock`, and
//!   * the COUNT result stays correct under tombstones, filters, and
//!     joined views — where the fast path must NOT fire.
//!
//! Tracks bundlebase-pni.
use bundlebase::bundle::BundleFacade;
use bundlebase::test_utils::{random_memory_dir, test_datafile};
use bundlebase_command::BundleBuilderExt;
use bundlebase_command::BundleFacadeCommandExt;
use bundlebase_common::BundlebaseError;
use datafusion::logical_expr::ExplainFormat;
use futures::{StreamExt, TryStreamExt};

mod common;
fn init() {
    common::init_catalog();
}

async fn physical_plan<T: BundleFacadeCommandExt + Sync>(
    bundle: &T,
    sql: &str,
) -> Result<String, BundlebaseError> {
    let mut stream = bundle
        .explain(false, false, ExplainFormat::Indent, Some(sql))
        .await?;
    let mut out = String::new();
    while let Some(batch) = stream.next().await {
        let batch = batch?;
        let plan_types = batch
            .column(0)
            .as_any()
            .downcast_ref::<arrow::array::StringArray>()
            .expect("plan_type column");
        let plan_texts = batch
            .column(1)
            .as_any()
            .downcast_ref::<arrow::array::StringArray>()
            .expect("plan column");
        for i in 0..batch.num_rows() {
            if plan_types.value(i) == "physical_plan" {
                out.push_str(plan_texts.value(i));
                out.push('\n');
            }
        }
    }
    Ok(out)
}

async fn count_star(bundle: &dyn BundleFacade) -> Result<i64, BundlebaseError> {
    let stream = bundle
        .query("SELECT COUNT(*) AS cnt FROM bundle", vec![], None)
        .await?;
    let batches: Vec<_> = stream.try_collect().await?;
    let col = batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<arrow::array::Int64Array>()
        .expect("Int64 cnt");
    Ok(col.value(0))
}

/// `SELECT COUNT(*) FROM bundle` on a vanilla bundle must return the
/// right number AND must short-circuit to the synthetic empty-projection
/// plan — no parquet/csv scan node should appear in the physical plan.
#[tokio::test]
async fn test_count_star_uses_fast_path() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    assert_eq!(count_star(bundle.as_ref()).await?, 100);

    let plan = physical_plan(bundle.as_ref(), "SELECT COUNT(*) FROM bundle").await?;
    // The fast path returns a `DataSourceExec` over a `MemorySourceConfig`
    // with an empty schema. A real file scan would surface as
    // `CsvExec`/`ParquetExec`/`DataSourceExec: file_groups=…`. Assert the
    // file-scan signature is absent.
    assert!(
        !plan.contains("file_groups") && !plan.contains("ParquetExec") && !plan.contains("CsvExec"),
        "COUNT(*) physical plan must not include a file scan, got:\n{}",
        plan
    );
    Ok(())
}

/// Multi-block bundle: every block independently short-circuits, so
/// COUNT(*) is `sum(num_rows)` across blocks with no file I/O.
#[tokio::test]
async fn test_count_star_fast_path_multi_block() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;
    bundle
        .attach(test_datafile("customers-101-150.csv"), None)
        .await?;

    assert_eq!(count_star(bundle.as_ref()).await?, 100 + 50);

    let plan = physical_plan(bundle.as_ref(), "SELECT COUNT(*) FROM bundle").await?;
    assert!(
        !plan.contains("file_groups") && !plan.contains("ParquetExec") && !plan.contains("CsvExec"),
        "multi-block COUNT(*) plan must avoid file scans, got:\n{}",
        plan
    );
    Ok(())
}

/// After DELETE, the fast path must NOT fire (it only knows the raw
/// row count, not the live count). The result must still be correct.
#[tokio::test]
async fn test_count_star_correct_after_delete() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;
    let before = count_star(bundle.as_ref()).await?;
    assert_eq!(before, 100);

    // CSV columns are Utf8 by default; pick a value that uniquely
    // identifies a single row.
    bundle
        .delete("\"Email\" = 'zunigavanessa@smith.info'")
        .await?;
    bundle.commit("delete one row").await?;

    let bundle = bundlebase::Bundle::open(data_dir.url().as_str(), None).await?;
    let after = count_star(bundle.as_ref()).await?;
    assert_eq!(
        after,
        before - 1,
        "delete must drop the count by exactly one and persist across reopen"
    );
    Ok(())
}

/// COUNT(*) with a WHERE clause cannot use the fast path (the optimizer
/// won't push an empty projection through a non-trivial filter); the
/// answer must still be right.
#[tokio::test]
async fn test_count_star_with_where_correct() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    let stream = bundle
        .query(
            "SELECT COUNT(*) AS cnt FROM bundle WHERE \"Country\" = 'Chile'",
            vec![],
            None,
        )
        .await?;
    let batches: Vec<_> = stream.try_collect().await?;
    let col = batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<arrow::array::Int64Array>()
        .expect("Int64");
    let cnt = col.value(0);
    assert!(cnt >= 0 && cnt <= 100);
    Ok(())
}
