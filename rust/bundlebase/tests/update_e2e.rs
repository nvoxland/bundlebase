use bundlebase::bundle::BundleFacade;
use bundlebase::test_utils::{random_memory_dir, test_datafile};
use bundlebase_command::BundleBuilderExt;
use bundlebase_common::BundlebaseError;
use futures::TryStreamExt;

mod common;

fn init() {
    common::init_catalog();
}

/// Helper: run a SQL query and collect all batches
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

/// Helper: run a COUNT query and return the count
async fn query_count(facade: &dyn BundleFacade, sql: &str) -> i64 {
    let batches = query_collect(facade, sql).await;
    assert!(!batches.is_empty(), "query `{}` returned no batches", sql);
    let col = batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<arrow::array::Int64Array>()
        .unwrap_or_else(|| panic!("query `{}`: expected Int64 count column", sql));
    col.value(0)
}

#[tokio::test]
async fn test_update_basic() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    builder
        .update("SET salary = 999 WHERE salary > 200000")
        .await?;

    let cnt = query_count(
        builder.as_ref(),
        "SELECT COUNT(*) as cnt FROM bundle WHERE salary = 999",
    )
    .await;
    assert!(cnt > 0, "Expected updated rows with salary=999");

    let cnt_old = query_count(
        builder.as_ref(),
        "SELECT COUNT(*) as cnt FROM bundle WHERE salary > 200000 AND salary != 999",
    )
    .await;
    assert_eq!(cnt_old, 0, "No rows should have salary > 200000 except 999");

    Ok(())
}

#[tokio::test]
async fn test_update_expression() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    let min_before = {
        let batches = query_collect(
            builder.as_ref(),
            "SELECT MIN(salary) as m FROM bundle WHERE salary > 0",
        )
        .await;
        batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<arrow::array::Float64Array>()
            .unwrap()
            .value(0)
    };

    builder
        .update("SET salary = salary * 2 WHERE salary > 0")
        .await?;

    let min_after = {
        let batches = query_collect(
            builder.as_ref(),
            "SELECT MIN(salary) as m FROM bundle WHERE salary > 0",
        )
        .await;
        batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<arrow::array::Float64Array>()
            .unwrap()
            .value(0)
    };

    assert!(
        min_after >= min_before * 1.9,
        "Min salary should roughly double"
    );
    Ok(())
}

#[tokio::test]
async fn test_update_to_null() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    let titles_before =
        query_count(builder.as_ref(), "SELECT COUNT(title) as cnt FROM bundle").await;

    builder
        .update("SET title = NULL WHERE salary > 200000")
        .await?;

    let titles_after =
        query_count(builder.as_ref(), "SELECT COUNT(title) as cnt FROM bundle").await;
    assert!(
        titles_after < titles_before,
        "Some titles should be NULL now"
    );
    Ok(())
}

#[tokio::test]
async fn test_update_multiple_columns() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    builder
        .update("SET first_name = 'UPDATED', last_name = 'USER' WHERE salary > 200000")
        .await?;

    let cnt = query_count(
        builder.as_ref(),
        "SELECT COUNT(*) as cnt FROM bundle WHERE first_name = 'UPDATED' AND last_name = 'USER'",
    )
    .await;
    assert!(cnt > 0, "Expected rows with updated names");
    Ok(())
}

#[tokio::test]
async fn test_update_preserves_unmodified() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    // Get original first_name for id=1
    let batches = query_collect(
        builder.as_ref(),
        "SELECT first_name FROM bundle WHERE id = 1",
    )
    .await;
    let original_name = batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<arrow::array::StringViewArray>()
        .unwrap()
        .value(0)
        .to_string();

    // Update only salary
    builder.update("SET salary = 999 WHERE id = 1").await?;

    let batches = query_collect(
        builder.as_ref(),
        "SELECT first_name, salary FROM bundle WHERE id = 1",
    )
    .await;
    let name_after = batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<arrow::array::StringViewArray>()
        .unwrap()
        .value(0)
        .to_string();

    assert_eq!(name_after, original_name, "first_name should be unchanged");
    Ok(())
}

#[tokio::test]
async fn test_update_no_match() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    let count_before = builder.num_rows().await?;
    builder
        .update("SET salary = 0 WHERE salary > 99999999")
        .await?;
    let count_after = builder.num_rows().await?;

    assert_eq!(count_before, count_after);
    Ok(())
}

#[tokio::test]
async fn test_update_commit_reopen() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    builder
        .update("SET salary = 999 WHERE salary > 200000")
        .await?;

    let cnt_before = query_count(
        builder.as_ref(),
        "SELECT COUNT(*) as cnt FROM bundle WHERE salary = 999",
    )
    .await;
    assert!(cnt_before > 0);

    builder.commit("Updated salaries").await?;

    // Reopen
    let bundle = bundlebase::Bundle::open(data_dir.url().as_str(), None).await?;
    let cnt_after = query_count(
        bundle.as_ref(),
        "SELECT COUNT(*) as cnt FROM bundle WHERE salary = 999",
    )
    .await;
    assert_eq!(cnt_before, cnt_after, "Update should persist after reopen");

    Ok(())
}

#[tokio::test]
async fn test_update_same_row_twice() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    // First update sets salary to 100
    builder.update("SET salary = 100 WHERE id = 1").await?;
    // Second update sets salary to 200 (should overwrite)
    builder.update("SET salary = 200 WHERE id = 1").await?;

    let batches = query_collect(builder.as_ref(), "SELECT salary FROM bundle WHERE id = 1").await;
    let salary = batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<arrow::array::Float64Array>()
        .unwrap()
        .value(0);
    assert_eq!(salary, 200.0, "Second update should win");
    Ok(())
}

#[tokio::test]
async fn test_delete_then_update() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    let initial_count = builder.num_rows().await?;

    builder.delete("salary > 200000").await?;
    let after_delete = builder.num_rows().await?;
    assert!(after_delete < initial_count);

    builder
        .update("SET salary = 0 WHERE salary < 50000")
        .await?;

    let cnt = query_count(
        builder.as_ref(),
        "SELECT COUNT(*) as cnt FROM bundle WHERE salary = 0",
    )
    .await;
    assert!(cnt > 0, "Update after delete should work");
    Ok(())
}

#[tokio::test]
async fn test_update_multiple_commits() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    // First commit: set salary to 100 for id=1
    builder.update("SET salary = 100 WHERE id = 1").await?;
    builder.commit("First update").await?;

    // Second commit: set salary to 200 for id=1 (latest wins)
    builder.update("SET salary = 200 WHERE id = 1").await?;
    builder.commit("Second update").await?;

    // Reopen and verify latest wins
    let bundle = bundlebase::Bundle::open(data_dir.url().as_str(), None).await?;
    let batches = query_collect(bundle.as_ref(), "SELECT salary FROM bundle WHERE id = 1").await;
    let salary = batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<arrow::array::Float64Array>()
        .unwrap()
        .value(0);
    assert_eq!(salary, 200.0, "Second commit's update should win");

    Ok(())
}

#[tokio::test]
async fn test_update_then_filter_in_session() -> Result<(), BundlebaseError> {
    // Update then filter: the CASE WHEN FilterOp should transform values at DataFrame level
    init();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    builder
        .update("SET salary = 99999 WHERE salary > 200000")
        .await?;

    let cnt_before = query_count(
        builder.as_ref(),
        "SELECT COUNT(*) as cnt FROM bundle WHERE salary = 99999",
    )
    .await;
    assert!(cnt_before > 0, "Update should be visible");

    // Filter keeps salary >= 50000 — 99999 passes this filter
    builder
        .filter("SELECT * FROM bundle WHERE salary >= 50000", vec![])
        .await?;

    let cnt_after = query_count(
        builder.as_ref(),
        "SELECT COUNT(*) as cnt FROM bundle WHERE salary = 99999",
    )
    .await;
    assert_eq!(
        cnt_after, cnt_before,
        "Updated rows (99999) should survive filter (>= 50000)"
    );

    Ok(())
}

#[tokio::test]
async fn test_update_then_delete_in_session() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    builder
        .update("SET salary = 99999 WHERE salary > 200000")
        .await?;
    let cnt_updated = query_count(
        builder.as_ref(),
        "SELECT COUNT(*) as cnt FROM bundle WHERE salary = 99999",
    )
    .await;
    assert!(cnt_updated > 0);

    // Delete low salaries — should not affect updated rows (99999 > 50000)
    builder.delete("salary < 50000").await?;

    let cnt_after = query_count(
        builder.as_ref(),
        "SELECT COUNT(*) as cnt FROM bundle WHERE salary = 99999",
    )
    .await;
    assert_eq!(
        cnt_after, cnt_updated,
        "Updated rows should survive delete of low salaries"
    );

    Ok(())
}

#[tokio::test]
async fn test_update_survives_rename() -> Result<(), BundlebaseError> {
    // Rename is a schema-only op (no DataFrame-level data transform),
    // so it clears the cache but the overlay should still apply.
    init();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    builder
        .update("SET salary = 999 WHERE salary > 200000")
        .await?;
    let cnt1 = query_count(
        builder.as_ref(),
        "SELECT COUNT(*) as cnt FROM bundle WHERE salary = 999",
    )
    .await;
    assert!(cnt1 > 0);

    // Rename a different column — forces dataframe rebuild
    builder.rename_column("first_name", "fname").await?;

    let cnt2 = query_count(
        builder.as_ref(),
        "SELECT COUNT(*) as cnt FROM bundle WHERE salary = 999",
    )
    .await;
    assert_eq!(cnt2, cnt1, "Updated rows should survive rename");

    Ok(())
}

#[tokio::test]
async fn test_update_csv() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    builder
        .update("SET City = 'UPDATED' WHERE Index > 90")
        .await?;

    let cnt = query_count(
        builder.as_ref(),
        "SELECT COUNT(*) as cnt FROM bundle WHERE City = 'UPDATED'",
    )
    .await;
    assert!(cnt > 0, "CSV update should work");
    Ok(())
}

/// DELETE → COMMIT → REOPEN must persist the row exclusion via the
/// tombstone file path: `commit()` writes a `.tomb`, the manifest
/// records a `DeleteOp`, and on `Bundle::open` the op replays
/// `add_deleted_rows` onto each `DataBlock` so `DataBlock::scan`'s
/// `DeletedRowFilterDataSource` keeps them out of every query.
#[tokio::test]
async fn test_delete_persists_across_reopen() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    let initial = builder.num_rows().await?;

    builder.delete("salary > 200000").await?;
    let after_session = builder.num_rows().await?;
    assert!(
        after_session < initial,
        "delete should reduce row count in-session"
    );
    builder.commit("delete high salaries").await?;

    // Reopen and verify deletes still apply (the FilterOp from the
    // session is also persisted, but we want the assertion to fail if
    // either side stops working — so probe a query whose answer can
    // only come out right when the tombstone is consulted at scan).
    let bundle = bundlebase::Bundle::open(data_dir.url().as_str(), None).await?;
    assert_eq!(
        bundle.num_rows().await?,
        after_session,
        "row count must match across reopen"
    );
    let leaked = query_count(
        bundle.as_ref(),
        "SELECT COUNT(*) as cnt FROM bundle WHERE salary > 200000",
    )
    .await;
    assert_eq!(
        leaked, 0,
        "tombstoned rows must not surface in queries after reopen"
    );

    // Manifest sanity-check: exactly one delete op recorded with the
    // original where clause and a non-empty tombstone filename.
    let (yaml, _commit, _url) = common::latest_commit(data_dir.as_ref())
        .await?
        .expect("commit must be persisted");
    assert!(
        yaml.contains("type: delete"),
        "manifest should contain a delete op:\n{}",
        yaml
    );
    assert!(
        yaml.contains("salary > 200000"),
        "delete op should echo its where clause:\n{}",
        yaml
    );
    assert!(
        yaml.contains(".tomb"),
        "delete op should reference a tombstone file:\n{}",
        yaml
    );

    Ok(())
}

/// Sequential DELETEs across separate commits must accumulate — every
/// reopen replays each `DeleteOp::apply` in order and `add_deleted_rows`
/// on `DataBlock` is additive (not a replacement). Catches a regression
/// where the second commit's tombstone would clobber the first.
#[tokio::test]
async fn test_delete_across_multiple_commits() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    let initial = builder.num_rows().await?;

    builder.delete("salary > 200000").await?;
    let after_first = builder.num_rows().await?;
    assert!(after_first < initial);
    builder.commit("delete high salaries").await?;

    // Reopen and run the second delete from a fresh builder so we're
    // exercising the persistence path, not just the in-session FilterOp.
    let opened = bundlebase::Bundle::open(data_dir.url().as_str(), None).await?;
    let builder2 = opened.extend(None).await?;
    let reopened_count = builder2.num_rows().await?;
    assert_eq!(reopened_count, after_first, "first delete must persist");

    builder2.delete("salary < 50000").await?;
    let after_second = builder2.num_rows().await?;
    assert!(after_second < after_first);
    builder2.commit("delete low salaries").await?;

    // Reopen one more time and assert both populations are gone.
    let final_bundle = bundlebase::Bundle::open(data_dir.url().as_str(), None).await?;
    assert_eq!(
        final_bundle.num_rows().await?,
        after_second,
        "both deletes should still apply after second reopen"
    );
    let high = query_count(
        final_bundle.as_ref(),
        "SELECT COUNT(*) as cnt FROM bundle WHERE salary > 200000",
    )
    .await;
    let low = query_count(
        final_bundle.as_ref(),
        "SELECT COUNT(*) as cnt FROM bundle WHERE salary < 50000",
    )
    .await;
    assert_eq!(high, 0, "first commit's deletes must survive");
    assert_eq!(low, 0, "second commit's deletes must survive");

    Ok(())
}

#[tokio::test]
async fn test_update_after_rename() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let builder = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;

    builder.rename_column("salary", "pay").await?;
    builder.update("SET pay = 999 WHERE pay > 200000").await?;

    let cnt = query_count(
        builder.as_ref(),
        "SELECT COUNT(*) as cnt FROM bundle WHERE pay = 999",
    )
    .await;
    assert!(cnt > 0, "Update after rename should work");
    Ok(())
}
