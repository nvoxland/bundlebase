mod builder;
mod column_lineage;
mod commit;
mod facade;
mod init;
mod operation;
mod sql;

use crate::io::EMPTY_SCHEME;
pub use builder::{BundleBuilder, BundleStatus};
pub use column_lineage::{ColumnLineageAnalyzer, ColumnSource};
pub use commit::BundleCommit;
pub use facade::BundleFacade;
pub use init::{InitCommit, INIT_FILENAME};
pub use operation::JoinTypeOption;
pub use operation::{AnyOperation, BundleChange, Operation};
use std::collections::{HashMap, HashSet};

use crate::bundle::commit::manifest_version;
use crate::data::{DataPack, DataReaderFactory, ObjectId, PackJoin, VersionedBlockId};
use crate::io::{DataStorage, ObjectStoreDir, ObjectStoreFile, EMPTY_URL};
use crate::functions::FunctionRegistry;
use crate::index::{IndexDefinition, IndexedBlocks};
use crate::catalog::{BlockSchemaProvider, BundleSchemaProvider, PackSchemaProvider, CATALOG_NAME};
use crate::BundlebaseError;
use arrow::array::Array;
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use datafusion::catalog::MemorySchemaProvider;
use datafusion::common::{DFSchema, DFSchemaRef};
use datafusion::datasource::object_store::ObjectStoreUrl;
use datafusion::logical_expr::{EmptyRelation, ExplainFormat, ExplainOption, LogicalPlan};
use datafusion::prelude::*;
use datafusion::scalar::ScalarValue;
use log::debug;
use parking_lot::RwLock;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use url::Url;
use uuid::Uuid;

/// A read-only view of a Bundle loaded from persistent storage.
///
/// `Bundle` represents a bundle that has been committed and persisted to disk.
/// It is immutable in the sense that it reflects a fixed state from storage, though operations
/// can be applied by extending it with `BundleBuilder`.
///
/// # Manifest Chain Loading
/// When opening a bundle, all parent bundles referenced by the `from` field are loaded
/// recursively, establishing a complete inheritance chain. This allows bundles to build
/// upon previously committed versions.
pub struct Bundle {
    id: String,
    name: Option<String>,
    description: Option<String>,
    version: String,
    last_manifest_version: u32,

    data_dir: ObjectStoreDir,
    commits: Vec<BundleCommit>,
    operations: Vec<AnyOperation>,

    data_packs: Arc<RwLock<HashMap<ObjectId, Arc<DataPack>>>>,
    base_pack: Option<ObjectId>,
    joins: HashMap<String, PackJoin>,
    indexes: Arc<RwLock<Vec<Arc<IndexDefinition>>>>,
    dataframe: DataFrameHolder,

    ctx: Arc<SessionContext>,
    storage: Arc<DataStorage>,
    adapter_factory: Arc<DataReaderFactory>,
    function_registry: Arc<RwLock<FunctionRegistry>>,
}

impl Clone for Bundle {
    fn clone(&self) -> Self {
        // Deep clone indexes and function_registry for independence
        // Share data_packs to maintain compatibility with SessionContext schema providers
        let indexes = {
            let idxs = self.indexes.read();
            Arc::new(RwLock::new(idxs.clone()))
        };

        let function_registry = {
            let registry = self.function_registry.read();
            Arc::new(RwLock::new(registry.clone()))
        };

        Self {
            id: self.id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            data_dir: self.data_dir.clone(),
            commits: self.commits.clone(),
            operations: self.operations.clone(),
            version: self.version.clone(),
            last_manifest_version: self.last_manifest_version,
            base_pack: self.base_pack.clone(),
            data_packs: Arc::clone(&self.data_packs),
            joins: self.joins.clone(),
            indexes,
            dataframe: DataFrameHolder {
                dataframe: Arc::new(RwLock::new(self.dataframe.dataframe.read().clone()))
            },
            ctx: Arc::clone(&self.ctx),
            storage: Arc::clone(&self.storage),
            adapter_factory: Arc::clone(&self.adapter_factory),
            function_registry,
        }
    }
}

impl Bundle {
    pub async fn empty() -> Result<Self, BundlebaseError> {
        let url = Url::parse(EMPTY_URL)?;

        let storage = Arc::new(DataStorage::new());
        let function_registry = Arc::new(RwLock::new(FunctionRegistry::new()));

        let mut config = SessionConfig::new().with_default_catalog_and_schema(CATALOG_NAME, "public");
        let options = config.options_mut();
        options.sql_parser.enable_ident_normalization = false;
        let ctx = Arc::new(SessionContext::new_with_config(config));

        // Create data_packs bundle and dataframe cache
        let data_packs = Arc::new(RwLock::new(HashMap::new()));

        let empty_dataframe = DataFrame::new(
            ctx.state(),
            LogicalPlan::EmptyRelation(EmptyRelation {
                produce_one_row: false,
                schema: DFSchemaRef::new(DFSchema::empty()),
            }),
        );

        let dataframe = DataFrameHolder::new(Some(empty_dataframe));

        // Register schema providers
        let catalog = ctx
            .catalog(CATALOG_NAME)
            .expect("Default catalog not found");
        catalog.register_schema(
            "blocks",
            Arc::new(BlockSchemaProvider::new(data_packs.clone())),
        )?;
        catalog.register_schema(
            "packs",
            Arc::new(PackSchemaProvider::new(data_packs.clone())),
        )?;
        catalog.register_schema(
            "public",
            Arc::new(BundleSchemaProvider::new(dataframe.clone())),
        )?;
        catalog.register_schema("temp", Arc::new(MemorySchemaProvider::new()))?;

        ctx.register_object_store(
            ObjectStoreUrl::parse("memory://")?.as_ref(),
            crate::io::get_memory_store(),
        );
        ctx.register_object_store(
            ObjectStoreUrl::parse(format!("{}://", EMPTY_SCHEME))?.as_ref(),
            crate::io::get_null_store(),
        );

        Ok(Self {
            ctx,
            id: Uuid::new_v4().to_string(),
            base_pack: None,
            data_packs,
            joins: HashMap::new(),
            indexes: Arc::new(RwLock::new(Vec::new())),
            storage: Arc::clone(&storage),
            adapter_factory: DataReaderFactory::new(
                Arc::clone(&function_registry),
                Arc::clone(&storage),
            )
            .into(),
            function_registry,
            name: None,
            description: None,
            operations: vec![],

            last_manifest_version: 0,
            version: "empty".to_string(),
            data_dir: ObjectStoreDir::from_url(&url)?,
            commits: vec![],
            dataframe,
        })
    }

    /// Loads a read-only Bundle from persistent storage.
    ///
    /// # Arguments
    /// * `path` - Path to the bundle to open. Can be a URL (e.g., `file:///path/to/bundle`, `s3://bucket/bundle`) OR a filesystem path (relative or absolute)
    ///
    /// # Process
    /// 1. Reads the manifest directory to find committed operations
    /// 2. If the manifest references a parent bundle (via `from` field), loads it recursively
    /// 3. Establishes the complete inheritance chain
    /// 4. Initializes the DataFusion session context with the bundle schema
    ///
    /// # Example
    /// let bundle = Bundle::open("file:///data/my_bundle").await?;
    /// let schema = bundle.schema();
    /// ```
    pub async fn open(path: &str) -> Result<Self, BundlebaseError> {
        let mut visited = HashSet::new();
        let mut bundle = Bundle::empty().await?;
        Self::open_internal(
            ObjectStoreDir::from_str(path)?.url().as_str(),
            &mut visited,
            &mut bundle,
        )
        .await?;

        Ok(bundle)
    }

    /// Internal implementation of open() that tracks visited URLs to detect cycles
    async fn open_internal(
        url: &str,
        visited: &mut HashSet<String>,
        bundle: &mut Bundle,
    ) -> Result<(), BundlebaseError> {
        if !visited.insert(url.to_string()) {
            return Err(format!(
                "Circular dependency detected in bundle from chain: {}",
                url
            )
            .into());
        }

        let data_dir = ObjectStoreDir::from_str(url)?;
        let manifest_dir = data_dir.subdir("_manifest")?;

        let init_commit: Option<InitCommit> = manifest_dir.file(INIT_FILENAME)?.read_yaml().await?;
        let init_commit =
            init_commit.expect(format!("No _manifest/{} found in {}", INIT_FILENAME, url).as_str());

        // Recursively load the base bundle and store the Arc reference
        if let Some(from_url) = &init_commit.from {
            // Box the recursive call to avoid infinite future size
            Box::pin(Self::open_internal(from_url.as_str(), visited, bundle)).await?;
        };
        bundle.id = init_commit.id;
        bundle.data_dir = data_dir.clone();

        // List files in the manifest directory
        let manifest_files = manifest_dir.list_files().await?;

        let manifest_files = manifest_files
            .iter()
            .filter(|x| x.filename() != INIT_FILENAME)
            .collect::<Vec<_>>();

        if manifest_files.is_empty() {
            return Err(format!("No data bundle in: {}", url).into());
        }

        // Load and apply each manifest in order
        for manifest_file in manifest_files {
            bundle.last_manifest_version = manifest_version(manifest_file.filename());
            let mut commit: BundleCommit = manifest_file.read_yaml().await?.unwrap();
            commit.url = Some(manifest_file.url().clone());
            commit.data_dir = Some(data_dir.url().clone());

            bundle.commits.push(commit.clone());

            // Apply operations from this manifest's changes
            for change in commit.changes {
                for op in change.operations {
                    bundle.apply_operation(op).await?;
                }
            }
        }
        Ok(())
    }

    /// Creates a BundleBuilder that extends this bundle.
    /// Will store the new bundle in the passed data_dir.
    pub fn extend(&self, data_dir: &str) -> Result<BundleBuilder, BundlebaseError> {
        BundleBuilder::extend(Arc::new(self.clone()), data_dir)
    }

    /// Modifies this bundle with the given operation
    async fn apply_operation(&mut self, op: AnyOperation) -> Result<(), BundlebaseError> {
        let description = &op.describe();
        debug!("Applying operation to bundle: {}...", &description);

        debug!("Checking: {}", &description);
        op.check(self).await?;

        debug!("Apply: {}", &description);
        op.apply(self).await?;
        self.operations.push(op);

        self.compute_version();
        // clear cached values
        self.dataframe.clear();
        debug!("Cleared dataframe");

        debug!("Applying operation to bundle: {}...DONE", &description);

        Ok(())
    }

    pub fn data_dir(&self) -> &ObjectStoreDir {
        &self.data_dir
    }

    /// Opens a file relative to the bundle's data directory.
    ///
    /// # Arguments
    /// * `path` - Path relative to data_dir, or a full URL
    fn file(&self, path: &str) -> Result<ObjectStoreFile, BundlebaseError> {
        ObjectStoreFile::from_str(path, self.data_dir())
    }

    pub fn ctx(&self) -> Arc<SessionContext> {
        self.ctx.clone()
    }

    /// All operations applied to this bundle
    pub fn operations(&self) -> &Vec<AnyOperation> {
        &self.operations
    }

    pub async fn explain(&self) -> Result<String, BundlebaseError> {
        let mut result = String::new();

        let df = (*self.dataframe().await?).clone();
        let plan = df.explain_with_options(ExplainOption {
            verbose: false,
            analyze: false,
            format: ExplainFormat::Indent,
        })?;
        let records = plan.collect().await?;

        for batch in records {
            let plan_type_column = batch.column(0);
            let plan_column = batch.column(1);

            if let (Some(plan_type_array), Some(plan_array)) = (
                plan_type_column
                    .as_any()
                    .downcast_ref::<arrow::array::StringArray>(),
                plan_column
                    .as_any()
                    .downcast_ref::<arrow::array::StringArray>(),
            ) {
                for i in 0..plan_type_column.len() {
                    if !plan_type_column.is_null(i) && !plan_column.is_null(i) {
                        let plan_type = plan_type_array.value(i);
                        let plan_text = plan_array.value(i);
                        result.push_str(&format!("\n*** {} ***\n{}\n", plan_type, plan_text));
                    }
                }
            }
        }
        Ok(result.trim().to_string())
    }

    /// Joins the pack
    async fn dataframe_join(
        &self,
        base_df: DataFrame,
        pack_join: &PackJoin,
    ) -> Result<DataFrame, BundlebaseError> {
        let base_table = format!(
            "packs.{}",
            DataPack::table_name(&self.base_pack.expect("Missing base pack"))
        );
        let join_table = format!("packs.{}", DataPack::table_name(pack_join.pack_id()));

        let expr = sql::parse_join_expr(&self.ctx, &base_table, pack_join).await?;

        Ok(base_df.join_on(
            self.ctx.table(&join_table).await?.alias(pack_join.name())?,
            pack_join.join_type().to_datafusion(),
            expr,
        )?)
    }

    fn compute_version(&mut self) {
        let mut hasher = Sha256::new();

        for op in self.operations.iter() {
            hasher.update(op.version().as_bytes());
        }

        self.version = hex::encode(hasher.finalize())[0..12].to_string();
    }

    pub(crate) fn add_pack(&self, pack_id: ObjectId, pack: Arc<DataPack>) {
        self.data_packs.write().insert(pack_id, pack);
    }

    pub(crate) fn get_pack(&self, pack_id: &ObjectId) -> Option<Arc<DataPack>> {
        self.data_packs.read().get(pack_id).cloned()
    }

    /// Get read access to the indexes list
    pub(crate) fn indexes(&self) -> &Arc<RwLock<Vec<Arc<IndexDefinition>>>> {
        &self.indexes
    }

    /// Check if an index already exists at the correct version
    pub(crate) fn get_index(&self, column: &str, block: &VersionedBlockId) -> Option<Arc<IndexedBlocks>> {
        for index in self.indexes.read().iter() {
            if index.column() == column {
                let indexed_blocks = index.indexed_blocks(block);

                if indexed_blocks.is_some() {
                    return Some(indexed_blocks.unwrap())
                }
            }
        }
        None
    }
}

#[async_trait]
impl BundleFacade for Bundle {
    fn id(&self) -> &str {
        &self.id
    }

    /// Retrieve the bundle name, if set.
    fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Retrieve the bundle description, if set.
    fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Retrieve the URL of the base bundle this was loaded from, if any.
    fn url(&self) -> &Url {
        &self.data_dir.url()
    }

    fn from(&self) -> Option<&Url> {
        self.commits
            .iter()
            .filter(|x| x.data_dir != Some(self.data_dir.url().clone()))
            .last()
            .map(|c| c.data_dir.as_ref().unwrap())
    }

    fn version(&self) -> String {
        self.version.clone()
    }

    /// Returns the commit history for this bundle, starting with any base bundles
    fn history(&self) -> Vec<BundleCommit> {
        self.commits.clone()
    }

    async fn schema(&self) -> Result<SchemaRef, BundlebaseError> {
        Ok(Arc::new(
            self.dataframe().await?.schema().clone().as_arrow().clone(),
        ))
    }

    async fn num_rows(&self) -> Result<usize, BundlebaseError> {
        (*self.dataframe().await?)
            .clone()
            .count()
            .await
            .map_err(|e| e.into())
    }

    async fn dataframe(&self) -> Result<Arc<DataFrame>, BundlebaseError> {
        // Check cache first
        if let Some(df) = self.dataframe.maybe_dataframe() {
            debug!("dataframe: Using cached dataframe");
            return Ok(df);
        }

        debug!("Building dataframe...");
        let df = match self.base_pack {
            Some(base_pack) => {
                let mut df = self
                    .ctx
                    .table(&format!("packs.{}", DataPack::table_name(&base_pack)))
                    .await?;

                for (_, pack_join) in &self.joins {
                    debug!("Executing join with pack {}", pack_join.pack_id());
                    df = self.dataframe_join(df, pack_join).await?;
                }

                // Apply operations to the base DataFrame
                debug!("dataframe: Applying {} operations to dataframe...", self.operations().len());

                for op in self.operations().iter() {
                    debug!("Applying to dataframe: {}", &op.describe());
                    df = op.apply_dataframe(df, self.ctx.clone()).await?;
                }
                debug!("dataframe: Applying {} operations to dataframe...DONE", self.operations().len());

                df
            }
            None => {
                debug!("No base pack, using empty dataframe");
                DataFrame::new(
                    self.ctx().state(),
                    LogicalPlan::EmptyRelation(EmptyRelation {
                        produce_one_row: false,
                        schema: DFSchemaRef::new(DFSchema::empty()),
                    }),
                )
            },
        };
        self.dataframe.replace(df);
        debug!("Building dataframe...DONE");
        Ok(self.dataframe.dataframe())
    }

    async fn query(
        &self,
        sql: &str,
        params: Vec<ScalarValue>,
    ) -> Result<BundleBuilder, BundlebaseError> {
        let bundle =
            BundleBuilder::extend(Arc::new(self.clone()), &self.data_dir.url().as_str())?;
        bundle.query(sql, params).await
    }
}

#[derive(Debug)]
pub struct DataFrameHolder {
    pub(crate) dataframe: Arc<RwLock<Option<Arc<DataFrame>>>>,
}

impl DataFrameHolder {
    fn new(df: Option<DataFrame>) -> Self {
        Self {
            dataframe: Arc::new(RwLock::new(df.map(|df| Arc::new(df)))),
        }
    }

    pub fn dataframe(&self) -> Arc<DataFrame> {
        self.dataframe.read().clone().expect("Dataframe not ready")
    }

    fn maybe_dataframe(&self) -> Option<Arc<DataFrame>> {
        self.dataframe.read().clone()
    }

    pub fn replace(&self, df: DataFrame) -> Arc<DataFrame> {
        self.dataframe.write().replace(Arc::new(df));
        self.dataframe.read().clone().expect("Dataframe not ready")
    }

    fn clear(&self) {
        let mut guard = self.dataframe.write();
        *guard = None;
    }
}

impl Clone for DataFrameHolder {
    fn clone(&self) -> Self {
        Self {
            dataframe: Arc::clone(&self.dataframe)
        }
    }
}

/// Convert a DataFusion ScalarValue to a SQL literal string
pub fn scalar_value_to_sql_literal(value: &ScalarValue) -> String {
    match value {
        ScalarValue::Null => "NULL".to_string(),
        ScalarValue::Boolean(Some(b)) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        ScalarValue::Boolean(None) => "NULL".to_string(),
        ScalarValue::Int8(Some(i)) => i.to_string(),
        ScalarValue::Int8(None) => "NULL".to_string(),
        ScalarValue::Int16(Some(i)) => i.to_string(),
        ScalarValue::Int16(None) => "NULL".to_string(),
        ScalarValue::Int32(Some(i)) => i.to_string(),
        ScalarValue::Int32(None) => "NULL".to_string(),
        ScalarValue::Int64(Some(i)) => i.to_string(),
        ScalarValue::Int64(None) => "NULL".to_string(),
        ScalarValue::UInt8(Some(i)) => i.to_string(),
        ScalarValue::UInt8(None) => "NULL".to_string(),
        ScalarValue::UInt16(Some(i)) => i.to_string(),
        ScalarValue::UInt16(None) => "NULL".to_string(),
        ScalarValue::UInt32(Some(i)) => i.to_string(),
        ScalarValue::UInt32(None) => "NULL".to_string(),
        ScalarValue::UInt64(Some(i)) => i.to_string(),
        ScalarValue::UInt64(None) => "NULL".to_string(),
        ScalarValue::Float32(Some(f)) => f.to_string(),
        ScalarValue::Float32(None) => "NULL".to_string(),
        ScalarValue::Float64(Some(f)) => f.to_string(),
        ScalarValue::Float64(None) => "NULL".to_string(),
        ScalarValue::Utf8(Some(s)) => {
            // Escape single quotes by doubling them (SQL standard)
            let escaped = s.replace("'", "''");
            format!("'{}'", escaped)
        }
        ScalarValue::Utf8(None) => "NULL".to_string(),
        // For other types, convert to string representation
        _ => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::operation::SetNameOp;

    #[tokio::test]
    async fn test_version() -> Result<(), BundlebaseError> {
        let mut c = Bundle::empty().await?;
        assert_eq!(c.version(), "empty".to_string());

        c.apply_operation(AnyOperation::SetName(SetNameOp {
            name: "New Name".to_string(),
        }))
        .await?;

        assert_eq!(c.version(), "ead23fcd0c25".to_string());

        c.apply_operation(AnyOperation::SetName(SetNameOp {
            name: "Other Name".to_string(),
        }))
        .await?;

        assert_eq!(c.version(), "b4ef54330e9a".to_string());

        Ok(())
    }
}
