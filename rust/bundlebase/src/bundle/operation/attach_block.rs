use crate::bundle::column_metadata;
use crate::bundle::facade::BundleFacade;
use crate::bundle::operation::Operation;
use crate::bundle::DataBlock;
use crate::data::{BlockId, ObjectId};
use crate::io::readable_file_from_path;
use crate::object_id::ColumnId;
use crate::progress::ProgressScope;
use crate::source::AttachedFileInfo;
use crate::{Bundle, BundleBuilder, BundlebaseError};
use arrow_schema::SchemaRef;
use datafusion::common::DataFusionError;
use log::debug;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// Information about the source that a block was fetched from.
///
/// This struct consolidates source-related fields for blocks attached via source fetch.
/// When present, all fields are required and track the origin of the data.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    /// The source ID that fetched this block
    pub id: ObjectId,
    /// The original source location (e.g., remote URL) where data was fetched from
    pub location: String,
    /// The version of the source at fetch time (e.g., ETag, last-modified)
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AttachBlockOp {
    pub id: BlockId,
    pub pack: ObjectId,
    pub location: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_options: Option<HashMap<String, String>>,
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
    pub column_ids: Vec<ColumnId>,
}

impl AttachBlockOp {
    /// Setup an AttachBlockOp for a file.
    ///
    /// Reads schema, version, statistics, and layout from the file at `location`.
    ///
    /// # Arguments
    /// * `pack` - Pack to attach the block to
    /// * `location` - Where data is stored (URL or path)
    /// * `hash` - Pre-computed SHA256 hash, or `None` to compute it from the file
    /// * `source_info` - Source tracking metadata, or `None` for directly-attached files
    /// * `builder` - Bundle builder
    pub async fn setup(
        pack: &ObjectId,
        location: &str,
        hash: Option<&str>,
        source_info: Option<SourceInfo>,
        builder: &BundleBuilder,
    ) -> Result<Self, BundlebaseError> {
        let progress = ProgressScope::new(
            &format!("Attaching '{}'", location),
            None,
        );

        let hash = match hash {
            Some(h) => h.to_string(),
            None => {
                progress.update(1, Some("Computing hash"));

                // Check if this is a non-file URL - these don't support file-based hash
                //todo: do this right
                if location.starts_with("bundle://") || location.starts_with("bundle+") || location.starts_with("bundlebase://") || location.starts_with("bundlebase+") {
                    let temp_id = BlockId::generate();
                    let adapter_factory = builder.bundle().reader_factory.clone();
                    let adapter = adapter_factory
                        .reader(location, &temp_id, builder, None, None, None, None)
                        .await?;
                    let version = adapter.read_version().await?;

                    use sha2::{Digest, Sha256};
                    let mut hasher = Sha256::new();
                    hasher.update(version.as_bytes());
                    hex::encode(hasher.finalize())
                } else {
                    let file = readable_file_from_path(location, builder.data_dir(), builder.config()).await?;
                    file.compute_hash().await?
                }
            }
        };

        let block_id = BlockId::generate();

        progress.update(2, Some("Creating adapter"));
        let adapter_factory = builder.bundle().reader_factory.clone();
        let adapter = adapter_factory
            .reader(location, &block_id, builder, None, None, None, None)
            .await?;

        progress.update(3, Some("Reading version"));
        let version = adapter.read_version().await?;

        progress.update(4, Some("Reading schema"));
        let schema = adapter.read_schema().await?;

        // Capture any format-specific options detected during schema inference
        let detected_options = adapter.read_options();
        let read_options = if detected_options.is_empty() {
            None
        } else {
            Some(detected_options)
        };

        // Reuse existing ColumnIds for columns whose names match existing columns.
        // This ensures the same logical column shares a ColumnId across blocks.
        let existing_ops = builder.operations();
        let resolved = column_metadata::resolved_column_names(&existing_ops);
        let name_to_id: HashMap<String, ColumnId> = resolved
            .into_iter()
            .map(|(id, name)| (name, id))
            .collect();

        let column_ids = schema.as_ref()
            .map(|s| {
                s.fields().iter().map(|f| {
                    name_to_id.get(f.name()).copied().unwrap_or_else(ColumnId::generate)
                }).collect()
            })
            .unwrap_or_default();

        let mut op = AttachBlockOp {
            location: location.to_string(),
            num_rows: None,
            bytes: None,
            version,
            hash,
            schema,
            id: block_id,
            pack: *pack,
            layout: None,
            source_info,
            read_options,
            column_ids,
        };

        progress.update(5, Some("Reading statistics"));
        match adapter.read_statistics().await? {
            Some(stats) => {
                op.num_rows = stats.num_rows.get_value().copied();
                op.bytes = stats.total_byte_size.get_value().copied();
            }
            None => {
                debug!("No statistics available for adapter at {}", adapter.url());
            }
        }

        progress.update(6, Some("Building layout"));
        let data_dir = builder.bundle().data_dir();
        op.layout = match adapter.build_layout(data_dir.as_ref()).await? {
            Some(file) => Some(data_dir.relative_path(file.as_ref())?),
            None => None,
        };

        Ok(op)
    }
}

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
                self.read_options.as_ref(),
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
            self.column_ids.clone(),
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
    
    use url::Url;

    #[tokio::test]
    async fn test_describe() {
        let op = AttachBlockOp {
            location: "file:///test/data.csv".to_string(),
            version: "test-version".to_string(),
            hash: "0".repeat(64),
            id: BlockId::generate(),
            pack: ObjectId::generate(),
            num_rows: None,
            bytes: None,
            schema: None,
            layout: None,
            source_info: None,
            read_options: None,
            column_ids: vec![],
        };

        assert_eq!(op.describe(), "ATTACH: file:///test/data.csv");
    }

    #[tokio::test]
    async fn test_setup() -> Result<(), BundlebaseError> {
        let datafile = test_datafile("userdata.parquet");
        let bundle = empty_bundle().await;
        let op =
            AttachBlockOp::setup(&ObjectId::generate(), datafile, None, None, bundle.as_ref()).await?;
        let block_id = String::from(op.id);
        let pack = String::from(op.pack);
        let version = ObjectStoreFile::from_url(
            &Url::parse(datafile).unwrap(),
            crate::test_utils::test_config(),
        )?
        .version()
        .await?;

        let serialized = serde_yaml_ng::to_string(&op)?;

        // Check the static portion (before columnIds which contains random values)
        let expected_prefix = format!(
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
  - name: id
    data_type: Int32
    nullable: true
  - name: first_name
    data_type: Utf8View
    nullable: true
  - name: last_name
    data_type: Utf8View
    nullable: true
  - name: email
    data_type: Utf8View
    nullable: true
  - name: gender
    data_type: Utf8View
    nullable: true
  - name: ip_address
    data_type: Utf8View
    nullable: true
  - name: cc
    data_type: Utf8View
    nullable: true
  - name: country
    data_type: Utf8View
    nullable: true
  - name: birthdate
    data_type: Utf8View
    nullable: true
  - name: salary
    data_type: Float64
    nullable: true
  - name: title
    data_type: Utf8View
    nullable: true
  - name: comments
    data_type: Utf8View
    nullable: true
"#,
            for_yaml(block_id),
            for_yaml(pack),
            for_yaml(version),
        );
        assert!(
            serialized.starts_with(&expected_prefix),
            "Serialized output doesn't start with expected prefix.\nActual:\n{}",
            serialized
        );

        // Verify columnIds section has 13 entries (one per schema field)
        assert!(serialized.contains("columnIds:"));
        let column_ids_section = serialized.split("columnIds:").nth(1).unwrap();
        let column_id_count = column_ids_section.lines().filter(|l| l.trim().starts_with("- ")).count();
        assert_eq!(column_id_count, 13, "Expected 13 column IDs for 13 schema fields");
        Ok(())
    }

    #[tokio::test]
    async fn test_attach_dataframe_schema() -> Result<(), BundlebaseError> {
        let bundle = crate::BundleBuilder::create("memory:///test_bundle", None).await?;
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
            id: BlockId::generate(),
            pack: ObjectId::generate(),
            num_rows: None,
            bytes: None,
            schema: None,
            layout: None,
            source_info: None,
            read_options: None,
            column_ids: vec![],
        };

        let version = op.version();

        // Note: version hash changes when struct fields change
        assert!(!version.is_empty());
    }
}
//
