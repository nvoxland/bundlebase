use arrow_schema::{DataType, Field, Schema, SchemaRef};
use bundlebase;
use bundlebase::bundle::{BundleFacade, INIT_FILENAME, META_DIR};
use bundlebase::io::{readable_file_from_path, readable_file_from_url};
use bundlebase::test_utils::{random_memory_dir, random_memory_url, test_datafile};
use bundlebase::BundleConfig;
use bundlebase::FunctionSignature;
use bundlebase::{op_field, AnyOperation};
use bundlebase::{test_utils, Bundle, BundlebaseError};
use url::Url;

mod common;

#[tokio::test]
async fn test_basic_e2e() -> Result<(), BundlebaseError> {
    let data_dir = random_memory_dir();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("userdata.parquet"), None)
        .await?
        .drop_column("title")
        .await?
        .rename_column("first_name", "name")
        .await?;
    let version = readable_file_from_url(
        &Url::parse(test_datafile("userdata.parquet"))?,
        BundleConfig::default().into(),
    )?
    .version()
    .await?;

    bundle.commit("First commit").await?;

    let init_content = bundle
        .data_dir()
        .file(&format!("{}/{}", META_DIR, INIT_FILENAME))?
        .read_str()
        .await?
        .expect("init commit doesn't exist");
    assert_eq!(
        init_content.trim(),
        format!("id: {}", bundle.bundle().id()).trim()
    );

    // Find and read the versioned manifest file
    let (contents, commit, url) = common::latest_commit(bundle.data_dir().as_ref()).await?.unwrap();

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
    version: {}
    hash: 59d4fdcdd71e5b6ab79d0bc8fae8ee6f144d3639250facb4b519b36b92c8a5cc
    numRows: 1000
    bytes: 113629
    schema:
      fields:
      - name: registration_dttm
        data_type:
          type: Timestamp
          unit: Nanosecond
          timezone: null
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: id
        data_type: Int32
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: first_name
        data_type: Utf8View
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: last_name
        data_type: Utf8View
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: email
        data_type: Utf8View
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: gender
        data_type: Utf8View
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: ip_address
        data_type: Utf8View
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: cc
        data_type: Utf8View
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: country
        data_type: Utf8View
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: birthdate
        data_type: Utf8View
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: salary
        data_type: Float64
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: title
        data_type: Utf8View
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: comments
        data_type: Utf8View
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      metadata: {{}}
- id: {}
  description: DROP COLUMN title
  operations:
  - type: dropColumn
    names:
    - title
- id: {}
  description: RENAME COLUMN first_name TO name
  operations:
  - type: renameColumn
    oldName: first_name
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
    assert_eq!(contents, expected);

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
    let data_dir = random_memory_url();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.as_str(), None).await?;

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
    let temp_dir = random_memory_dir();

    let mut bundle = bundlebase::BundleBuilder::create(temp_dir.url().as_str(), None).await?;
    bundle.attach(test_datafile("userdata.parquet"), None).await?;
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
    version: {}
    hash: 59d4fdcdd71e5b6ab79d0bc8fae8ee6f144d3639250facb4b519b36b92c8a5cc
    numRows: 1000
    bytes: 113629
    schema:
      fields:
      - name: registration_dttm
        data_type:
          type: Timestamp
          unit: Nanosecond
          timezone: null
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: id
        data_type: Int32
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: first_name
        data_type: Utf8View
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: last_name
        data_type: Utf8View
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: email
        data_type: Utf8View
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: gender
        data_type: Utf8View
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: ip_address
        data_type: Utf8View
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: cc
        data_type: Utf8View
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: country
        data_type: Utf8View
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: birthdate
        data_type: Utf8View
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: salary
        data_type: Float64
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: title
        data_type: Utf8View
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: comments
        data_type: Utf8View
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      metadata: {{}}
- id: {}
  description: DROP COLUMN title
  operations:
  - type: dropColumn
    names:
    - title
- id: {}
  description: DROP COLUMN comments
  operations:
  - type: dropColumn
    names:
    - comments
- id: {}
  description: RENAME COLUMN first_name TO fname
  operations:
  - type: renameColumn
    oldName: first_name
    newName: fname
- id: {}
  description: RENAME COLUMN last_name TO lname
  operations:
  - type: renameColumn
    oldName: last_name
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
    assert_eq!(contents.trim(), expected.trim());

    Ok(())
}

#[tokio::test]
async fn test_open_with_function() -> Result<(), BundlebaseError> {
    let data_dir = random_memory_dir();

    // Create bundle with function definition
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;
    let schema = SchemaRef::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("value", DataType::Utf8, true),
    ]));
    bundle
        .create_function(FunctionSignature::new("test_func", schema))
        .await?;

    bundle.commit("Commit changes").await?;

    // Open the saved bundle
    let _loaded_bundle = Bundle::open(data_dir.url().as_str(), None).await?;

    Ok(())
}

#[tokio::test]
async fn test_name_and_description() -> Result<(), BundlebaseError> {
    let data_dir = random_memory_url();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.as_str(), None).await?;

    // Default should be None
    assert!(bundle.bundle().name().is_none());
    assert!(bundle.bundle().description().is_none());

    // Set name and verify getter
    bundle.set_name("My Bundle").await?;
    bundle.set_description("My Bundle Desc").await?;

    assert_eq!(bundle.bundle().name(), Some("My Bundle".to_string()));
    assert_eq!(bundle.bundle().description(), Some("My Bundle Desc".to_string()));

    bundle.commit("Commit changes").await?;

    assert_eq!(bundle.bundle().name(), Some("My Bundle".to_string()));
    assert_eq!(bundle.bundle().description(), Some("My Bundle Desc".to_string()));

    // Open and verify
    let loaded = Bundle::open(data_dir.as_str(), None).await?;
    assert_eq!(loaded.name(), Some("My Bundle".to_string()));
    assert_eq!(loaded.description(), Some("My Bundle Desc".to_string()));

    Ok(())
}

#[tokio::test]
async fn test_attach_csv() -> Result<(), BundlebaseError> {
    let data_dir = random_memory_dir();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle.attach(test_datafile("customers-0-100.csv"), None).await?;

    bundle.commit("CSV commit").await?;

    // Find and read the versioned manifest file
    let (contents, commit, _) = common::latest_commit(bundle.data_dir().as_ref()).await?.unwrap();

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
    version: {}
    hash: f2147696392a019d768a11ff68bab8e8dec77b5af2c93e8e5d5e399bd7bba8b9
    layout: {}
    numRows: 100
    bytes: 17160
    schema:
      fields:
      - name: Index
        data_type: Int64
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: Customer Id
        data_type: Utf8
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: First Name
        data_type: Utf8
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: Last Name
        data_type: Utf8
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: Company
        data_type: Utf8
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: City
        data_type: Utf8
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: Country
        data_type: Utf8
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: Phone 1
        data_type: Utf8
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: Phone 2
        data_type: Utf8
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: Email
        data_type: Utf8
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: Subscription Date
        data_type: Date32
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      - name: Website
        data_type: Utf8
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {{}}
      metadata: {{}}",
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
            test_utils::for_yaml(
                op_field!(commit.operations()[0], AnyOperation::AttachBlock, layout).unwrap()
            )
        )
        .trim(),
        contents.trim()
    );

    // Open the saved bundle
    let loaded_bundle = Bundle::open(data_dir.url().as_str(), None).await?;

    // Verify data can be queried
    let df = loaded_bundle.dataframe().await?;
    let batches = df.as_ref().clone().collect().await?;
    assert!(batches[0].num_rows() > 0);
    assert!(batches[0].schema().column_with_name("Website").is_some());

    // Verify layout file exists
    let layout = op_field!(commit.operations()[0], AnyOperation::AttachBlock, layout).unwrap();
    let layout_file = readable_file_from_path(
        &layout,
        loaded_bundle.data_dir(),
        BundleConfig::default().into(),
    )?;
    assert!(
        layout_file.exists().await?,
        "Layout file should exist at: {}",
        layout
    );

    Ok(())
}

#[tokio::test]
async fn test_attach_json() -> Result<(), BundlebaseError> {
    let data_dir = random_memory_dir();
    let mut bundle = bundlebase::BundleBuilder::create(data_dir.url().as_str(), None).await?;

    bundle
        .attach(test_datafile("objects.json"), None)
        .await?
        .rename_column("score", "points")
        .await?;

    bundle.commit("JSON commit").await?;

    // Find and read the versioned manifest file
    let (contents, commit, _) = common::latest_commit(bundle.data_dir().as_ref()).await?.unwrap();

    // Verify it contains the expected operations
    assert!(contents.contains("author: "));
    assert!(contents.contains("message: JSON commit"));
    assert!(contents.contains("type: attachBlock"));
    assert!(contents.contains("location: memory:///test_data/objects.json"));
    assert!(contents.contains("type: renameColumn"));
    assert!(contents.contains("oldName: score"));
    assert!(contents.contains("newName: points"));
    assert!(contents.contains("numRows: 4"));

    // Verify the attach operation metadata
    match &commit.operations()[0] {
        AnyOperation::AttachBlock(op) => {
            assert_eq!(op.location, "memory:///test_data/objects.json");
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
    assert_eq!(batches[0].num_rows(), 4); // objects.json has 4 rows
    assert!(batches[0].schema().column_with_name("points").is_some());
    assert!(!batches[0].schema().column_with_name("score").is_some());

    Ok(())
}
