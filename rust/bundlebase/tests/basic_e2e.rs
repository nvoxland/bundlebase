use bundlebase;
use bundlebase::bundle::{BundleFacade, INIT_FILENAME, META_DIR};
use bundlebase::test_utils::{random_memory_dir, random_memory_url, test_datafile};
use bundlebase::{op_field, AnyOperation, BundleConfig};
use bundlebase::{test_utils, Bundle};
use bundlebase_command::BundleBuilderExt;
use bundlebase_common::{BundlebaseError, ConfigProvider};
use bundlebase_io::{readable_file_from_path, readable_file_from_url};
use std::sync::Arc;
use url::Url;

mod common;

fn init() {
    common::init_catalog();
}

#[tokio::test]
async fn test_basic_e2e() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await?
        .drop_column("title")
        .await?
        .rename_column("first_name", "name")
        .await?;
    let version = readable_file_from_url(
        &Url::parse(test_datafile("userdata.parquet"))?,
        Arc::new(BundleConfig::new(None)?) as Arc<dyn ConfigProvider>,
    )
    .await?
    .version()
    .await?;

    bundle.commit("First commit").await?;

    let init_content = bundle
        .data_dir()
        .file(&format!("{}/{}", META_DIR, INIT_FILENAME))?
        .read_str()
        .await?
        .expect("init commit doesn't exist");
    let fmt_version = bundlebase_common::format_version_string();
    assert_eq!(
        init_content.trim(),
        format!(
            "id: {}\nminVersion: '{}'\nmaxVersion: '{}'",
            bundle.bundle().id(),
            fmt_version,
            fmt_version
        )
        .trim()
    );

    // Find and read the versioned manifest file
    let (contents, commit, url) = common::latest_commit(bundle.data_dir().as_ref())
        .await?
        .unwrap();

    let expected = format!(
        r#"author: {}
message: First commit
timestamp: {}
changes:
- id: {}
  description: {}
  operations:
  - type: attachBlock
    id: {}
    pack: {}
    location: memory:///test_data/userdata.parquet
    format: parquet
    version: {}
    hash: 8c26edb7f30d7694a1431224f28e5932
    numRows: 1000
    bytes: 113629
    schema: 3b/a5bd5f9d91f9d1.block.schema.yaml
- id: {}
  description: DROP COLUMN title
  operations:
  - type: dropColumn
- id: {}
  description: RENAME COLUMN first_name TO name
  operations:
  - type: renameColumn
    newName: name
"#,
        commit.author,
        commit.timestamp,
        commit.changes[0].id,
        commit.changes[0].description,
        test_utils::for_yaml(String::from(op_field!(
            &commit.operations()[0],
            AnyOperation::AttachBlock,
            id
        ))),
        test_utils::for_yaml(String::from(op_field!(
            &commit.operations()[0],
            AnyOperation::AttachBlock,
            pack
        ))),
        test_utils::for_yaml(version),
        commit.changes[1].id,
        commit.changes[2].id,
    );
    assert_eq!(common::strip_column_ids(&contents), expected);

    // Verify column IDs are present in the actual serialized output
    assert!(
        contents.contains("columnIds:"),
        "AttachBlock should have columnIds"
    );
    assert!(
        contents.contains("type: dropColumn\n    id:"),
        "DropColumn should have id"
    );
    assert!(
        contents.contains("type: renameColumn\n    id:"),
        "RenameColumn should have id"
    );

    // Open the saved bundle
    let loaded_bundle = Bundle::open(data_dir.url().as_str(), None).await?;

    assert_eq!(loaded_bundle.history().len(), 1);
    assert_eq!(loaded_bundle.history().get(0).unwrap().url, Some(url));

    // Verify data can be queried
    let df = loaded_bundle.dataframe().await?;
    let batches = df.as_ref().clone().collect().await?;
    assert!(batches[0].num_rows() > 0);
    assert!(!batches[0].schema().column_with_name("title").is_some());

    Ok(())
}

#[tokio::test]
async fn test_empty_bundle() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_url();
    let bundle = bundlebase::BundleBuilder::create(data_dir.as_str(), None).await?;

    assert_eq!(0, bundle.num_rows().await?);

    // Commit empty bundle
    bundle.commit("Initial commit").await?;

    // Verify commit succeeded and bundle is still queryable
    assert_eq!(0, bundle.num_rows().await?);

    // Test loading the saved bundle
    let loaded_bundle = Bundle::open(data_dir.as_str(), None).await?;

    // Verify it's empty
    assert_eq!(loaded_bundle.num_rows().await?, 0);

    Ok(())
}

#[tokio::test]
async fn test_save_multiple_operations() -> Result<(), BundlebaseError> {
    init();
    let temp_dir = random_memory_dir();

    let bundle = bundlebase::BundleBuilder::create(temp_dir.url().as_str(), None).await?;
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    bundle.drop_column("title").await?;
    bundle.drop_column("comments").await?;
    bundle.rename_column("first_name", "fname").await?;
    bundle.rename_column("last_name", "lname").await?;

    // Save bundle
    bundle.commit("Commit changes").await?;

    // Find and read the versioned manifest file
    let (contents, commit, _) = common::latest_commit(temp_dir.as_ref()).await?.unwrap();

    let expected = format!(
        r#"
author: {}
message: Commit changes
timestamp: {}
changes:
- id: {}
  description: {}
  operations:
  - type: attachBlock
    id: {}
    pack: {}
    location: memory:///test_data/userdata.parquet
    format: parquet
    version: {}
    hash: 8c26edb7f30d7694a1431224f28e5932
    numRows: 1000
    bytes: 113629
    schema: 3b/a5bd5f9d91f9d1.block.schema.yaml
- id: {}
  description: DROP COLUMN title
  operations:
  - type: dropColumn
- id: {}
  description: DROP COLUMN comments
  operations:
  - type: dropColumn
- id: {}
  description: RENAME COLUMN first_name TO fname
  operations:
  - type: renameColumn
    newName: fname
- id: {}
  description: RENAME COLUMN last_name TO lname
  operations:
  - type: renameColumn
    newName: lname
"#,
        commit.author,
        commit.timestamp,
        commit.changes[0].id,
        commit.changes[0].description,
        test_utils::for_yaml(String::from(op_field!(
            &commit.operations()[0],
            AnyOperation::AttachBlock,
            id
        ))),
        test_utils::for_yaml(String::from(op_field!(
            &commit.operations()[0],
            AnyOperation::AttachBlock,
            pack
        ))),
        test_utils::for_yaml(op_field!(
            &commit.operations()[0],
            AnyOperation::AttachBlock,
            version
        )),
        commit.changes[1].id,
        commit.changes[2].id,
        commit.changes[3].id,
        commit.changes[4].id,
    );
    assert_eq!(common::strip_column_ids(&contents).trim(), expected.trim());

    // Verify column IDs are present in the actual serialized output
    assert!(
        contents.contains("columnIds:"),
        "AttachBlock should have columnIds"
    );
    assert!(
        contents.contains("type: dropColumn\n    id:"),
        "DropColumn should have id"
    );
    assert!(
        contents.contains("type: renameColumn\n    id:"),
        "RenameColumn should have id"
    );

    Ok(())
}

#[tokio::test]
async fn test_name_and_description() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_url();
    let bundle = bundlebase::BundleBuilder::create(data_dir.as_str(), None).await?;

    // Default should be None
    assert!(bundle.bundle().name().is_none());
    assert!(bundle.bundle().description().is_none());

    // Set name and verify getter
    bundle.set_name("My Bundle").await?;
    bundle.set_description("My Bundle Desc").await?;

    assert_eq!(bundle.bundle().name(), Some("My Bundle".to_string()));
    assert_eq!(
        bundle.bundle().description(),
        Some("My Bundle Desc".to_string())
    );

    bundle.commit("Commit changes").await?;

    assert_eq!(bundle.bundle().name(), Some("My Bundle".to_string()));
    assert_eq!(
        bundle.bundle().description(),
        Some("My Bundle Desc".to_string())
    );

    // Open and verify
    let loaded = Bundle::open(data_dir.as_str(), None).await?;
    assert_eq!(loaded.name(), Some("My Bundle".to_string()));
    assert_eq!(loaded.description(), Some("My Bundle Desc".to_string()));

    Ok(())
}

#[tokio::test]
async fn test_attach_csv() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("customers-0-100.csv"), None)
        .await?;

    bundle.commit("CSV commit").await?;

    // Find and read the versioned manifest file
    let (contents, commit, _) = common::latest_commit(bundle.data_dir().as_ref())
        .await?
        .unwrap();

    let layout_line = match op_field!(commit.operations()[0], AnyOperation::AttachBlock, layout) {
        Some(layout_val) => format!("\n    layout: {}", test_utils::for_yaml(layout_val)),
        None => String::new(),
    };

    assert_eq!(
        format!(
            r"
author: {}
message: CSV commit
timestamp: {}
changes:
- id: {}
  description: {}
  operations:
  - type: attachBlock
    id: {}
    pack: {}
    location: memory:///test_data/customers-0-100.csv
    format: csv
    version: {}
    hash: {}{}
    numRows: 100
    bytes: 17160
    schema: 26/2b64b78fa6eff8.block.schema.yaml",
            commit.author,
            commit.timestamp,
            commit.changes[0].id,
            commit.changes[0].description,
            test_utils::for_yaml(
                op_field!(commit.operations()[0], AnyOperation::AttachBlock, id).into()
            ),
            test_utils::for_yaml(
                op_field!(commit.operations()[0], AnyOperation::AttachBlock, pack).into()
            ),
            test_utils::for_yaml(op_field!(
                commit.operations()[0],
                AnyOperation::AttachBlock,
                version
            )),
            op_field!(commit.operations()[0], AnyOperation::AttachBlock, hash),
            layout_line,
        )
        .trim(),
        common::strip_column_ids(&contents).trim()
    );

    // Verify column IDs are present in the actual serialized output
    assert!(
        contents.contains("columnIds:"),
        "AttachBlock should have columnIds"
    );

    // Open the saved bundle
    let loaded_bundle = Bundle::open(data_dir.url().as_str(), None).await?;

    // Verify data can be queried
    let df = loaded_bundle.dataframe().await?;
    let batches = df.as_ref().clone().collect().await?;
    assert!(batches[0].num_rows() > 0);
    assert!(batches[0].schema().column_with_name("Website").is_some());

    // Verify layout file exists if one was created (small files don't need layouts)
    if let Some(layout) = op_field!(commit.operations()[0], AnyOperation::AttachBlock, layout) {
        let layout_file = readable_file_from_path(
            &layout,
            loaded_bundle.data_dir(),
            Arc::new(BundleConfig::new(None)?) as Arc<dyn ConfigProvider>,
        )
        .await?;
        assert!(
            layout_file.exists().await?,
            "Layout file should exist at: {}",
            layout
        );
    }

    Ok(())
}

#[tokio::test]
async fn test_attach_json() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("objects.jsonl"), None)
        .await?
        .rename_column("score", "points")
        .await?;

    bundle.commit("JSON commit").await?;

    // Find and read the versioned manifest file
    let (contents, commit, _) = common::latest_commit(bundle.data_dir().as_ref())
        .await?
        .unwrap();

    // Verify it contains the expected operations
    assert!(contents.contains("author: "));
    assert!(contents.contains("message: JSON commit"));
    assert!(contents.contains("type: attachBlock"));
    assert!(contents.contains("location: memory:///test_data/objects.jsonl"));
    assert!(contents.contains("type: renameColumn"));
    // oldName is no longer serialized (resolved at runtime from column ID)
    assert!(!contents.contains("oldName: score"));
    assert!(contents.contains("newName: points"));
    assert!(contents.contains("numRows: 4"));

    // Verify the attach operation metadata
    match &commit.operations()[0] {
        AnyOperation::AttachBlock(op) => {
            assert_eq!(op.location, "memory:///test_data/objects.jsonl");
            assert_eq!(op.num_rows, Some(4));
            // Version is present and not empty
            assert!(!op.version.is_empty());
        }
        _ => panic!("Expected AttachBlock operation"),
    }

    // Open the saved bundle
    let loaded_bundle = Bundle::open(data_dir.url().as_str(), None).await?;

    // Verify data can be queried
    let df = loaded_bundle.dataframe().await?;
    let batches = df.as_ref().clone().collect().await?;
    assert_eq!(batches[0].num_rows(), 4); // objects.jsonl has 4 rows
    assert!(batches[0].schema().column_with_name("points").is_some());
    assert!(!batches[0].schema().column_with_name("score").is_some());

    Ok(())
}

// ==================== Version compatibility tests ====================

/// Helper to overwrite the init commit YAML with custom min/max versions.
async fn set_init_versions(
    data_dir: &dyn bundlebase_io::IOReadWriteDir,
    min_version: Option<&str>,
    max_version: Option<&str>,
) {
    let manifest_dir = data_dir.subdir(META_DIR).unwrap();
    let init_file_read = manifest_dir.file(INIT_FILENAME).unwrap();
    let yaml_str = init_file_read.read_str().await.unwrap().unwrap();
    let mut doc: serde_yaml_ng::Value = serde_yaml_ng::from_str(&yaml_str).unwrap();

    match min_version {
        Some(v) => {
            doc["minVersion"] = serde_yaml_ng::Value::String(v.to_string());
        }
        None => {
            doc.as_mapping_mut().unwrap().remove("minVersion");
        }
    }
    match max_version {
        Some(v) => {
            doc["maxVersion"] = serde_yaml_ng::Value::String(v.to_string());
        }
        None => {
            doc.as_mapping_mut().unwrap().remove("maxVersion");
        }
    }

    let new_yaml = serde_yaml_ng::to_string(&doc).unwrap();
    let manifest_dir_rw = data_dir.writable_subdir(META_DIR).unwrap();
    let init_file_write = manifest_dir_rw.writable_file(INIT_FILENAME).unwrap();
    init_file_write
        .write(bytes::Bytes::from(new_yaml))
        .await
        .unwrap();
}

#[tokio::test]
async fn test_version_check_passes_for_current_version() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    bundle.commit("Initial").await?;

    // Should open fine — versions match current
    let _loaded = Bundle::open(data_dir.url().as_str(), None).await?;
    Ok(())
}

#[tokio::test]
async fn test_version_check_fails_when_min_version_too_high() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    bundle.commit("Initial").await?;

    // Set min version higher than current
    set_init_versions(data_dir.as_ref(), Some("99.0"), Some("99.0")).await;

    let err_msg = match Bundle::open(data_dir.url().as_str(), None).await {
        Err(e) => e.to_string(),
        Ok(_) => panic!("Expected version check to fail"),
    };
    assert!(
        err_msg.contains("requires bundlebase >= 99.0"),
        "Error: {}",
        err_msg
    );
    Ok(())
}

#[tokio::test]
async fn test_version_check_fails_when_max_version_too_low() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    bundle.commit("Initial").await?;

    // Set max version lower than current
    set_init_versions(data_dir.as_ref(), Some("0.1"), Some("0.1")).await;

    let err_msg = match Bundle::open(data_dir.url().as_str(), None).await {
        Err(e) => e.to_string(),
        Ok(_) => panic!("Expected version check to fail"),
    };
    assert!(
        err_msg.contains("requires bundlebase <= 0.1"),
        "Error: {}",
        err_msg
    );
    assert!(
        err_msg.contains("upgrade-bundle"),
        "Error should mention upgrade-bundle: {}",
        err_msg
    );
    Ok(())
}

#[tokio::test]
async fn test_version_check_allows_old_bundles_without_versions() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    bundle.commit("Initial").await?;

    // Remove version fields to simulate pre-versioning bundle
    set_init_versions(data_dir.as_ref(), None, None).await;

    // Should still open
    let _loaded = Bundle::open(data_dir.url().as_str(), None).await?;
    Ok(())
}

#[tokio::test]
async fn test_upgrade_bundle_fixes_too_old_bundle() -> Result<(), BundlebaseError> {
    init();
    let data_dir = random_memory_dir();
    let bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    bundle.commit("Initial").await?;

    // Set max version too low so open would fail
    set_init_versions(data_dir.as_ref(), Some("0.1"), Some("0.1")).await;

    // Verify it would fail to open
    assert!(
        matches!(Bundle::open(data_dir.url().as_str(), None).await, Err(_)),
        "Expected version check to fail"
    );

    // Upgrade the bundle (bypasses version check)
    bundlebase::BundleBuilder::upgrade_bundle(data_dir.url().as_str(), None).await?;

    // Now it should open successfully
    let loaded = Bundle::open(data_dir.url().as_str(), None).await?;
    assert!(loaded.num_rows().await? > 0);
    Ok(())
}
