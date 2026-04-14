use crate::bundle::facade::BundleFacade;
use crate::bundle::operation::Operation;
use crate::bundle::DataBlock;
use crate::connector::AttachFormat;
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
/// Each attached block tracks the exact source files that produced it. Single-file
/// source fetches store one entry; batched blocks store many.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    /// The source ID that fetched this block
    pub id: ObjectId,
    /// Source files that produced this block.
    pub batch_sources: Vec<BatchedSource>,
}

/// A single source file within a batched block.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BatchedSource {
    pub location: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AttachBlockOp {
    pub id: BlockId,
    pub pack: ObjectId,
    pub location: String,
    pub format: AttachFormat,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_options: Option<HashMap<String, String>>,
    pub version: String,
    /// xxHash (xxh3-128) of the content (32-character hex string)
    pub hash: String,
    #[serde(rename = "source", skip_serializing_if = "Option::is_none")]
    pub source_info: Option<SourceInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_rows: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<usize>,
    pub schema: String,
    pub column_ids: String,
    /// In-memory cache of the schema referenced by `schema`. Never
    /// serialized — populated either at attach time (setup) or right after
    /// the manifest is parsed during `Bundle::open`.
    #[serde(skip)]
    pub schema_cache: Option<SchemaRef>,
    /// In-memory cache of the column IDs referenced by `column_ids`.
    /// Never serialized — populated at attach time or by `Bundle::open`.
    #[serde(skip)]
    pub column_ids_cache: Vec<ColumnId>,
}

/// State shared across a batch of parallel `AttachBlockOp::setup_with_shared_context`
/// calls so they can deduplicate column-id assignments and schema-file writes.
///
/// Without this batching, each parallel setup() call sees no sibling state
/// (none of the in-flight ops are applied yet) and would (a) generate fresh
/// ColumnIds for the same logical columns and (b) re-write the same schema
/// file once per attach. Both result in massive duplication.
#[derive(Debug, Default)]
pub struct SharedAttachContext {
    /// column name → ColumnId. Pre-populated from the bundle's existing
    /// schema; mutated as new columns are seen.
    pub name_to_id: parking_lot::Mutex<HashMap<String, ColumnId>>,
    /// schema content hash → relative path of the written schema file.
    /// First setup that sees a given schema writes the file; subsequent
    /// setups in the same batch reuse the path.
    pub schema_paths: parking_lot::Mutex<HashMap<u128, String>>,
    /// column-id list content hash → relative path of the written
    /// `.block.columns.yaml` sidecar file. Same dedup semantics as
    /// `schema_paths`.
    pub column_ids_paths: parking_lot::Mutex<HashMap<u128, String>>,
}

impl AttachBlockOp {
    /// Setup an AttachBlockOp for a file.
    ///
    /// Reads schema, version, statistics, and layout from the file at `location`.
    ///
    /// # Arguments
    /// * `pack` - Pack to attach the block to
    /// * `location` - Where data is stored (URL or path); must be an attachable format (CSV, TSV, JSONL, Parquet)
    /// * `hash` - Pre-computed SHA256 hash, or `None` to compute it from the file
    /// * `source_info` - Source tracking metadata, or `None` for directly-attached files
    /// * `expected_schema` - Optional expected columns for pre-reserved ID reuse (from CreateSourceOp)
    /// * `builder` - Bundle builder
    /// * `shared` - Optional shared batch context for parallel setup calls
    pub async fn setup(
        pack: &ObjectId,
        location: &str,
        format: AttachFormat,
        hash: Option<&str>,
        source_info: Option<SourceInfo>,
        expected_schema: Option<&[crate::bundle::operation::create_source::ExpectedColumn]>,
        builder: &BundleBuilder,
        shared: Option<&std::sync::Arc<SharedAttachContext>>,
    ) -> Result<Self, BundlebaseError> {
        let progress = ProgressScope::new(&format!("Attaching '{}'", location), None);

        let hash = match hash {
            Some(h) => h.to_string(),
            None => {
                progress.update(1, Some("Computing hash"));

                // Check if this is a non-file URL - these don't support file-based hash
                //todo: do this right
                if location.starts_with("bundle://")
                    || location.starts_with("bundle+")
                    || location.starts_with("bundlebase://")
                    || location.starts_with("bundlebase+")
                {
                    let temp_id = BlockId::generate();
                    let adapter_factory = builder.bundle().reader_factory.clone();
                    let adapter = adapter_factory
                        .reader(location, &format, &temp_id, builder, None, None, None, None)
                        .await?;
                    let version = adapter.read_version().await?;

                    format!("{:032x}", xxhash_rust::xxh3::xxh3_128(version.as_bytes()))
                } else {
                    let file =
                        readable_file_from_path(location, builder.data_dir(), builder.config())
                            .await?;
                    file.compute_hash().await?
                }
            }
        };

        let block_id = BlockId::generate();

        progress.update(2, Some("Creating adapter"));
        let adapter_factory = builder.bundle().reader_factory.clone();
        let adapter = adapter_factory
            .reader(
                location, &format, &block_id, builder, None, None, None, None,
            )
            .await?;

        progress.update(3, Some("Reading version"));
        let version = adapter.read_version().await?;

        progress.update(4, Some("Reading schema"));
        let schema = adapter.read_schema().await?;

        let read_options = {
            let opts = adapter.read_options();
            if opts.is_empty() {
                None
            } else {
                Some(opts)
            }
        };

        // Build a lookup from expected_schema (case-sensitive name → pre-reserved ColumnId)
        let name_to_expected_id: HashMap<&str, ColumnId> = expected_schema
            .map(|cols| cols.iter().map(|c| (c.name.as_str(), c.id)).collect())
            .unwrap_or_default();

        // Acquire (or build then acquire) the shared attach context. Locks
        // are held only across the per-field / per-schema critical sections
        // (microseconds each).
        let shared_arc = match shared {
            Some(arc) => arc.clone(),
            None => builder.shared_attach_context(),
        };
        let column_ids: Vec<ColumnId> = schema
            .as_ref()
            .map(|s| {
                let mut map_guard = shared_arc.name_to_id.lock();
                s.fields()
                    .iter()
                    .map(|f| {
                        if let Some(&id) = name_to_expected_id.get(f.name().as_str()) {
                            return id;
                        }
                        *map_guard
                            .entry(f.name().clone())
                            .or_insert_with(ColumnId::generate)
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Persist the schema and column-id list as content-addressed sidecar
        // files under the bundle's data dir. Within a batch we dedupe via
        // the shared cache; across batches the content-addressed path makes
        // writes idempotent.
        let data_dir = builder.bundle().data_dir();
        let schema_cache = schema;
        let schema = match schema_cache.as_ref() {
            Some(s) => Self::write_schema_file(s, &shared_arc, data_dir.as_ref()).await?,
            None => {
                return Err(BundlebaseError::from(format!(
                    "Cannot attach '{}': adapter returned no schema",
                    location
                )));
            }
        };
        let column_ids_cache = column_ids;
        let column_ids =
            Self::write_column_ids_file(&column_ids_cache, &shared_arc, data_dir.as_ref()).await?;

        let mut op = AttachBlockOp {
            location: location.to_string(),
            format,
            num_rows: None,
            bytes: None,
            version,
            hash,
            schema,
            column_ids,
            schema_cache,
            id: block_id,
            pack: *pack,
            layout: None,
            source_info,
            read_options,
            column_ids_cache,
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
        op.layout = match adapter.build_layout(data_dir.as_ref()).await? {
            Some(file) => Some(data_dir.relative_path(file.as_ref())?),
            None => None,
        };

        Ok(op)
    }

    /// Serialize a schema to YAML and write it to the bundle's data dir as a
    /// content-addressed sidecar file (`xx/yyyyyyyyyyyyyy.block.schema.yaml`).
    /// Within a batch, dedupe via `shared.schema_paths` so identical schemas
    /// only get written once. Returns the relative path to store on the op.
    pub async fn write_schema_file(
        schema: &SchemaRef,
        shared: &std::sync::Arc<SharedAttachContext>,
        data_dir: &dyn bundlebase_io::IOReadWriteDir,
    ) -> Result<String, BundlebaseError> {
        use bundlebase_common::{ContentAddress, ContentCategory, ContentFormat};
        use futures::stream;

        // Build the YAML body once, hash it for the in-batch dedup map.
        let yaml_body = serde_yaml_ng::to_string(&SchemaWire::from_schema(schema))
            .map_err(|e| BundlebaseError::from(format!("Failed to serialize schema: {}", e)))?;
        let body_hash = xxhash_rust::xxh3::xxh3_128(yaml_body.as_bytes());

        // Fast path: this exact schema has already been written by an earlier
        // setup in the same batch.
        if let Some(path) = shared.schema_paths.lock().get(&body_hash) {
            return Ok(path.clone());
        }

        let bytes = bytes::Bytes::from(yaml_body.into_bytes());
        let stream = Box::pin(stream::once(async move { Ok::<_, std::io::Error>(bytes) }));
        let address =
            ContentAddress::with_sub_type(ContentCategory::Block, "schema", ContentFormat::Yaml)?;
        let result = data_dir.write_stream(stream, &address).await?;
        let relative = data_dir.relative_path(result.file.as_ref())?;
        shared
            .schema_paths
            .lock()
            .insert(body_hash, relative.clone());
        Ok(relative)
    }

    /// Serialize a column-id list to YAML and write it to the data dir as a
    /// content-addressed sidecar file (`xx/yyyyyyyyyyyyyy.block.columns.yaml`).
    /// Dedup semantics mirror `write_schema_file`: identical lists share
    /// the same file via `shared.column_ids_paths`.
    pub async fn write_column_ids_file(
        column_ids: &[ColumnId],
        shared: &std::sync::Arc<SharedAttachContext>,
        data_dir: &dyn bundlebase_io::IOReadWriteDir,
    ) -> Result<String, BundlebaseError> {
        use bundlebase_common::{ContentAddress, ContentCategory, ContentFormat};
        use futures::stream;

        let yaml_body = serde_yaml_ng::to_string(column_ids)
            .map_err(|e| BundlebaseError::from(format!("Failed to serialize column_ids: {}", e)))?;
        let body_hash = xxhash_rust::xxh3::xxh3_128(yaml_body.as_bytes());

        if let Some(path) = shared.column_ids_paths.lock().get(&body_hash) {
            return Ok(path.clone());
        }

        let bytes = bytes::Bytes::from(yaml_body.into_bytes());
        let stream = Box::pin(stream::once(async move { Ok::<_, std::io::Error>(bytes) }));
        let address =
            ContentAddress::with_sub_type(ContentCategory::Block, "columns", ContentFormat::Yaml)?;
        let result = data_dir.write_stream(stream, &address).await?;
        let relative = data_dir.relative_path(result.file.as_ref())?;
        shared
            .column_ids_paths
            .lock()
            .insert(body_hash, relative.clone());
        Ok(relative)
    }
}

/// Wire form of an Arrow Schema for the standalone `.block.schema.yaml`
/// sidecar file. Reuses the same YAML shape as the previous inline `schema:`
/// field by delegating to `serde_util::serialize_schema_option` /
/// `deserialize_schema_option`.
#[derive(Debug)]
struct SchemaWire<'a>(&'a SchemaRef);

impl<'a> SchemaWire<'a> {
    fn from_schema(s: &'a SchemaRef) -> Self {
        Self(s)
    }
}

impl<'a> serde::Serialize for SchemaWire<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        super::serde_util::serialize_schema_option(&Some(self.0.clone()), serializer)
    }
}

impl Operation for AttachBlockOp {
    fn describe(&self) -> String {
        format!("ATTACH: {}", self.location)
    }

    /// Fast version hash that avoids the generic `hash_config` (which JSON
    /// round-trips and recursively re-serializes the entire op, including a
    /// potentially huge schema). The data file's `version` is already a
    /// content hash, so combining it with the few fields that change the
    /// op's identity (location, pack, column_ids, source location) is
    /// sufficient and ~1000× faster on bundles with thousands of attaches.
    fn version(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(b"AttachBlockOp\0");
        h.update(self.id.to_string().as_bytes());
        h.update(b"\0");
        h.update(self.pack.to_string().as_bytes());
        h.update(b"\0");
        h.update(self.location.as_bytes());
        h.update(b"\0");
        h.update(self.version.as_bytes());
        h.update(b"\0");
        h.update(self.hash.as_bytes());
        h.update(b"\0");
        for col_id in &self.column_ids_cache {
            h.update(col_id.to_string().as_bytes());
            h.update(b",");
        }
        h.update(b"\0");
        if let Some(ref src) = self.source_info {
            h.update(src.id.to_string().as_bytes());
            for batched_source in &src.batch_sources {
                h.update(b"\0");
                h.update(batched_source.location.as_bytes());
                h.update(b"\0");
                h.update(batched_source.version.as_bytes());
            }
        }
        hex::encode(h.finalize())[0..12].to_string()
    }

    fn to_hollow(&self, _context: &super::HollowContext) -> Option<super::AnyOperation> {
        None
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
                &self.format,
                &self.id,
                bundle,
                self.schema_cache.clone(),
                self.layout.clone(),
                expected_version,
                self.read_options.as_ref(),
            )
            .await?;

        let block = Arc::new(DataBlock::new(
            self.id,
            self.schema_cache
                .clone()
                .expect("BUG: schema must be set during setup"),
            &self.version,
            reader,
            bundle.indexes().clone(),
            bundle.data_dir(),
            bundle.config(),
            self.source_info.clone(),
            self.column_ids_cache.clone(),
            self.num_rows,
        ));

        let pack = bundle.get_pack(&self.pack).expect("Cannot find pack");
        pack.add_block(block);

        // Add to source's attached_files tracking
        if let Some(ref source_info) = self.source_info {
            if let Some(source) = bundle.get_source(&source_info.id) {
                for batched in &source_info.batch_sources {
                    source.add_attached_file(
                        &batched.location,
                        AttachedFileInfo {
                            location: self.location.clone(),
                            version: batched.version.clone(),
                            bytes: self.bytes,
                        },
                    );
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::plugin::object_store::ObjectStoreFile;
    use crate::io::IOReadFile;
    use crate::test_utils::{empty_bundle, for_yaml, test_datafile};

    use url::Url;

    #[tokio::test]
    async fn test_describe() {
        let op = AttachBlockOp {
            location: "file:///test/data.csv".to_string(),
            format: AttachFormat::Csv,
            version: "test-version".to_string(),
            hash: "0".repeat(64),
            id: BlockId::generate(),
            pack: ObjectId::generate(),
            num_rows: None,
            bytes: None,
            schema: "00/00000000000000.block.schema.yaml".to_string(),
            column_ids: "00/00000000000000.block.columns.yaml".to_string(),
            schema_cache: None,
            layout: None,
            source_info: None,
            read_options: None,
            column_ids_cache: vec![],
        };

        assert_eq!(op.describe(), "ATTACH: file:///test/data.csv");
    }

    #[tokio::test]
    async fn test_setup() -> Result<(), BundlebaseError> {
        let datafile = test_datafile("userdata.parquet");
        let bundle = empty_bundle().await;
        let op = AttachBlockOp::setup(
            &ObjectId::generate(),
            datafile,
            AttachFormat::Parquet,
            None,
            None,
            None,
            bundle.as_ref(),
            None,
        )
        .await?;
        let block_id = String::from(op.id);
        let pack = String::from(op.pack);
        let version = ObjectStoreFile::from_url(
            &Url::parse(datafile).unwrap(),
            crate::test_utils::test_config(),
        )?
        .version()
        .await?;

        let serialized = serde_yaml_ng::to_string(&op)?;

        // Check the static portion. The schema field is no longer inlined —
        // it's persisted as a sidecar file and the op carries `schema:`.
        let expected_prefix = format!(
            r#"id: {}
pack: {}
location: memory:///test_data/userdata.parquet
format: parquet
version: {}
hash: 8c26edb7f30d7694a1431224f28e5932
numRows: 1000
bytes: 113629
schema: "#,
            for_yaml(block_id),
            for_yaml(pack),
            for_yaml(version),
        );
        assert!(
            serialized.starts_with(&expected_prefix),
            "Serialized output doesn't start with expected prefix.\nActual:\n{}",
            serialized
        );

        // The serialized op should NOT contain an inline schema or columnIds
        // payload — both are persisted as sidecar files now.
        assert!(
            !serialized.contains("\n  fields:"),
            "AttachBlockOp must not serialize inline schema:\n{}",
            serialized
        );
        assert!(
            !serialized.contains("\n- 000000"),
            "AttachBlockOp must not serialize inline columnIds:\n{}",
            serialized
        );

        // schema and columnIds should reference the standard sharded sidecar layout
        assert!(
            serialized.contains(".block.schema.yaml"),
            "Expected schema ending in .block.schema.yaml:\n{}",
            serialized
        );
        assert!(
            serialized.contains(".block.columns.yaml"),
            "Expected columnIds ending in .block.columns.yaml:\n{}",
            serialized
        );
        Ok(())
    }

    // test_attach_dataframe_schema moved to integration tests (uses BundleBuilderExt)

    #[tokio::test]
    async fn test_version() {
        let op = AttachBlockOp {
            location: "file:///test/data.csv".to_string(),
            format: AttachFormat::Csv,
            version: "test-version".to_string(),
            hash: "0".repeat(64),
            id: BlockId::generate(),
            pack: ObjectId::generate(),
            num_rows: None,
            bytes: None,
            schema: "00/00000000000000.block.schema.yaml".to_string(),
            column_ids: "00/00000000000000.block.columns.yaml".to_string(),
            schema_cache: None,
            layout: None,
            source_info: None,
            read_options: None,
            column_ids_cache: vec![],
        };

        let version = op.version();

        // Note: version hash changes when struct fields change
        assert!(!version.is_empty());
    }
}
//
