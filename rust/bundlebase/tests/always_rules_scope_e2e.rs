//! ATTACH must re-apply always-rules to the NEWLY attached block(s) only,
//! not to every existing block (bb-c6l). Re-running a non-idempotent
//! `SET x = x + k` rule against blocks it already processed corrupts their
//! values on every attach; re-running always-delete re-tombstones
//! already-deleted rows.

use bundlebase::bundle::BundleFacade;
use bundlebase::test_utils::{random_memory_dir, test_datafile};
use bundlebase_command::BundleBuilderExt;
use bundlebase_common::BundlebaseError;
use futures::TryStreamExt;

mod common;

async fn scalar_f64(facade: &dyn BundleFacade, sql: &str) -> f64 {
    let stream = facade.query(sql, vec![], None).await.expect("query");
    let batches: Vec<arrow::record_batch::RecordBatch> =
        stream.try_collect().await.expect("collect");
    let batch = batches.iter().find(|b| b.num_rows() > 0).expect("one row");
    batch
        .column(0)
        .as_any()
        .downcast_ref::<arrow::array::Float64Array>()
        .expect("f64")
        .value(0)
}

async fn scalar_i64(facade: &dyn BundleFacade, sql: &str) -> i64 {
    let stream = facade.query(sql, vec![], None).await.expect("query");
    let batches: Vec<arrow::record_batch::RecordBatch> =
        stream.try_collect().await.expect("collect");
    let batch = batches.iter().find(|b| b.num_rows() > 0).expect("one row");
    batch
        .column(0)
        .as_any()
        .downcast_ref::<arrow::array::Int64Array>()
        .expect("i64")
        .value(0)
}

/// A non-idempotent always-update rule must apply exactly once per row: the
/// newly attached block is updated, and blocks updated on earlier attaches
/// are NOT re-incremented.
#[tokio::test]
async fn test_always_update_not_reapplied_to_existing_blocks() -> Result<(), BundlebaseError> {
    common::init_catalog();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    // userdata.parquet: id=1 has salary 49756.53.
    let base_salary = 49756.53_f64;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    // Rule created after attach → applies to the existing block once.
    builder
        .always_update("SET salary = salary + 1000 WHERE id = 1")
        .await?;
    let after_rule = scalar_f64(&*builder, "SELECT salary FROM bundle WHERE id = 1").await;
    assert!(
        (after_rule - (base_salary + 1000.0)).abs() < 0.01,
        "rule must apply once on creation, got {after_rule}"
    );

    // Second attach adds a new block; the rule applies to IT, not to the
    // already-updated first block.
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    let distinct = scalar_i64(
        &*builder,
        "SELECT COUNT(DISTINCT salary) AS n FROM bundle WHERE id = 1",
    )
    .await;
    assert_eq!(
        distinct, 1,
        "both id=1 rows must share one salary; a second increment on the \
         first block (bb-c6l) would produce two distinct values"
    );

    let min = scalar_f64(
        &*builder,
        "SELECT MIN(salary) AS s FROM bundle WHERE id = 1",
    )
    .await;
    assert!(
        (min - (base_salary + 1000.0)).abs() < 0.01,
        "salary must be base + 1000 (applied once), got {min}"
    );

    Ok(())
}

/// Always-delete still applies to newly attached data (guards against
/// over-scoping the fix to nothing).
#[tokio::test]
async fn test_always_delete_applies_to_new_blocks() -> Result<(), BundlebaseError> {
    common::init_catalog();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    builder.always_delete("id = 1").await?;
    assert_eq!(
        scalar_i64(&*builder, "SELECT COUNT(*) AS n FROM bundle WHERE id = 1").await,
        0,
        "rule deletes the matching row on creation"
    );

    // New block's id=1 row must also be deleted by the persistent rule.
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    assert_eq!(
        scalar_i64(&*builder, "SELECT COUNT(*) AS n FROM bundle WHERE id = 1").await,
        0,
        "always-delete must re-apply to newly attached data"
    );

    Ok(())
}
