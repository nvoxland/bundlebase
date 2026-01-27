use crate::bundle::facade::BundleFacade;
use crate::bundle::operation::Operation;
use crate::bundle::DataBlock;
use crate::data::ObjectId;
use crate::io::readable_file_from_path;
use crate::progress::ProgressScope;
use crate::source::AttachedFileInfo;
use crate::{Bundle, BundleBuilder, BundlebaseError};
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use datafusion::common::DataFusionError;
use log::debug;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Information about the source that a block was fetched from.
///
/// This struct consolidates source-related fields for blocks attached via source fetch.
/// When present, all fields are required and track the origin of the data.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    /// The source function ID that fetched this block
    pub id: ObjectId,
    /// The original source location (e.g., remote URL) where data was fetched from
    pub location: String,
    /// The version of the source at fetch time (e.g., ETag, last-modified)
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AttachBlockOp {
    pub id: ObjectId,
    pub pack: ObjectId,
    pub location: String,
    pub version: String,
    /// SHA256 hash of the content (full 64-character hex string)
    pub hash: String,
    #[serde(rename = "source", skip_serializing_if = "Option::is_none")]
    pub source_info: Option<SourceInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_rows: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<usize>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "super::serde_util::serialize_schema_option",
        deserialize_with = "super::serde_util::deserialize_schema_option"
    )]
    pub schema: Option<SchemaRef>,
}

impl AttachBlockOp {
    /// Read version from a URL using the adapter factory.
    async fn read_version_from(
        url: &str,
        builder: &BundleBuilder,
    ) -> Result<String, BundlebaseError> {
        let temp_id = ObjectId::generate();
        let adapter_factory = builder.bundle().reader_factory.clone();
        let adapter = adapter_factory
            .reader(url, &temp_id, builder, None, None, None)
            .await?;
        adapter.read_version().await
    }

    /// Setup an AttachBlockOp for a file attached via source fetch.
    ///
    /// Reads version from `source_location` (the remote URL) rather than
    /// `attach_location` (the local copy). This enables accurate change
    /// detection on subsequent fetches when `copy=true`.
    ///
    /// # Arguments
    /// * `pack` - Pack to attach the block to
    /// * `attach_location` - Where data is stored (local copy if copy=true)
    /// * `source_location` - Original remote URL for version tracking
    /// * `hash` - SHA256 hash of the content (computed during copy/materialize)
    /// * `builder` - Bundle builder
    pub async fn setup_for_source(
        pack: &ObjectId,
        attach_location: &str,
        source_location: &str,
        hash: &str,
        builder: &BundleBuilder,
    ) -> Result<Self, BundlebaseError> {
        // Read version from SOURCE location for change detection
        let source_version = Self::read_version_from(source_location, builder).await?;

        // Setup normally for schema/stats from attach_location
        let mut op = Self::setup_with_hash(pack, attach_location, hash, builder).await?;

        // Override version with source version
        op.version = source_version;

        Ok(op)
    }

    /// Setup an AttachBlockOp for a file, computing the hash by streaming.
    pub async fn setup(
        pack: &ObjectId,
        location: &str,
        builder: &BundleBuilder,
    ) -> Result<Self, BundlebaseError> {
        // Create progress scope (indeterminate - we don't know how many steps)
        let _progress = ProgressScope::new(
            &format!("Attaching '{}'", location),
            None, // indeterminate progress
        );

        _progress.update(1, Some("Computing hash"));

        // Check if this is a function:// URL - these don't support file-based hash
        //todo: do this right
        let hash = if location.starts_with("function://") {
            // For functions, use version as hash proxy
            let version = Self::read_version_from(location, builder).await?;

            // Hash the version string as a proxy for content hash
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(version.as_bytes());
            hex::encode(hasher.finalize())
        } else {
            // Normal file-based hash computation for other schemes
            let file = readable_file_from_path(location, builder.data_dir(), builder.config())?;
            file.compute_hash().await?
        };

        Self::setup_with_hash(pack, location, &hash, builder).await
    }

    /// Setup an AttachBlockOp with a pre-computed hash.
    ///
    /// Used when the hash is already known (e.g., from source materialization).
    pub async fn setup_with_hash(
        pack: &ObjectId,
        location: &str,
        hash: &str,
        builder: &BundleBuilder,
    ) -> Result<Self, BundlebaseError> {
        // Create progress scope (indeterminate - we don't know how many steps)
        let _progress = ProgressScope::new(
            &format!("Attaching '{}'", location),
            None, // indeterminate progress
        );

        let block_id = ObjectId::generate();

        _progress.update(1, Some("Creating adapter"));
        let adapter_factory = builder.bundle().reader_factory.clone();
        let adapter = adapter_factory
            .reader(location, &block_id, builder, None, None, None)
            .await?;

        _progress.update(2, Some("Reading version"));
        let version = adapter.read_version().await?;

        _progress.update(3, Some("Reading schema"));
        let schema = adapter.read_schema().await?;

        let mut op = AttachBlockOp {
            location: location.to_string(),
            num_rows: None,
            bytes: None,
            version,
            hash: hash.to_string(),
            schema,
            id: block_id,
            pack: *pack,
            layout: None,
            source_info: None,
        };

        _progress.update(4, Some("Reading statistics"));
        match adapter.read_statistics().await? {
            Some(stats) => {
                op.num_rows = stats.num_rows.get_value().copied();
                op.bytes = stats.total_byte_size.get_value().copied();
            }
            None => {
                debug!("No statistics available for adapter at {}", adapter.url());
            }
        }

        _progress.update(5, Some("Building layout"));
        let data_dir = builder.bundle().data_dir();
        op.layout = match adapter.build_layout(data_dir.as_ref()).await? {
            Some(file) => Some(data_dir.relative_path(file.as_ref())?),
            None => None,
        };

        Ok(op)
    }
}

#[async_trait]
impl Operation for AttachBlockOp {
    fn describe(&self) -> String {
        format!("ATTACH: {}", self.location)
    }

    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        Ok(())
    }

    fn allowed_on_view(&self) -> bool {
        false
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        // Only validate version for files that are NOT copied from a source.
        // When a file is copied (source_info.is_some()), the stored version is the
        // SOURCE version, not the local copy's version. The local copy is internal
        // to the bundle and won't change unexpectedly.
        let expected_version = if self.source_info.is_none() {
            Some(self.version.clone())
        } else {
            None
        };

        let reader = bundle
            .reader_factory
            .reader(
                self.location.as_str(),
                &self.id,
                bundle,
                self.schema.clone(),
                self.layout.clone(),
                expected_version,
            )
            .await?;

        let block = Arc::new(DataBlock::new(
            self.id,
            self.schema.clone().expect("BUG: schema must be set during setup"),
            &self.version,
            reader,
            bundle.indexes().clone(),
            bundle.data_dir(),
            bundle.config(),
            self.source_info.clone(),
        ));

        let pack = bundle.get_pack(&self.pack).expect("Cannot find pack");
        pack.add_block(block);

        // Add to source's attached_files tracking
        if let Some(ref source_info) = self.source_info {
            if let Some(source) = bundle.get_source(&source_info.id) {
                source.add_attached_file(
                    &source_info.location,
                    AttachedFileInfo {
                        location: self.location.clone(),
                        version: source_info.version.clone(),
                        bytes: self.bytes,
                    },
                );
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::BundleFacade;
    use crate::io::plugin::object_store::ObjectStoreFile;
    use crate::io::IOReadFile;
    use crate::test_utils::{empty_bundle, for_yaml, test_datafile};
    use crate::BundleConfig;
    use url::Url;

    #[tokio::test]
    async fn test_describe() {
        let op = AttachBlockOp {
            location: "file:///test/data.csv".to_string(),
            version: "test-version".to_string(),
            hash: "0".repeat(64),
            id: ObjectId::from(1),
            pack: ObjectId::from(2),
            num_rows: None,
            bytes: None,
            schema: None,
            layout: None,
            source_info: None,
        };

        assert_eq!(op.describe(), "ATTACH: file:///test/data.csv");
    }

    #[tokio::test]
    async fn test_setup() -> Result<(), BundlebaseError> {
        let datafile = test_datafile("userdata.parquet");
        let bundle = empty_bundle().await;
        let op =
            AttachBlockOp::setup(&ObjectId::generate(), datafile, bundle.as_ref()).await?;
        let block_id = String::from(op.id);
        let pack = String::from(op.pack);
        let version = ObjectStoreFile::from_url(
            &Url::parse(datafile).unwrap(),
            BundleConfig::default().into(),
        )?
        .version()
        .await?;

        assert_eq!(
            format!(
                r#"id: {}
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
"#,
                for_yaml(block_id),
                for_yaml(pack),
                for_yaml(version),
            ),
            serde_yaml_ng::to_string(&op)?
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_attach_dataframe_schema() -> Result<(), BundlebaseError> {
        let mut bundle = crate::BundleBuilder::create("memory:///test_bundle", None).await?;
        bundle.attach(test_datafile("userdata.parquet"), None).await?;

        // Get the DataFrame from the bundle
        let df = bundle.dataframe().await?;
        let df_schema = df.schema();

        // Verify DataFrame schema has correct column names and types
        let schema_string = df_schema
            .fields()
            .iter()
            .map(|f| format!("{}: {}", f.name(), f.data_type()))
            .collect::<Vec<_>>()
            .join("\n");

        // Expected schema with all column names and their data types from the parquet file
        let expected_schema = "registration_dttm: Timestamp(ns)\n\
                               id: Int32\n\
                               first_name: Utf8View\n\
                               last_name: Utf8View\n\
                               email: Utf8View\n\
                               gender: Utf8View\n\
                               ip_address: Utf8View\n\
                               cc: Utf8View\n\
                               country: Utf8View\n\
                               birthdate: Utf8View\n\
                               salary: Float64\n\
                               title: Utf8View\n\
                               comments: Utf8View";

        assert_eq!(schema_string, expected_schema,);

        Ok(())
    }

    #[tokio::test]
    async fn test_version() {
        let op = AttachBlockOp {
            location: "file:///test/data.csv".to_string(),
            version: "test-version".to_string(),
            hash: "0".repeat(64),
            id: ObjectId::from(1),
            pack: ObjectId::from(2),
            num_rows: None,
            bytes: None,
            schema: None,
            layout: None,
            source_info: None,
        };

        let version = op.version();

        // Note: version hash changes when struct fields change
        assert!(!version.is_empty());
    }
}
//
