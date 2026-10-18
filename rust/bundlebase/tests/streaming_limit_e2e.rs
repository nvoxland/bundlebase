//! Regression: the streaming full-scan path must not push a query LIMIT into
//! the reader below the delete filter (bb-ekq). The reader would stop after
//! `lim` RAW rows; the tombstone filter above then drops deleted rows and the
//! query silently returns fewer rows than survive.
//!
//! This lives in its own test binary because it forces the streaming path by
//! shrinking the global block cache via `BUNDLEBASE_BLOCK_CACHE_BYTES`, which
//! is read once at first cache use.

use bundlebase::bundle::BundleFacade;
use bundlebase::test_utils::{random_memory_dir, test_datafile};
use bundlebase_command::BundleBuilderExt;
use bundlebase_common::BundlebaseError;
use futures::TryStreamExt;

mod common;

#[tokio::test]
async fn test_limit_after_deletes_on_streaming_scan() -> Result<(), BundlebaseError> {
    // Must run before anything touches GLOBAL_BLOCK_CACHE: a 1-byte budget
    // rejects every block, forcing the constant-memory streaming scan path.
    std::env::set_var("BUNDLEBASE_BLOCK_CACHE_BYTES", "1");
    common::init_catalog();

    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    // An UPDATE creates an overlay so the streaming path takes the
    // overlay+tombstone wrapping branch; the DELETE tombstones the first
    // raw rows, which is what a pushed-down LIMIT would wrongly count.
    builder.update("SET first_name = 'X' WHERE id = 7").await?;
    builder.delete("id <= 5").await?;

    let stream = builder
        .query("SELECT id FROM bundle LIMIT 10", vec![], None)
        .await?;
    let batches: Vec<arrow::array::RecordBatch> = stream.try_collect().await?;
    let rows: usize = batches.iter().map(|b| b.num_rows()).sum();

    assert_eq!(
        rows, 10,
        "LIMIT 10 must return 10 surviving rows (995 remain after deleting 5); \
         a short result means the limit was applied to raw rows below the \
         delete filter"
    );
    Ok(())
}
