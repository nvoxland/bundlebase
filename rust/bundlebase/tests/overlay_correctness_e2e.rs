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

    // Confirm the index fast-path is actually exercised (otherwise this test
    // wouldn't guard the index path at all). The index lookup wraps its source
    // in SchemaRenameDataSource.
    let plan = physical_plan(&reopened, &sql).await;
    assert!(
        plan.contains("SchemaRenameDataSource"),
        "expected the indexed-lookup fast-path in the plan, got:\n{plan}"
    );

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
    let persisted = query_count(reopened.as_ref(), "SELECT COUNT(*) AS cnt FROM bundle").await;
    assert_eq!(
        persisted, expected as i64,
        "an undone DELETE must not be persisted at the next commit \
         (pending_deletes not reconciled by undo)"
    );

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
