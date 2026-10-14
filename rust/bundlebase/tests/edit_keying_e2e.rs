//! DELETE/UPDATE edit state must stay attached to the block it was computed
//! against (bb-2jp).
//!
//! Tombstones and update overlays are keyed by the stable `(BlockId, version)`
//! of the block — never by the block's position in the base pack, which
//! shifts when a block is DETACHed or REPLACEd (in-session, or by a
//! concurrent writer whose commit is replayed first).

use bundlebase::bundle::BundleFacade;
use bundlebase::test_utils::{random_memory_dir, test_datafile};
use bundlebase_command::BundleBuilderExt;
use bundlebase_common::BundlebaseError;
use futures::TryStreamExt;

mod common;

fn init() {
    common::init_catalog();
}

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

/// City of the row with the given customer Index (None if absent/null).
async fn city_of(facade: &dyn BundleFacade, index: i64) -> Option<String> {
    use arrow::array::{Array, StringArray};
    let batches = query_collect(
        facade,
        &format!("SELECT \"City\" FROM bundle WHERE \"Index\" = {index}"),
    )
    .await;
    let batch = batches.iter().find(|b| b.num_rows() > 0)?;
    let col = batch.column(0);
    let col = arrow::compute::cast(col, &arrow::datatypes::DataType::Utf8).ok()?;
    let arr = col.as_any().downcast_ref::<StringArray>()?;
    if arr.is_null(0) {
        None
    } else {
        Some(arr.value(0).to_string())
    }
}

const FIRST: &str = "customers-0-100.csv"; // Index 1..=100
const SECOND: &str = "customers-101-150.csv"; // Index 101..=150

/// Two-block bundle (100 + 50 rows), committed.
async fn two_block_bundle(
    data_dir: &str,
) -> Result<std::sync::Arc<bundlebase::BundleBuilder>, BundlebaseError> {
    let builder = bundlebase::BundleBuilder::create(data_dir, None).await?;
    builder.attach(test_datafile(FIRST), None).await?;
    builder.attach(test_datafile(SECOND), None).await?;
    builder.commit("two blocks").await?;
    Ok(builder)
}

/// DELETE in the second block, then DETACH the first block in the same
/// session. The commit-time tombstone is recorded after the DETACH, so a
/// positional key would point past the end of the (now one-block) pack.
#[tokio::test]
async fn test_delete_then_detach_earlier_block_keeps_tombstone() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let url = data_dir.url().to_string();
    let builder = two_block_bundle(&url).await?;

    assert_eq!(builder.delete("\"Index\" = 120").await?, 1);
    builder.detach_block(test_datafile(FIRST)).await?;
    assert_eq!(
        query_count(builder.as_ref(), "SELECT COUNT(*) FROM bundle").await,
        49
    );
    builder.commit("delete then detach").await?;

    let reopened = bundlebase::Bundle::open(&url, None).await?;
    assert_eq!(
        query_count(
            reopened.as_ref(),
            "SELECT COUNT(*) FROM bundle WHERE \"Index\" = 120"
        )
        .await,
        0,
        "the tombstone must follow its block across a DETACH of an earlier block"
    );
    assert_eq!(
        query_count(reopened.as_ref(), "SELECT COUNT(*) FROM bundle").await,
        49
    );
    Ok(())
}

/// UPDATE in the second block, then DETACH the first block in the same
/// session: the overlay must still land on the second block after reopen.
#[tokio::test]
async fn test_update_then_detach_earlier_block_keeps_overlay() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let url = data_dir.url().to_string();
    let builder = two_block_bundle(&url).await?;

    builder
        .update("SET City = 'MOVED_CITY' WHERE \"Index\" = 120")
        .await?;
    builder.detach_block(test_datafile(FIRST)).await?;
    builder.commit("update then detach").await?;

    let reopened = bundlebase::Bundle::open(&url, None).await?;
    assert_eq!(
        city_of(reopened.as_ref(), 120).await.as_deref(),
        Some("MOVED_CITY"),
        "the overlay must follow its block across a DETACH of an earlier block"
    );
    assert_eq!(
        query_count(
            reopened.as_ref(),
            "SELECT COUNT(*) FROM bundle WHERE City = 'MOVED_CITY'"
        )
        .await,
        1
    );
    Ok(())
}

/// Two writers start from the same committed state. Writer B detaches the
/// first block and commits; writer A (unaware) deletes a row in the second
/// block and commits after it. Replay applies B's DETACH before A's
/// tombstone, so a positional key computed in A's two-block view is wrong.
#[tokio::test]
async fn test_concurrent_detach_does_not_shift_tombstones() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let url = data_dir.url().to_string();
    two_block_bundle(&url).await?;

    let writer_a = bundlebase::Bundle::open(&url, None)
        .await?
        .extend(None)
        .await?;
    let writer_b = bundlebase::Bundle::open(&url, None)
        .await?
        .extend(None)
        .await?;

    writer_b.detach_block(test_datafile(FIRST)).await?;
    writer_b.commit("B detaches the first block").await?;

    assert_eq!(writer_a.delete("\"Index\" = 120").await?, 1);
    writer_a.commit("A deletes a second-block row").await?;

    let reopened = bundlebase::Bundle::open(&url, None).await?;
    assert_eq!(
        query_count(
            reopened.as_ref(),
            "SELECT COUNT(*) FROM bundle WHERE \"Index\" = 120"
        )
        .await,
        0,
        "A's tombstone must apply to the block it was computed against"
    );
    assert_eq!(
        query_count(reopened.as_ref(), "SELECT COUNT(*) FROM bundle").await,
        49
    );
    Ok(())
}

/// REPLACE semantics: edits are keyed by the block's (id, version). A
/// REPLACE gives the block a new version whose row numbers can't be assumed
/// to line up with the old content, so earlier edits on that block (committed
/// or still pending) are discarded rather than misapplied to the new data.
#[tokio::test]
async fn test_replace_discards_pending_edits_on_replaced_block() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let url = data_dir.url().to_string();
    let builder = two_block_bundle(&url).await?;

    let tmp = tempfile::tempdir().expect("tempdir");
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test_data")
        .join(SECOND);
    let copy = tmp.path().join("replacement.csv");
    std::fs::copy(&src, &copy).expect("copy fixture");
    let copy_url = format!("file://{}", copy.to_str().expect("utf8 path"));

    assert_eq!(builder.delete("\"Index\" = 120").await?, 1);
    builder
        .update("SET City = 'STALE_CITY' WHERE \"Index\" = 121")
        .await?;
    builder
        .replace_block(test_datafile(SECOND), &copy_url)
        .await?;

    // In-session: the replaced block carries no edits.
    assert_eq!(
        query_count(builder.as_ref(), "SELECT COUNT(*) FROM bundle").await,
        150
    );
    assert_ne!(
        city_of(builder.as_ref(), 121).await.as_deref(),
        Some("STALE_CITY")
    );

    builder.commit("edits then replace").await?;
    let reopened = bundlebase::Bundle::open(&url, None).await?;
    assert_eq!(
        query_count(reopened.as_ref(), "SELECT COUNT(*) FROM bundle").await,
        150,
        "edits computed against the old version must not apply to the replacement"
    );
    assert_ne!(
        city_of(reopened.as_ref(), 121).await.as_deref(),
        Some("STALE_CITY")
    );
    Ok(())
}

/// UPDATE must not match rows that are already deleted (tombstoned).
#[tokio::test]
async fn test_update_skips_deleted_rows() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let url = data_dir.url().to_string();
    let builder = two_block_bundle(&url).await?;

    assert_eq!(builder.delete("\"Index\" = 1").await?, 1);
    let city = builder.translate_sql("City");
    let wc = builder.translate_sql("\"Index\" = 1");
    let n = builder
        .run_command("update deleted row".to_string(), async {
            let n = builder
                .evaluate_update_cols(
                    std::slice::from_ref(&city),
                    &["'GHOST'".to_string()],
                    &wc,
                )
                .await?;
            builder.flush_pending_updates_to_blocks()?;
            Ok(n)
        })
        .await?;
    assert_eq!(n, 0, "UPDATE must not report/update a deleted row");

    builder.commit("delete then update").await?;
    let reopened = bundlebase::Bundle::open(&url, None).await?;
    assert_eq!(
        query_count(reopened.as_ref(), "SELECT COUNT(*) FROM bundle").await,
        149
    );
    Ok(())
}

/// Uncommitted DELETE/UPDATE on a builder made by extend() must not leak into
/// the source Bundle's queries.
#[tokio::test]
async fn test_extend_does_not_leak_uncommitted_edits() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let url = data_dir.url().to_string();
    two_block_bundle(&url).await?;

    let original = bundlebase::Bundle::open(&url, None).await?;
    let original_city = city_of(original.as_ref(), 5).await;
    let builder = original.extend(None).await?;

    assert_eq!(builder.delete("\"Index\" = 1").await?, 1);
    builder
        .update("SET City = 'LEAKED_CITY' WHERE \"Index\" = 5")
        .await?;

    // The builder sees its edits...
    assert_eq!(
        query_count(builder.as_ref(), "SELECT COUNT(*) FROM bundle").await,
        149
    );
    assert_eq!(
        city_of(builder.as_ref(), 5).await.as_deref(),
        Some("LEAKED_CITY")
    );

    // ...the original Bundle does not.
    assert_eq!(
        query_count(original.as_ref(), "SELECT COUNT(*) FROM bundle").await,
        150,
        "uncommitted DELETE leaked into the source Bundle"
    );
    assert_eq!(
        city_of(original.as_ref(), 5).await,
        original_city,
        "uncommitted UPDATE leaked into the source Bundle"
    );
    Ok(())
}

/// bb-8lk: undo after several edits restores the exact prior values —
/// overwritten update cells go back to their earlier UPDATE's value and
/// rows deleted by the undone changes come back.
#[tokio::test]
async fn test_undo_after_several_edits_restores_prior_values() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let url = data_dir.url().to_string();
    let builder = two_block_bundle(&url).await?;

    builder
        .update("SET City = 'FIRST' WHERE \"Index\" = 2 OR \"Index\" = 3")
        .await?;
    builder.delete("\"Index\" = 10").await?;
    builder
        .update("SET City = 'SECOND' WHERE \"Index\" = 2 OR \"Index\" = 4")
        .await?;
    builder.delete("\"Index\" = 11 OR \"Index\" = 120").await?;

    builder.undo().await?;
    builder.undo().await?;

    assert_eq!(city_of(builder.as_ref(), 2).await.as_deref(), Some("FIRST"));
    assert_eq!(city_of(builder.as_ref(), 3).await.as_deref(), Some("FIRST"));
    assert_ne!(city_of(builder.as_ref(), 4).await.as_deref(), Some("SECOND"));
    assert_eq!(
        query_count(builder.as_ref(), "SELECT COUNT(*) FROM bundle").await,
        149,
        "only the first DELETE remains"
    );

    builder.commit("after undo").await?;
    let reopened = bundlebase::Bundle::open(&url, None).await?;
    assert_eq!(city_of(reopened.as_ref(), 2).await.as_deref(), Some("FIRST"));
    assert_ne!(city_of(reopened.as_ref(), 4).await.as_deref(), Some("SECOND"));
    assert_eq!(
        query_count(reopened.as_ref(), "SELECT COUNT(*) FROM bundle").await,
        149
    );
    Ok(())
}
