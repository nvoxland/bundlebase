use crate::bundle::{BundleCommit, BundleStatus, DataBlock, Pack};
use crate::catalog;
use crate::data::ObjectId;
use crate::index::IndexDefinition;
use arrow::array::{Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use datafusion::catalog::{SchemaProvider, TableProvider};
use datafusion::datasource::MemTable;
use datafusion::error::DataFusionError;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use url::Url;

/// Bundle metadata for the details table
#[derive(Debug, Clone)]
pub struct BundleMetadata {
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub url: Url,
    pub from: Option<Url>,
    pub version: String,
}

/// SchemaProvider that exposes bundle metadata tables in the "bundle_info" schema.
/// Provides:
/// - `history`: Commit history for the bundle
/// - `status`: Uncommitted changes (only populated for BundleBuilder)
/// - `details`: Bundle metadata (id, name, description, url, from, version)
/// - `views`: List of views in the bundle
/// - `indexes`: List of indexes in the bundle
/// - `packs`: List of packs in the bundle
/// - `blocks`: List of blocks in the bundle
#[derive(Debug)]
pub struct BundleInfoSchemaProvider {
    commits: Arc<RwLock<Vec<BundleCommit>>>,
    status: Arc<RwLock<BundleStatus>>,
    metadata: Arc<RwLock<BundleMetadata>>,
    views: Arc<RwLock<HashMap<String, ObjectId>>>,
    indexes: Arc<RwLock<Vec<Arc<IndexDefinition>>>>,
    packs: Arc<RwLock<HashMap<ObjectId, Arc<Pack>>>>,
}

impl BundleInfoSchemaProvider {
    pub fn new(
        commits: Arc<RwLock<Vec<BundleCommit>>>,
        status: Arc<RwLock<BundleStatus>>,
        metadata: Arc<RwLock<BundleMetadata>>,
        views: Arc<RwLock<HashMap<String, ObjectId>>>,
        indexes: Arc<RwLock<Vec<Arc<IndexDefinition>>>>,
        packs: Arc<RwLock<HashMap<ObjectId, Arc<Pack>>>>,
    ) -> Self {
        Self {
            commits,
            status,
            metadata,
            views,
            indexes,
            packs,
        }
    }
}

#[async_trait]
impl SchemaProvider for BundleInfoSchemaProvider {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn table_names(&self) -> Vec<String> {
        vec![
            catalog::BUNDLE_HISTORY_TABLE.to_string(),
            catalog::BUNDLE_STATUS_TABLE.to_string(),
            catalog::BUNDLE_DETAILS_TABLE.to_string(),
            catalog::BUNDLE_VIEWS_TABLE.to_string(),
            catalog::BUNDLE_INDEXES_TABLE.to_string(),
            catalog::BUNDLE_PACKS_TABLE.to_string(),
            catalog::BUNDLE_BLOCKS_TABLE.to_string(),
        ]
    }

    async fn table(&self, name: &str) -> datafusion::error::Result<Option<Arc<dyn TableProvider>>> {
        if name == catalog::BUNDLE_HISTORY_TABLE {
            let commits = self.commits.read().clone();
            let table = BundleHistoryTable::new(commits)?;
            Ok(Some(Arc::new(table)))
        } else if name == catalog::BUNDLE_STATUS_TABLE {
            let status = self.status.read().clone();
            let table = BundleStatusTable::new(status)?;
            Ok(Some(Arc::new(table)))
        } else if name == catalog::BUNDLE_DETAILS_TABLE {
            let metadata = self.metadata.read().clone();
            let table = BundleDetailsTable::new(metadata)?;
            Ok(Some(Arc::new(table)))
        } else if name == catalog::BUNDLE_VIEWS_TABLE {
            let views = self.views.read().clone();
            let table = BundleViewsTable::new(views)?;
            Ok(Some(Arc::new(table)))
        } else if name == catalog::BUNDLE_INDEXES_TABLE {
            let indexes = self.indexes.read().clone();
            let table = BundleIndexesTable::new(indexes)?;
            Ok(Some(Arc::new(table)))
        } else if name == catalog::BUNDLE_PACKS_TABLE {
            let packs = self.packs.read().clone();
            let table = BundlePacksTable::new(packs)?;
            Ok(Some(Arc::new(table)))
        } else if name == catalog::BUNDLE_BLOCKS_TABLE {
            let packs = self.packs.read().clone();
            let table = BundleBlocksTable::new(packs)?;
            Ok(Some(Arc::new(table)))
        } else {
            Ok(None)
        }
    }

    fn table_exist(&self, name: &str) -> bool {
        name == catalog::BUNDLE_HISTORY_TABLE
            || name == catalog::BUNDLE_STATUS_TABLE
            || name == catalog::BUNDLE_DETAILS_TABLE
            || name == catalog::BUNDLE_VIEWS_TABLE
            || name == catalog::BUNDLE_INDEXES_TABLE
            || name == catalog::BUNDLE_PACKS_TABLE
            || name == catalog::BUNDLE_BLOCKS_TABLE
    }
}

/// Helper struct for creating the bundle_history table
struct BundleHistoryTable;

impl BundleHistoryTable {
    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("url", DataType::Utf8, true),
            Field::new("author", DataType::Utf8, false),
            Field::new("message", DataType::Utf8, false),
            Field::new("timestamp", DataType::Utf8, false),
            Field::new("change_count", DataType::Int32, false),
        ]))
    }

    fn new(commits: Vec<BundleCommit>) -> Result<MemTable, DataFusionError> {
        let schema = Self::schema();

        // Build arrays from commits
        let ids: Vec<i32> = (0..commits.len() as i32).collect();
        let urls: Vec<Option<String>> = commits
            .iter()
            .map(|c| c.url.as_ref().map(|u| u.to_string()))
            .collect();
        let authors: Vec<&str> = commits.iter().map(|c| c.author.as_str()).collect();
        let messages: Vec<&str> = commits.iter().map(|c| c.message.as_str()).collect();
        let timestamps: Vec<&str> = commits.iter().map(|c| c.timestamp.as_str()).collect();
        let change_counts: Vec<i32> = commits
            .iter()
            .map(|c| c.changes.len() as i32)
            .collect();

        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(Int32Array::from(ids)),
                Arc::new(StringArray::from(urls)),
                Arc::new(StringArray::from(authors)),
                Arc::new(StringArray::from(messages)),
                Arc::new(StringArray::from(timestamps)),
                Arc::new(Int32Array::from(change_counts)),
            ],
        )?;

        let batches = if commits.is_empty() {
            // Return empty batch with schema (one partition with zero rows)
            let empty_batch = RecordBatch::new_empty(Arc::clone(&schema));
            vec![vec![empty_batch]]
        } else {
            vec![vec![batch]]
        };

        MemTable::try_new(schema, batches)
    }
}

/// Helper struct for creating the bundle_status table
struct BundleStatusTable;

impl BundleStatusTable {
    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("change_id", DataType::Utf8, false),
            Field::new("description", DataType::Utf8, false),
            Field::new("operation_count", DataType::Int32, false),
        ]))
    }

    fn new(status: BundleStatus) -> Result<MemTable, DataFusionError> {
        let schema = Self::schema();
        let changes = status.changes();

        // Build arrays from changes
        let ids: Vec<i32> = (0..changes.len() as i32).collect();
        let change_ids: Vec<String> = changes.iter().map(|c| c.id.to_string()).collect();
        let descriptions: Vec<&str> = changes.iter().map(|c| c.description.as_str()).collect();
        let operation_counts: Vec<i32> = changes
            .iter()
            .map(|c| c.operations.len() as i32)
            .collect();

        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(Int32Array::from(ids)),
                Arc::new(StringArray::from(change_ids.iter().map(|s| s.as_str()).collect::<Vec<_>>())),
                Arc::new(StringArray::from(descriptions)),
                Arc::new(Int32Array::from(operation_counts)),
            ],
        )?;

        let batches = if changes.is_empty() {
            // Return empty batch with schema (one partition with zero rows)
            let empty_batch = RecordBatch::new_empty(Arc::clone(&schema));
            vec![vec![empty_batch]]
        } else {
            vec![vec![batch]]
        };

        MemTable::try_new(schema, batches)
    }
}

/// Helper struct for creating the bundle_details table
struct BundleDetailsTable;

impl BundleDetailsTable {
    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("name", DataType::Utf8, true),
            Field::new("description", DataType::Utf8, true),
            Field::new("url", DataType::Utf8, false),
            Field::new("from", DataType::Utf8, true),
            Field::new("version", DataType::Utf8, false),
        ]))
    }

    fn new(metadata: BundleMetadata) -> Result<MemTable, DataFusionError> {
        let schema = Self::schema();

        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(StringArray::from(vec![metadata.id.as_str()])),
                Arc::new(StringArray::from(vec![metadata.name.as_deref()])),
                Arc::new(StringArray::from(vec![metadata.description.as_deref()])),
                Arc::new(StringArray::from(vec![metadata.url.as_str()])),
                Arc::new(StringArray::from(vec![metadata.from.as_ref().map(|u| u.as_str())])),
                Arc::new(StringArray::from(vec![metadata.version.as_str()])),
            ],
        )?;

        MemTable::try_new(schema, vec![vec![batch]])
    }
}

/// Helper struct for creating the bundle_views table
struct BundleViewsTable;

impl BundleViewsTable {
    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("name", DataType::Utf8, false),
        ]))
    }

    fn new(views: HashMap<String, ObjectId>) -> Result<MemTable, DataFusionError> {
        let schema = Self::schema();

        // Sort views by name for consistent ordering
        let mut view_list: Vec<_> = views.iter().collect();
        view_list.sort_by_key(|(name, _)| *name);

        let ids: Vec<String> = view_list.iter().map(|(_, id)| id.to_string()).collect();
        let names: Vec<&str> = view_list.iter().map(|(name, _)| name.as_str()).collect();

        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(StringArray::from(ids.iter().map(|s| s.as_str()).collect::<Vec<_>>())),
                Arc::new(StringArray::from(names)),
            ],
        )?;

        let batches = if views.is_empty() {
            let empty_batch = RecordBatch::new_empty(Arc::clone(&schema));
            vec![vec![empty_batch]]
        } else {
            vec![vec![batch]]
        };

        MemTable::try_new(schema, batches)
    }
}

/// Helper struct for creating the bundle_indexes table
struct BundleIndexesTable;

impl BundleIndexesTable {
    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("column", DataType::Utf8, false),
            Field::new("type", DataType::Utf8, false),
            Field::new("tokenizer", DataType::Utf8, true),
        ]))
    }

    fn new(indexes: Vec<Arc<IndexDefinition>>) -> Result<MemTable, DataFusionError> {
        let schema = Self::schema();

        let ids: Vec<String> = indexes.iter().map(|idx| idx.id().to_string()).collect();
        let columns: Vec<&str> = indexes.iter().map(|idx| idx.column().as_str()).collect();
        let types: Vec<&str> = indexes
            .iter()
            .map(|idx| {
                if idx.is_text() {
                    "text"
                } else {
                    "column"
                }
            })
            .collect();
        let tokenizers: Vec<Option<String>> = indexes
            .iter()
            .map(|idx| {
                idx.index_type()
                    .tokenizer()
                    .map(|t| t.tantivy_tokenizer_name().to_string())
            })
            .collect();

        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(StringArray::from(ids.iter().map(|s| s.as_str()).collect::<Vec<_>>())),
                Arc::new(StringArray::from(columns)),
                Arc::new(StringArray::from(types)),
                Arc::new(StringArray::from(
                    tokenizers
                        .iter()
                        .map(|t| t.as_deref())
                        .collect::<Vec<_>>(),
                )),
            ],
        )?;

        let batches = if indexes.is_empty() {
            let empty_batch = RecordBatch::new_empty(Arc::clone(&schema));
            vec![vec![empty_batch]]
        } else {
            vec![vec![batch]]
        };

        MemTable::try_new(schema, batches)
    }
}

/// Helper struct for creating the bundle_packs table
struct BundlePacksTable;

impl BundlePacksTable {
    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("name", DataType::Utf8, false),
            Field::new("join_type", DataType::Utf8, true),
            Field::new("expression", DataType::Utf8, true),
        ]))
    }

    fn new(packs: HashMap<ObjectId, Arc<Pack>>) -> Result<MemTable, DataFusionError> {
        let schema = Self::schema();

        // Sort packs by ID for consistent ordering
        let mut pack_list: Vec<_> = packs.values().collect();
        pack_list.sort_by_key(|p| *p.id());

        let ids: Vec<String> = pack_list.iter().map(|p| p.id().to_string()).collect();
        let names: Vec<&str> = pack_list.iter().map(|p| p.name()).collect();
        let join_types: Vec<Option<&str>> = pack_list
            .iter()
            .map(|p| p.join_type().map(|jt| jt.as_str()))
            .collect();
        let expressions: Vec<Option<&str>> = pack_list
            .iter()
            .map(|p| p.expression())
            .collect();

        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(StringArray::from(ids.iter().map(|s| s.as_str()).collect::<Vec<_>>())),
                Arc::new(StringArray::from(names)),
                Arc::new(StringArray::from(join_types)),
                Arc::new(StringArray::from(expressions)),
            ],
        )?;

        let batches = if packs.is_empty() {
            let empty_batch = RecordBatch::new_empty(Arc::clone(&schema));
            vec![vec![empty_batch]]
        } else {
            vec![vec![batch]]
        };

        MemTable::try_new(schema, batches)
    }
}

/// Helper struct for creating the bundle_blocks table
struct BundleBlocksTable;

impl BundleBlocksTable {
    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("version", DataType::Utf8, false),
            Field::new("pack_id", DataType::Utf8, false),
            Field::new("pack_name", DataType::Utf8, false),
            Field::new("source_id", DataType::Utf8, true),
            Field::new("source_location", DataType::Utf8, true),
            Field::new("source_version", DataType::Utf8, true),
        ]))
    }

    fn new(packs: HashMap<ObjectId, Arc<Pack>>) -> Result<MemTable, DataFusionError> {
        let schema = Self::schema();

        // Collect all blocks from all packs
        let mut blocks: Vec<(Arc<DataBlock>, ObjectId, String)> = Vec::new();
        for pack in packs.values() {
            let pack_id = *pack.id();
            let pack_name = pack.name().to_string();
            for block in pack.blocks() {
                blocks.push((block, pack_id, pack_name.clone()));
            }
        }

        // Sort blocks by ID for consistent ordering
        blocks.sort_by_key(|(b, _, _)| *b.id());

        let ids: Vec<String> = blocks.iter().map(|(b, _, _)| b.id().to_string()).collect();
        let versions: Vec<String> = blocks.iter().map(|(b, _, _)| b.version()).collect();
        let pack_ids: Vec<String> = blocks.iter().map(|(_, pid, _)| pid.to_string()).collect();
        let pack_names: Vec<&str> = blocks.iter().map(|(_, _, pn)| pn.as_str()).collect();
        let source_ids: Vec<Option<String>> = blocks
            .iter()
            .map(|(b, _, _)| b.source_info().map(|si| si.id.to_string()))
            .collect();
        let source_locations: Vec<Option<&str>> = blocks
            .iter()
            .map(|(b, _, _)| b.source_info().map(|si| si.location.as_str()))
            .collect();
        let source_versions: Vec<Option<&str>> = blocks
            .iter()
            .map(|(b, _, _)| b.source_info().map(|si| si.version.as_str()))
            .collect();

        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(StringArray::from(ids.iter().map(|s| s.as_str()).collect::<Vec<_>>())),
                Arc::new(StringArray::from(versions.iter().map(|s| s.as_str()).collect::<Vec<_>>())),
                Arc::new(StringArray::from(pack_ids.iter().map(|s| s.as_str()).collect::<Vec<_>>())),
                Arc::new(StringArray::from(pack_names)),
                Arc::new(StringArray::from(
                    source_ids.iter().map(|s| s.as_deref()).collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(source_locations)),
                Arc::new(StringArray::from(source_versions)),
            ],
        )?;

        let batches = if blocks.is_empty() {
            let empty_batch = RecordBatch::new_empty(Arc::clone(&schema));
            vec![vec![empty_batch]]
        } else {
            vec![vec![batch]]
        };

        MemTable::try_new(schema, batches)
    }
}
