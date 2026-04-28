pub mod block_cache;
mod builder;
pub mod bundle_schema;
pub mod command_metadata;
mod commit;
mod data_block;
pub mod export;
mod facade;
mod indexed_blocks;
mod init;
pub mod operation;
mod pack;
mod source;
pub mod connector_entry {
    pub use bundlebase_udf::{parse_connector_name, resolve_connector, ConnectorEntry};
}
pub mod function_entry {
    pub use bundlebase_udf::{
        internal_function_name, parse_function_name, validate_kind_consistency, FunctionEntry,
        FunctionKind, FunctionRegistry,
    };
}
pub mod deleted_row_filter;
pub mod schema_rename_filter;
mod sql;
pub mod tombstone;
pub mod update_overlay;
pub mod update_overlay_filter;
pub mod verification;

pub use crate::arrow_types::parse_arrow_type_name;
use crate::io::EMPTY_SCHEME;
pub use crate::platform::Platform;
pub use crate::udf::UdfRuntime;
pub use builder::BundleBuilder;
pub use builder::BundleStatus;
pub use bundlebase_common::command_response::{CommandResponse, OutputShape};
pub use commit::{manifest_version, BundleCommit, CommitHistory};
pub use connector_entry::ConnectorEntry;
pub use data_block::DataBlock;
pub use facade::BundleFacade;
pub use function_entry::{
    validate_kind_consistency, FunctionEntry, FunctionKind, FunctionRegistry,
};
pub use indexed_blocks::IndexedBlocks;
pub use init::{InitCommit, INIT_FILENAME};
pub use operation::{
    resolve_cast_ops, AnyOperation, BundleChange, CreateSourceOp, EmptyContext, ExpectedColumn,
    Operation,
};
pub use pack::JoinTypeOption;
pub use pack::Pack;
pub use source::Source;
use std::collections::{HashMap, HashSet};
pub use verification::{FileVerificationResult, VerificationResults};

use crate::bundle::bundle_schema::BundleSchema;
use crate::bundle_config::PassedBundleConfig;
use crate::bundle_config::Scope;
use crate::catalog::CATALOG_NAME;
use crate::data::{BlockId, DataReaderFactory, ObjectId, VersionedBlockId};
use crate::function::VersionFunction;
use crate::index::IndexDefinition;
use crate::index::SearchTableFunction;
use crate::io::{
    read_yaml, readable_file_from_url, writable_dir_from_str, writable_dir_from_url, DataStorage,
    IOReadWriteDir, EMPTY_URL,
};
use crate::object_id::ColumnId;
use crate::source::ConnectorRegistry;
use crate::ConfigProvider;
use crate::{BundleConfig, BundlebaseError};
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use datafusion::datasource::object_store::ObjectStoreUrl;
use datafusion::execution::SendableRecordBatchStream;
use datafusion::logical_expr::{LogicalPlan, ScalarUDF};
use datafusion::prelude::*;
use datafusion::scalar::ScalarValue;
use log::{debug, info};
use parking_lot::RwLock;
use sha2::{Digest, Sha256};
use std::sync::{Arc, Weak};
use url::Url;
use uuid::Uuid;
pub static META_DIR: &str = "_bundlebase";

/// A stored report template with markdown content.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub content: String,
}

/// A persistent always-update rule: SET assignments + WHERE clause.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlwaysUpdateRule {
    pub set_clause: String,
    pub where_clause: String,
}

impl AlwaysUpdateRule {
    pub fn new(set_clause: impl Into<String>, where_clause: impl Into<String>) -> Self {
        Self {
            set_clause: set_clause.into(),
            where_clause: where_clause.into(),
        }
    }

    /// Returns the canonical text representation used for matching in DROP.
    pub fn rule_text(&self) -> String {
        format!("SET {} WHERE {}", self.set_clause, self.where_clause)
    }
}

/// A thread-safe Bundle loaded from persistent storage.
///
/// `Bundle` represents a bundle that has been committed and persisted to disk.
/// All mutable fields use interior mutability via `Arc<RwLock<T>>` to enable
/// thread-safe access without requiring `&mut self`.
///
/// # Manifest Chain Loading
/// When opening a bundle, all parent bundles referenced by the `from` field are loaded
/// recursively, establishing a complete inheritance chain. This allows bundles to build
/// upon previously committed versions.
pub struct Bundle {
    id: Arc<RwLock<String>>,
    name: Arc<RwLock<Option<String>>>,
    description: Arc<RwLock<Option<String>>>,
    version: Arc<RwLock<String>>,
    last_manifest_version: Arc<RwLock<u32>>,

    data_dir: Arc<RwLock<Arc<dyn IOReadWriteDir>>>,
    commits: Arc<RwLock<Vec<BundleCommit>>>,

    pub operations: Arc<RwLock<Vec<AnyOperation>>>,

    packs: Arc<RwLock<HashMap<ObjectId, Arc<Pack>>>>,
    sources: Arc<RwLock<HashMap<ObjectId, Arc<Source>>>>,
    indexes: Arc<RwLock<Vec<Arc<IndexDefinition>>>>,
    views: Arc<RwLock<HashMap<String, ObjectId>>>,
    reports: Arc<RwLock<HashMap<String, ReportEntry>>>,
    dataframe: DataFrameHolder,
    bundle_schema: Arc<RwLock<BundleSchema>>,

    ctx: Arc<SessionContext>,
    storage: Arc<DataStorage>,
    pub reader_factory: Arc<DataReaderFactory>,
    connector_registry: Arc<RwLock<ConnectorRegistry>>,
    function_registry: Arc<RwLock<FunctionRegistry>>,
    subprocess_cache: crate::function::ipc_bridge::SubprocessCache,

    /// Single, self-contained, internally thread-safe config holder.
    /// All config sources (stored, env, passed, runtime) live inside BundleConfig.
    config: Arc<BundleConfig>,

    /// Persistent always-delete rules (WHERE clauses applied to each new attach)
    always_delete_rules: Arc<RwLock<Vec<String>>>,

    /// Persistent always-update rules (SET/WHERE clauses applied to each new attach)
    always_update_rules: Arc<RwLock<Vec<AlwaysUpdateRule>>>,

    /// Update overlays loaded from committed overlay parquet files.
    /// Later entries override earlier ones per-cell.
    update_overlays: Arc<RwLock<Vec<update_overlay::UpdateOverlay>>>,

    /// True if this bundle is a view (has a view field in init commit)
    is_view: Arc<RwLock<bool>>,

    /// Minimum bundlebase version required to open this bundle (major, minor)
    min_version: Arc<RwLock<Option<(u16, u16)>>>,
    /// Maximum bundlebase version allowed to open this bundle (major, minor)
    max_version: Arc<RwLock<Option<(u16, u16)>>>,

    /// Running SHA-256 hasher for incremental version computation.
    /// Feeds each operation's version hash as it's applied, avoiding O(n) re-iteration.
    version_hasher: Arc<RwLock<Sha256>>,
}

impl bundlebase_data::DataContext for Bundle {
    fn config_provider(&self) -> Arc<dyn ConfigProvider> {
        Bundle::config(self) as Arc<dyn ConfigProvider>
    }

    fn data_context_dir(&self) -> Arc<dyn IOReadWriteDir> {
        Bundle::data_dir(self)
    }

    fn session_context(&self) -> Arc<SessionContext> {
        Bundle::ctx(self)
    }
}

impl Clone for Bundle {
    /// Clone the bundle, sharing all Arc<RwLock<T>> state.
    ///
    /// This clone **shares** all Arc fields with the original. This means:
    /// - Both bundles see the same state for all mutable fields
    /// - Mutations in one clone are visible in the other
    ///
    /// This is intentional for internal operations where changes need to be
    /// reflected back through schema providers (BundleInfoSchemaProvider).
    ///
    /// # Shared Fields
    /// All Arc<RwLock<T>> fields are shared, enabling thread-safe mutations
    /// visible across clones.
    fn clone(&self) -> Self {
        Self {
            id: Arc::clone(&self.id),
            name: Arc::clone(&self.name),
            description: Arc::clone(&self.description),
            version: Arc::clone(&self.version),
            last_manifest_version: Arc::clone(&self.last_manifest_version),
            data_dir: Arc::clone(&self.data_dir),
            commits: Arc::clone(&self.commits),
            operations: Arc::clone(&self.operations),
            packs: Arc::clone(&self.packs),
            sources: Arc::clone(&self.sources),
            indexes: Arc::clone(&self.indexes),
            views: Arc::clone(&self.views),
            reports: Arc::clone(&self.reports),
            dataframe: DataFrameHolder {
                dataframe: Arc::new(RwLock::new(self.dataframe.dataframe.read().clone())),
            },
            bundle_schema: Arc::clone(&self.bundle_schema),
            ctx: Arc::clone(&self.ctx),
            storage: Arc::clone(&self.storage),
            reader_factory: Arc::clone(&self.reader_factory),
            connector_registry: Arc::clone(&self.connector_registry),
            function_registry: Arc::clone(&self.function_registry),
            subprocess_cache: Arc::clone(&self.subprocess_cache),
            config: Arc::clone(&self.config),
            always_delete_rules: Arc::clone(&self.always_delete_rules),
            always_update_rules: Arc::clone(&self.always_update_rules),
            update_overlays: Arc::clone(&self.update_overlays),
            is_view: Arc::clone(&self.is_view),
            min_version: Arc::clone(&self.min_version),
            max_version: Arc::clone(&self.max_version),
            version_hasher: Arc::clone(&self.version_hasher),
        }
    }
}

impl Bundle {
    /// Creates an empty bundle wrapped in Arc with schema providers registered.
    ///
    /// Returns `Arc<Self>` ready for use. Schema providers are registered with the
    /// Bundle as the facade. BundleBuilder will re-register with itself as facade.
    pub async fn empty(
        passed_config: Option<PassedBundleConfig>,
    ) -> Result<Arc<Self>, BundlebaseError> {
        let url = Url::parse(EMPTY_URL)?;

        let storage = Arc::new(DataStorage::new());
        let connector_registry = Arc::new(RwLock::new(ConnectorRegistry::new()));

        let mut config =
            SessionConfig::new().with_default_catalog_and_schema(CATALOG_NAME, "default");
        let options = config.options_mut();
        options.sql_parser.enable_ident_normalization = false;
        let ctx = Arc::new(SessionContext::new_with_config(config));

        let packs = Arc::new(RwLock::new(HashMap::new()));
        let commits = Arc::new(RwLock::new(vec![]));
        let indexes = Arc::new(RwLock::new(Vec::new()));
        let views = Arc::new(RwLock::new(HashMap::new()));
        let reports = Arc::new(RwLock::new(HashMap::new()));
        let sources = Arc::new(RwLock::new(HashMap::new()));
        let operations = Arc::new(RwLock::new(Vec::new()));

        let id = Arc::new(RwLock::new(Uuid::new_v4().to_string()));
        let name = Arc::new(RwLock::new(None));
        let description = Arc::new(RwLock::new(None));
        let version = Arc::new(RwLock::new("empty".to_string()));

        let empty_dataframe = no_data_dataframe(&ctx)?;

        let dataframe = DataFrameHolder::new(Some(empty_dataframe));

        // Register version() UDF with initial "empty" version
        ctx.register_udf(ScalarUDF::new_from_impl(VersionFunction::new(
            "empty".to_string(),
        )));

        ctx.register_object_store(
            ObjectStoreUrl::parse("memory://")?.as_ref(),
            crate::io::get_memory_store(),
        );
        ctx.register_object_store(
            ObjectStoreUrl::parse(format!("{}://", EMPTY_SCHEME))?.as_ref(),
            crate::io::get_null_store(),
        );

        let bundle_config = Arc::new(BundleConfig::new(passed_config.as_ref())?);
        let config_provider: Arc<dyn ConfigProvider> =
            Arc::clone(&bundle_config) as Arc<dyn ConfigProvider>;
        let data_dir = Arc::new(RwLock::new(
            writable_dir_from_url(&url, config_provider).await?,
        ));
        let subprocess_cache = crate::function::ipc_bridge::new_subprocess_cache();

        let bundle = Arc::new(Self {
            ctx: Arc::clone(&ctx),
            id,
            packs,
            sources,
            indexes,
            views,
            reports,
            storage: Arc::clone(&storage),
            reader_factory: DataReaderFactory::new_with_plugins(
                Arc::clone(&storage),
                vec![
                    Arc::new(crate::data::plugin::CsvPlugin::default()),
                    Arc::new(crate::data::plugin::BundlebasePlugin),
                    Arc::new(crate::data::plugin::JsonlPlugin::default()),
                    Arc::new(crate::data::plugin::ParquetPlugin::default()),
                ],
            )
            .into(),
            connector_registry,
            function_registry: Arc::new(RwLock::new(FunctionRegistry::new(
                Arc::clone(&data_dir),
                Arc::clone(&ctx),
                Arc::clone(&subprocess_cache),
            ))),
            subprocess_cache,
            name,
            description,
            operations,
            last_manifest_version: Arc::new(RwLock::new(0)),
            version,
            data_dir,
            commits,
            dataframe,
            bundle_schema: Arc::new(RwLock::new(BundleSchema::new())),
            config: bundle_config,
            always_delete_rules: Arc::new(RwLock::new(Vec::new())),
            always_update_rules: Arc::new(RwLock::new(Vec::new())),
            update_overlays: Arc::new(RwLock::new(Vec::new())),
            is_view: Arc::new(RwLock::new(false)),
            min_version: Arc::new(RwLock::new(None)),
            max_version: Arc::new(RwLock::new(None)),
            version_hasher: Arc::new(RwLock::new(Sha256::new())),
        });

        // Register schema providers and the search() table function
        let facade_weak = Arc::downgrade(&bundle) as Weak<dyn BundleFacade>;
        crate::catalog::register_schema_providers(&ctx, facade_weak.clone())?;
        ctx.register_udtf("search", Arc::new(SearchTableFunction::new(facade_weak)));

        Ok(bundle)
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
    /// # Note
    /// Schema providers are registered by `empty()` BEFORE `open_recursive()`,
    /// because operations during loading may query them (e.g., CreateIndexOp builds a dataframe).
    ///
    /// # Example
    /// let bundle = Bundle::open("file:///data/my_bundle").await?;
    /// let schema = bundle.schema();
    /// ```
    pub async fn open(
        path: &str,
        config: Option<PassedBundleConfig>,
    ) -> Result<Arc<Self>, BundlebaseError> {
        let mut visited = HashSet::new();
        let arc_bundle = Self::empty(config).await?;

        arc_bundle.add_pack(ObjectId::BASE_PACK, Arc::new(Pack::new_base()));

        // Refresh data_dir with the config
        arc_bundle.refresh_data_dir().await?;

        Self::open_recursive(
            writable_dir_from_str(path, arc_bundle.config())
                .await?
                .url()
                .as_str(),
            &mut visited,
            &arc_bundle,
        )
        .await?;

        Ok(arc_bundle)
    }

    /// Internal implementation of open() that tracks visited URLs to detect cycles
    async fn open_recursive(
        url: &str,
        visited: &mut HashSet<String>,
        bundle: &Bundle,
    ) -> Result<(), BundlebaseError> {
        if !visited.insert(url.to_string()) {
            return Err(
                format!("Circular dependency detected in bundle from chain: {}", url).into(),
            );
        }

        let data_dir = writable_dir_from_str(url, bundle.config()).await?;
        let manifest_dir = data_dir.writable_subdir(META_DIR)?;

        debug!("Loading initial commit from {}", INIT_FILENAME);

        let init_commit: Option<InitCommit> =
            read_yaml(manifest_dir.file(INIT_FILENAME)?.as_ref()).await?;
        let init_commit = init_commit.ok_or_else(|| {
            BundlebaseError::from(format!(
                "No bundle found at '{}' ({}/{} does not exist)",
                url, META_DIR, INIT_FILENAME
            ))
        })?;

        // Recursively load the base bundle and store the Arc reference
        // Handle views: if view field is set, load parent from "../"
        // Otherwise, use the from field if present
        let parent_url = if init_commit.view.is_some() {
            // For views, parent is always in the parent directory
            // Ensure the URL has a trailing slash so "../" joins correctly
            let mut current_url_str = data_dir.url().to_string();
            if !current_url_str.ends_with('/') {
                current_url_str.push('/');
            }
            let current_url = Url::parse(&current_url_str)?;
            Some(current_url.join("../")?)
        } else {
            init_commit.from.clone()
        };

        if let Some(from_url) = parent_url {
            // Resolve relative URLs against current data_dir
            let resolved_url = if from_url.path().starts_with("..") {
                // Join relative path with current directory
                let current_url = Url::parse(data_dir.url().as_str())?;
                current_url.join(from_url.as_str())?
            } else {
                from_url.clone()
            };

            // Box the recursive call to avoid infinite future size
            Box::pin(Self::open_recursive(resolved_url.as_str(), visited, bundle)).await?;
        };

        // Set id if provided in init_commit
        // If id is None (extending case), keep the id inherited from parent bundle
        if let Some(id) = &init_commit.id {
            *bundle.id.write() = id.clone();
        }

        *bundle.data_dir.write() = Arc::clone(&data_dir);

        // Mark this bundle as a view if it has a view field in the init commit
        *bundle.is_view.write() = init_commit.view.is_some();

        // Set initial version bounds from init commit
        *bundle.min_version.write() = init_commit
            .min_version
            .as_deref()
            .map(bundlebase_common::parse_format_version);
        *bundle.max_version.write() = init_commit
            .max_version
            .as_deref()
            .map(bundlebase_common::parse_format_version);

        // List files in the manifest directory
        let manifest_files = manifest_dir.list_files().await?;

        // Filter out init file AND files from subdirectories (like view_* directories)
        // We only want files directly in the manifest directory
        let manifest_dir_url_str = manifest_dir.url().to_string();
        let manifest_files = manifest_files
            .iter()
            .filter(|x| {
                let file_url = x.url.to_string();
                // File should start with manifest dir URL
                if !file_url.starts_with(&manifest_dir_url_str) {
                    return false;
                }
                // Get the path after the manifest dir
                let relative_path = &file_url[manifest_dir_url_str.len()..];
                // Skip init file
                if x.filename() == Some(INIT_FILENAME) {
                    return false;
                }
                // Only include files directly in manifest dir (no "/" in relative path except leading one)
                !relative_path.trim_start_matches('/').contains('/')
            })
            .collect::<Vec<_>>();

        if manifest_files.is_empty() {
            return Err(format!("No data bundle in: {}", url).into());
        }

        // Sort manifest files by version to ensure commits are loaded in chronological order
        // ObjectStore.list() does not guarantee any particular ordering
        let mut manifest_files = manifest_files.into_iter().cloned().collect::<Vec<_>>();
        manifest_files.sort_by_key(|f| manifest_version(f.filename().unwrap_or("")));

        // Check format version compatibility before deserializing operations.
        // This uses raw YAML scanning so that unknown operation types from newer
        // bundlebase versions don't cause deserialization errors.
        Self::check_format_version(&init_commit, &manifest_files, bundle.config()).await?;

        // Load and apply each manifest in order
        for manifest_file_info in manifest_files {
            *bundle.last_manifest_version.write() =
                manifest_version(manifest_file_info.filename().unwrap_or(""));
            // Create IOFile from FileInfo to read the manifest
            let manifest_file =
                readable_file_from_url(&manifest_file_info.url, bundle.config()).await?;
            let mut commit: BundleCommit =
                read_yaml(manifest_file.as_ref()).await?.ok_or_else(|| {
                    BundlebaseError::from(format!(
                        "Failed to read manifest: {}",
                        manifest_file_info.url
                    ))
                })?;
            commit.url = Some(manifest_file_info.url.clone());
            commit.data_dir = Some(data_dir.url().clone());

            // Resolve schema sidecar files in parallel and populate each
            // AttachBlockOp's in-memory `schema` field. Done before the apply
            // loop because op.apply() requires `self.schema` to be populated.
            resolve_attach_sidecars(&mut commit, data_dir.as_ref(), bundle.config()).await?;

            debug!(
                "Loading commit from {}: {} changes",
                manifest_file_info.filename().unwrap_or("<unknown>"),
                commit.changes.len()
            );

            bundle.commits.write().push(commit.clone());

            // Apply operations from this manifest's changes
            for change in commit.changes {
                debug!(
                    "  Change: {} with {} operations",
                    change.description,
                    change.operations.len()
                );
                for op in change.operations {
                    // Skip view-related operations when loading a view
                    if *bundle.is_view.read() {
                        match &op {
                            AnyOperation::CreateView(_)
                            | AnyOperation::RenameView(_)
                            | AnyOperation::DropView(_) => {
                                debug!("    Skipping (view operation in view): {}", op.describe());
                                continue;
                            }
                            _ => {}
                        }
                    }
                    debug!("    Applying: {}", op.describe());
                    bundle.apply_operation_deferred(op).await?;
                }
            }
        }

        // Compute version once after all operations are applied (O(n) instead of O(n²))
        bundle.recompute_version();

        Ok(())
    }
}

/// Walk all `AttachBlock` ops in a freshly-deserialized commit, parallel-load
/// the referenced `.block.schema.yaml` and `.block.columns.yaml` sidecar
/// files, and populate each op's in-memory `schema` and `column_ids` fields.
/// Required because `AttachBlockOp::apply()` reads these and panics if
/// they're empty/None.
async fn resolve_attach_sidecars(
    commit: &mut BundleCommit,
    data_dir: &dyn IOReadWriteDir,
    config: Arc<BundleConfig>,
) -> Result<(), BundlebaseError> {
    use crate::bundle::operation::AnyOperation;
    use bundlebase_common::ColumnId;
    use std::collections::{HashMap, HashSet};

    let _ = config; // currently unused — kept in signature for symmetry with read_yaml callers.

    // Collect unique sidecar paths used by attach ops in this commit.
    let mut schema_paths: HashSet<String> = HashSet::new();
    let mut column_ids_paths: HashSet<String> = HashSet::new();
    for change in &commit.changes {
        for op in &change.operations {
            if let AnyOperation::AttachBlock(a) = op {
                if a.schema_cache.is_none() {
                    schema_paths.insert(a.schema.clone());
                }
                if a.column_ids_cache.is_empty() {
                    column_ids_paths.insert(a.column_ids.clone());
                }
            }
        }
    }

    if schema_paths.is_empty() && column_ids_paths.is_empty() {
        return Ok(());
    }

    // Parallel-load each unique schema sidecar file. Use the data dir's own
    // `file()` accessor (rather than constructing an ObjectStoreFile
    // directly) so backends like the tar object store, which join paths
    // through their own logic, work correctly.
    let schema_futures = schema_paths.into_iter().map(|path| async move {
        let file = data_dir.file(&path).map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to construct schema file ref '{}': {}",
                path, e
            ))
        })?;
        let value: Option<serde_yaml_ng::Value> = read_yaml(file.as_ref()).await.map_err(|e| {
            BundlebaseError::from(format!("Failed to read schema file '{}': {}", path, e))
        })?;
        let value = value.ok_or_else(|| {
            BundlebaseError::from(format!("Schema sidecar file not found: '{}'", path))
        })?;
        let schema = crate::bundle::operation::serde_util::deserialize_schema_internal(&value)
            .map_err(|e| {
                BundlebaseError::from(format!(
                    "Failed to deserialize schema from '{}': {}",
                    path, e
                ))
            })?;
        Ok::<(String, std::sync::Arc<arrow_schema::Schema>), BundlebaseError>((path, schema))
    });

    // Parallel-load each unique column-id-list sidecar file.
    let column_id_futures = column_ids_paths.into_iter().map(|path| async move {
        let file = data_dir.file(&path).map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to construct column-id file ref '{}': {}",
                path, e
            ))
        })?;
        let value: Option<Vec<ColumnId>> = read_yaml(file.as_ref()).await.map_err(|e| {
            BundlebaseError::from(format!("Failed to read column-id file '{}': {}", path, e))
        })?;
        let ids = value.ok_or_else(|| {
            BundlebaseError::from(format!("Column-id sidecar file not found: '{}'", path))
        })?;
        Ok::<(String, Vec<ColumnId>), BundlebaseError>((path, ids))
    });

    let (schemas_loaded, columns_loaded) = futures::try_join!(
        futures::future::try_join_all(schema_futures),
        futures::future::try_join_all(column_id_futures),
    )?;
    let path_to_schema: HashMap<String, std::sync::Arc<arrow_schema::Schema>> =
        schemas_loaded.into_iter().collect();
    let path_to_column_ids: HashMap<String, Vec<ColumnId>> = columns_loaded.into_iter().collect();

    // Populate each attach op's in-memory schema + column_ids fields.
    for change in &mut commit.changes {
        for op in &mut change.operations {
            if let AnyOperation::AttachBlock(a) = op {
                if a.schema_cache.is_none() {
                    a.schema_cache = path_to_schema.get(&a.schema).cloned();
                }
                if a.column_ids_cache.is_empty() {
                    if let Some(ids) = path_to_column_ids.get(&a.column_ids) {
                        a.column_ids_cache = ids.clone();
                    }
                }
            }
        }
    }

    Ok(())
}

impl Bundle {
    /// Check bundle format version compatibility by scanning raw YAML before typed deserialization.
    ///
    /// Reads the init commit and all manifest files as raw YAML values, looking for
    /// minVersion/maxVersion in the init commit and SetMinVersion/SetMaxVersion operations
    /// in manifests. The most recent values win. Returns an error if the current bundlebase
    /// version is outside the allowed range.
    async fn check_format_version(
        init_commit: &InitCommit,
        manifest_files: &[crate::file_info::FileInfo],
        config: Arc<BundleConfig>,
    ) -> Result<(), BundlebaseError> {
        let current = bundlebase_common::format_version();
        let current_str = bundlebase_common::format_version_string();

        // Start with values from init commit
        let mut min_version = init_commit
            .min_version
            .as_deref()
            .map(bundlebase_common::parse_format_version);
        let mut max_version = init_commit
            .max_version
            .as_deref()
            .map(bundlebase_common::parse_format_version);

        // Scan manifests for SetMinVersion/SetMaxVersion operations. Most
        // bundles never set these, so before paying for a full YAML parse
        // (~50 ms on a 3 MB manifest) do a cheap substring check on the raw
        // bytes — if neither operation name is present, skip the parse.
        for file_info in manifest_files {
            let file = readable_file_from_url(&file_info.url, config.clone()).await?;
            let bytes = match file.read_bytes().await? {
                Some(b) => b,
                None => continue,
            };
            let yaml_str = std::str::from_utf8(&bytes)
                .map_err(|e| BundlebaseError::from(format!("Invalid UTF-8 in manifest: {}", e)))?;
            // Fast path: no version ops in this manifest's raw text. Skip
            // the (expensive) YAML parse entirely — a 3 MB manifest parses
            // in ~50 ms, while a str::contains check is ~1 ms.
            if !yaml_str.contains("setMinVersion") && !yaml_str.contains("setMaxVersion") {
                continue;
            }
            let doc: serde_yaml_ng::Value = serde_yaml_ng::from_str(yaml_str)
                .map_err(|e| BundlebaseError::from(format!("Invalid YAML in manifest: {}", e)))?;

            // Walk changes[].operations[] looking for version ops
            if let Some(changes) = doc.get("changes").and_then(|c| c.as_sequence()) {
                for change in changes {
                    if let Some(ops) = change.get("operations").and_then(|o| o.as_sequence()) {
                        for op in ops {
                            if let Some(op_type) = op.get("type").and_then(|t| t.as_str()) {
                                match op_type {
                                    "setMinVersion" => {
                                        if let Some(v) = op.get("version").and_then(|v| v.as_str())
                                        {
                                            min_version =
                                                Some(bundlebase_common::parse_format_version(v));
                                        }
                                    }
                                    "setMaxVersion" => {
                                        if let Some(v) = op.get("version").and_then(|v| v.as_str())
                                        {
                                            max_version =
                                                Some(bundlebase_common::parse_format_version(v));
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
        }

        // Check compatibility
        if let Some(min) = min_version {
            if current < min {
                return Err(format!(
                    "This bundle requires bundlebase >= {}.{}, but current version is {}",
                    min.0, min.1, current_str
                )
                .into());
            }
        }
        if let Some(max) = max_version {
            if current > max {
                return Err(format!(
                    "This bundle requires bundlebase <= {}.{}, but current version is {}. \
                     Run `bundlebase upgrade-bundle` to update.",
                    max.0, max.1, current_str
                )
                .into());
            }
        }

        Ok(())
    }

    /// Get the view ID for a given view name
    pub fn get_view_id(&self, name: &str) -> Option<ObjectId> {
        self.views.read().get(name).copied()
    }

    /// Get the view ID for a given view identifier (either name or ID)
    ///
    /// This method accepts either:
    /// - A view ID (as a string representation of ObjectId)
    /// - A view name
    ///
    /// Returns the ID and name if found, or an error if not found or ambiguous.
    pub fn get_view_id_by_name_or_id(
        &self,
        identifier: &str,
    ) -> Result<(ObjectId, String), BundlebaseError> {
        let views = self.views.read();

        // Try to parse as ObjectId first
        if let Ok(id) = ObjectId::try_from(identifier) {
            // Look for this ID in the views map values
            for (name, view_id) in views.iter() {
                if view_id == &id {
                    return Ok((id, name.clone()));
                }
            }
            return Err(format!("View with ID '{}' not found", identifier).into());
        }

        // Treat as name
        if let Some(id) = views.get(identifier) {
            Ok((*id, identifier.to_string()))
        } else {
            // Provide helpful error message listing available views
            if views.is_empty() {
                Err(format!("View '{}' not found (no views exist)", identifier).into())
            } else {
                let available: Vec<String> = views
                    .iter()
                    .map(|(name, id)| format!("{} (id: {})", name, id))
                    .collect();
                Err(format!(
                    "View '{}' not found. Available views:\n  {}",
                    identifier,
                    available.join("\n  ")
                )
                .into())
            }
        }
    }

    /// Get the number of packs (for testing/debugging)
    pub fn packs_count(&self) -> usize {
        self.packs.read().len()
    }

    /// Check if this bundle is a view
    pub fn is_view(&self) -> bool {
        *self.is_view.read()
    }

    /// Modifies this bundle with the given operation using interior mutability.
    pub(crate) async fn apply_operation(&self, op: AnyOperation) -> Result<(), BundlebaseError> {
        let description = &op.describe();
        debug!("Applying operation to bundle: {}...", &description);

        debug!("Checking: {}", &description);
        op.check(self).await?;

        debug!("Apply: {}", &description);
        op.apply(self).await?;
        self.update_version_for_op(&op);
        self.operations.write().push(op);
        // clear cached values
        self.dataframe.clear();
        *self.bundle_schema.write() = BundleSchema::new();
        debug!("Cleared dataframe");

        debug!("Applying operation to bundle: {}...DONE", &description);

        Ok(())
    }

    /// Applies an operation without recomputing the version.
    /// Used during bulk manifest loading where intermediate versions are never read.
    /// Call `recompute_version()` once after all operations are applied.
    async fn apply_operation_deferred(&self, op: AnyOperation) -> Result<(), BundlebaseError> {
        let description = &op.describe();
        debug!("Applying operation (deferred): {}...", &description);

        op.check(self).await?;
        op.apply(self).await?;
        self.operations.write().push(op);

        // Clear cached values so subsequent operations see fresh state
        self.dataframe.clear();
        *self.bundle_schema.write() = BundleSchema::new();

        debug!("Applying operation (deferred): {}...DONE", &description);
        Ok(())
    }

    pub fn data_dir(&self) -> Arc<dyn IOReadWriteDir> {
        Arc::clone(&*self.data_dir.read())
    }

    pub fn config(&self) -> Arc<BundleConfig> {
        Arc::clone(&self.config)
    }

    /// Returns all stored reports (id -> entry).
    pub fn reports(&self) -> HashMap<String, ReportEntry> {
        self.reports.read().clone()
    }

    /// Look up a stored report by id.
    pub fn report_by_id(&self, id: &str) -> Option<ReportEntry> {
        self.reports.read().get(id).cloned()
    }

    /// Returns the current always-delete rules (WHERE clauses).
    pub fn always_delete_rules(&self) -> Vec<String> {
        self.always_delete_rules.read().clone()
    }

    /// Add an always-delete rule. Deduplicates — adding the same rule twice is a no-op.
    pub fn add_always_delete_rule(&self, where_clause: &str) {
        let mut rules = self.always_delete_rules.write();
        if !rules.contains(&where_clause.to_string()) {
            rules.push(where_clause.to_string());
        }
    }

    /// Remove a specific always-delete rule by WHERE clause.
    pub fn remove_always_delete_rule(&self, where_clause: &str) {
        self.always_delete_rules
            .write()
            .retain(|r| r != where_clause);
    }

    /// Remove all always-delete rules.
    pub fn clear_always_delete_rules(&self) {
        self.always_delete_rules.write().clear();
    }

    /// Returns the current always-update rules.
    pub fn always_update_rules(&self) -> Vec<AlwaysUpdateRule> {
        self.always_update_rules.read().clone()
    }

    /// Add an always-update rule. Deduplicates — adding the same rule twice is a no-op.
    pub fn add_always_update_rule(&self, rule: &AlwaysUpdateRule) {
        let mut rules = self.always_update_rules.write();
        if !rules.contains(rule) {
            rules.push(rule.clone());
        }
    }

    /// Remove a specific always-update rule by its canonical text ("SET ... WHERE ...").
    pub fn remove_always_update_rule(&self, rule_text: &str) {
        self.always_update_rules
            .write()
            .retain(|r| r.rule_text() != rule_text);
    }

    /// Remove all always-update rules.
    pub fn clear_always_update_rules(&self) {
        self.always_update_rules.write().clear();
    }

    /// Recreate data_dir from the current URL + config.
    /// Called after SaveConfigOp changes config.
    pub(crate) async fn refresh_data_dir(&self) -> Result<(), BundlebaseError> {
        let url = self.data_dir.read().url().clone();
        *self.data_dir.write() = writable_dir_from_url(&url, self.config()).await?;
        Ok(())
    }

    /// Update this bundle's state from another bundle, preserving Arc references.
    ///
    /// This is used by BundleBuilder to "reload" without breaking shared references
    /// held by schema providers. All `Arc<RwLock<T>>` fields have their contents
    /// replaced with the contents from the other bundle.
    ///
    /// The dataframe cache is cleared as it may now be stale.
    pub(crate) fn reload_from(&self, other: Bundle) {
        *self.id.write() = other.id.read().clone();
        *self.name.write() = other.name.read().clone();
        *self.description.write() = other.description.read().clone();
        *self.version.write() = other.version.read().clone();
        *self.last_manifest_version.write() = *other.last_manifest_version.read();
        *self.operations.write() = other.operations.read().clone();
        *self.sources.write() = other.sources.read().clone();
        *self.commits.write() = other.commits.read().clone();
        *self.packs.write() = other.packs.read().clone();
        *self.indexes.write() = other.indexes.read().clone();
        *self.views.write() = other.views.read().clone();
        // Reload config: replace Stored entries from the new manifest
        self.config.reload_stored(&other.config);
        *self.data_dir.write() = Arc::clone(&*other.data_dir.read());
        *self.is_view.write() = *other.is_view.read();
        *self.always_delete_rules.write() = other.always_delete_rules.read().clone();
        *self.always_update_rules.write() = other.always_update_rules.read().clone();
        *self.update_overlays.write() = other.update_overlays.read().clone();
        *self.version_hasher.write() = other.version_hasher.read().clone();
        self.dataframe.clear();
        *self.bundle_schema.write() = BundleSchema::new();
    }

    pub fn ctx(&self) -> Arc<SessionContext> {
        self.ctx.clone()
    }

    /// Joins the pack with join metadata to the base dataframe.
    ///
    /// After the join, any columns from the join pack whose names collide with
    /// columns already present in the base DataFrame are renamed to
    /// `{pack_name}_{column_name}` so the schema never contains ambiguous names.
    async fn dataframe_join(
        &self,
        base_df: DataFrame,
        pack: &Pack,
        bundle_schema: &mut BundleSchema,
    ) -> Result<DataFrame, BundlebaseError> {
        let join_table = format!("packs.{}", Pack::table_name(pack.id()));

        // Translate the join expression from user-visible names to internal names.
        // Build a combined name map from base pack columns AND join pack columns.
        let pack_expression = pack
            .expression()
            .expect("Pack must have expression for join");
        let mut combined_names = bundle_schema.clone();
        // Add join pack's column names from AttachBlock operations targeting this pack
        let ops = self.operations.read().clone();
        for op in &ops {
            if let AnyOperation::AttachBlock(attach) = op {
                if &attach.pack == pack.id() {
                    if let Some(schema) = &attach.schema_cache {
                        for (field, col_id) in
                            schema.fields().iter().zip(attach.column_ids_cache.iter())
                        {
                            combined_names
                                .entry(*col_id)
                                .or_insert_with(|| field.name().clone());
                        }
                    }
                }
            }
        }
        let translated_expression = combined_names.translate_sql(pack_expression);

        // Create a temporary pack with translated expression for parsing
        let translated_pack = Pack::new(
            *pack.id(),
            &pack.name(),
            &translated_expression,
            *pack.join_type().expect("Pack must have join_type"),
        );

        let (expr, left_alias) =
            sql::parse_join_expr(&self.ctx, "", &translated_pack, &base_df).await?;

        let base_df = base_df.alias(left_alias)?;

        let name = pack.name();
        let join_type = pack.join_type().expect("Pack must have join_type for join");

        // Capture base column names before aliasing so we can detect duplicates
        let base_col_names: std::collections::HashSet<String> = base_df
            .schema()
            .columns()
            .iter()
            .map(|c| c.name.clone())
            .collect();

        let mut joined_df = base_df.join_on(
            self.ctx.table(&join_table).await?.alias(&name)?,
            join_type.to_datafusion(),
            expr,
        )?;

        // Disambiguate: rename join-pack internal name columns that collide with base columns.
        // This happens when both packs share a column with the same ColumnId (same logical column).
        for col in joined_df.schema().columns() {
            let is_from_join_pack = col.relation.as_ref().is_some_and(|r| r.table() == name);
            if is_from_join_pack && base_col_names.contains(&col.name) {
                // Rename to disambiguated internal name
                let new_internal_name = format!("{}_{}", name, col.name);
                joined_df = joined_df.with_column_renamed(col.flat_name(), &new_internal_name)?;
                // Add a col_names entry so the final rename maps this to a user-visible name.
                // The user-visible name is pack_name + user_visible_original_name.
                if let Some(col_id) = bundle_schema::parse_internal_name(&col.name) {
                    let user_name = bundle_schema
                        .get(&col_id)
                        .map(|n| format!("{}_{}", name, n))
                        .unwrap_or_else(|| new_internal_name.clone());
                    // Generate a new ColumnId for this disambiguated column so the
                    // final rename picks it up (it renames internal_name → user_name)
                    let disambig_id = ColumnId::generate();
                    bundle_schema.insert(disambig_id, user_name);
                    // Also rename to the new disambiguated internal name so final rename works
                    let final_internal = bundle_schema
                        .internal_name(&disambig_id)
                        .expect("just inserted");
                    joined_df =
                        joined_df.with_column_renamed(&new_internal_name, &final_internal)?;
                }
            }
        }

        Ok(joined_df)
    }

    fn resolved_bundle_schema(&self) -> BundleSchema {
        let current = self.bundle_schema.read().clone();
        if !current.is_empty() {
            return current;
        }
        let resolved = BundleSchema::resolved(&self.operations.read());
        *self.bundle_schema.write() = resolved.clone();
        resolved
    }

    /// Incrementally updates the version by feeding a single new operation into the
    /// running hasher. O(1) per call instead of O(n).
    fn update_version_for_op(&self, op: &AnyOperation) {
        let mut hasher = self.version_hasher.write();
        hasher.update(op.version().as_bytes());

        // Finalize a clone to get the version string without consuming the running state
        let new_version = hex::encode(hasher.clone().finalize())[0..12].to_string();
        *self.version.write() = new_version.clone();

        self.function_registry
            .read()
            .refresh_version_udf(new_version);
    }

    /// Recomputes the version from scratch by iterating all operations.
    /// Also resets the running hasher to match. Used during bulk loading
    /// and for correctness verification in tests.
    fn recompute_version(&self) {
        let mut hasher = Sha256::new();

        for op in self.operations.read().iter() {
            hasher.update(op.version().as_bytes());
        }

        *self.version_hasher.write() = hasher.clone();
        let new_version = hex::encode(hasher.finalize())[0..12].to_string();
        *self.version.write() = new_version.clone();

        self.function_registry
            .read()
            .refresh_version_udf(new_version);
    }

    pub(crate) fn add_pack(&self, pack_id: ObjectId, pack: Arc<Pack>) {
        self.packs.write().insert(pack_id, pack);
    }

    pub(crate) fn get_pack(&self, pack_id: &ObjectId) -> Option<Arc<Pack>> {
        self.packs.read().get(pack_id).cloned()
    }

    /// Get read access to the packs map
    pub(crate) fn packs(&self) -> &Arc<RwLock<HashMap<ObjectId, Arc<Pack>>>> {
        &self.packs
    }

    /// Detach fields that should be independent for an extend operation.
    ///
    /// After cloning, some fields share the same Arc<RwLock> with the original.
    /// This method creates independent wrappers so modifications don't affect
    /// the original bundle.
    ///
    /// Fields detached:
    /// - `data_dir`: Extended bundles may have different storage locations
    /// - `last_manifest_version`: Each bundle tracks its own manifest version
    /// - `operations`: Each bundle has its own operation list (select/filter adds ops)
    pub(crate) fn detach_for_extend(&mut self) {
        // Create independent copies of fields that will be modified
        // Read values first to avoid borrow conflicts
        let current_data_dir = Arc::clone(&*self.data_dir.read());
        let current_manifest_version = *self.last_manifest_version.read();
        let current_operations = self.operations.read().clone();
        self.data_dir = Arc::new(RwLock::new(current_data_dir));
        self.last_manifest_version = Arc::new(RwLock::new(current_manifest_version));
        self.operations = Arc::new(RwLock::new(current_operations));
        self.bundle_schema = Arc::new(RwLock::new(BundleSchema::new()));
    }

    /// Find a join pack by its name
    pub fn pack_by_name(&self, name: &str) -> Option<Arc<Pack>> {
        self.packs
            .read()
            .values()
            .find(|p| p.name() == name)
            .cloned()
    }

    /// Get a pack's name by its ID
    pub fn pack_name(&self, pack_id: &ObjectId) -> Option<String> {
        self.packs.read().get(pack_id).map(|p| p.name().to_string())
    }

    /// Get all join pack names
    pub fn join_names(&self) -> Vec<String> {
        self.packs
            .read()
            .values()
            .filter_map(|p| Some(p.name().to_string()))
            .collect()
    }

    /// Get read access to the indexes list
    pub(crate) fn indexes(&self) -> &Arc<RwLock<Vec<Arc<IndexDefinition>>>> {
        &self.indexes
    }

    /// Check if an index already exists at the correct version for the given column ID
    pub(crate) fn get_index(
        &self,
        column_id: &ColumnId,
        block: &VersionedBlockId,
    ) -> Option<Arc<IndexedBlocks>> {
        for index in self.indexes.read().iter() {
            if index.column_ids().contains(column_id) {
                if let Some(indexed_blocks) = index.indexed_blocks(block) {
                    return Some(indexed_blocks);
                }
            }
        }
        None
    }

    /// Add a source definition to the bundle
    pub(crate) fn add_source(&self, op: CreateSourceOp) {
        let registry = self.connector_registry.read();
        if let Ok(source) = Source::from_op(&op, &registry) {
            self.sources.write().insert(op.id, Arc::new(source));
        }
    }

    /// Get a source by its ID
    pub fn get_source(&self, source_id: &ObjectId) -> Option<Arc<Source>> {
        self.sources.read().get(source_id).cloned()
    }

    /// Get all sources for a specific pack
    pub fn get_sources_for_pack(&self, pack_id: &ObjectId) -> Vec<Arc<Source>> {
        self.sources
            .read()
            .values()
            .filter(|s| s.pack() == pack_id)
            .cloned()
            .collect()
    }

    /// Get all sources
    pub fn sources(&self) -> HashMap<ObjectId, Arc<Source>> {
        self.sources.read().clone()
    }

    /// Check if any temporary connectors or functions has been added.
    pub(crate) fn has_temporary_udf(&self) -> bool {
        self.function_registry.read().has_temporary()
            || self.connector_registry.read().has_temporary()
    }

    /// Find a block by ID across all packs
    pub(crate) fn find_block(&self, block_id: &BlockId) -> Option<Arc<DataBlock>> {
        let packs = self.packs.read();
        for pack in packs.values() {
            for block in pack.blocks() {
                if block.id() == block_id {
                    return Some(block);
                }
            }
        }
        None
    }

    /// Find the currently attached block whose current location matches `location`.
    pub fn find_block_by_current_location(&self, location: &str) -> Option<Arc<DataBlock>> {
        let block_locations = self.build_block_location_map();
        let packs = self.packs.read();
        for pack in packs.values() {
            for block in pack.blocks() {
                if block_locations
                    .get(block.id())
                    .is_some_and(|current_location| current_location == location)
                {
                    return Some(block);
                }
            }
        }
        None
    }

    /// Get the connector registry
    pub(crate) fn connector_registry(&self) -> Arc<RwLock<ConnectorRegistry>> {
        Arc::clone(&self.connector_registry)
    }

    /// Get the function registry
    pub(crate) fn function_registry(&self) -> Arc<RwLock<FunctionRegistry>> {
        Arc::clone(&self.function_registry)
    }

    /// Build a map of block IDs to their expected hashes from operations.
    ///
    /// Searches through AttachBlockOp and ReplaceBlockOp operations to build
    /// a mapping from block ID to the expected hash. For blocks that have been
    /// replaced, uses the hash from the most recent ReplaceBlockOp.
    pub fn build_block_hash_map(&self) -> HashMap<BlockId, String> {
        let mut block_hashes: HashMap<BlockId, String> = HashMap::new();

        for op in self.operations.read().iter() {
            match op {
                operation::AnyOperation::AttachBlock(attach) => {
                    block_hashes.insert(attach.id, attach.hash.clone());
                }
                operation::AnyOperation::ReplaceBlock(replace) => {
                    // ReplaceBlock updates the hash for an existing block
                    block_hashes.insert(replace.id, replace.new_hash.clone());
                }
                _ => {}
            }
        }

        block_hashes
    }

    /// Build a map of block IDs to their stored locations from operations.
    pub fn build_block_location_map(&self) -> HashMap<BlockId, String> {
        let mut block_locations: HashMap<BlockId, String> = HashMap::new();

        for op in self.operations.read().iter() {
            match op {
                operation::AnyOperation::AttachBlock(attach) => {
                    block_locations.insert(attach.id, attach.location.clone());
                }
                operation::AnyOperation::ReplaceBlock(replace) => {
                    block_locations.insert(replace.id, replace.new_location.clone());
                }
                _ => {}
            }
        }

        block_locations
    }

    /// Verify the integrity of all files in the bundle by checking SHA256 hashes.
    ///
    /// This method checks:
    /// - All data blocks: Verifies SHA256 hash matches the stored hash from operations
    /// - Index files: Verifies the files exist (no hash verification for indexes)
    ///
    /// # Returns
    /// `VerificationResults` with details for each file verified.
    pub async fn verify_data(&self) -> Result<VerificationResults, BundlebaseError> {
        let mut results = Vec::new();
        let block_hashes = self.build_block_hash_map();
        let block_locations = self.build_block_location_map();

        // Verify each block in each pack
        let packs = self.packs.read().clone();
        for pack in packs.values() {
            for block in pack.blocks() {
                let block_id = block.id();
                let location = block_locations
                    .get(block_id)
                    .cloned()
                    .unwrap_or_else(|| block.reader().url().to_string());

                let expected_hash = block_hashes.get(block_id).cloned();

                match self
                    .verify_block_hash(&location, expected_hash.as_deref())
                    .await
                {
                    Ok((actual_hash, passed)) => {
                        results.push(FileVerificationResult {
                            location,
                            file_type: "data".to_string(),
                            expected_hash,
                            actual_hash: Some(actual_hash),
                            passed,
                            error: None,
                            version_updated: false,
                        });
                    }
                    Err(e) => {
                        results.push(FileVerificationResult {
                            location,
                            file_type: "data".to_string(),
                            expected_hash,
                            actual_hash: None,
                            passed: false,
                            error: Some(e.to_string()),
                            version_updated: false,
                        });
                    }
                }
            }
        }

        // Verify index files exist
        let indexes = self.indexes.read().clone();
        for index_def in indexes.iter() {
            for indexed_blocks in index_def.all_indexed_blocks() {
                let path = indexed_blocks.path();
                let result = self.verify_index_exists(path).await;
                results.push(result);
            }
        }

        Ok(VerificationResults::from_files(results))
    }

    /// Verify a block's hash by computing it from the file.
    ///
    /// Returns (actual_hash, passed) where passed is true if hashes match or no expected hash.
    async fn verify_block_hash(
        &self,
        location: &str,
        expected_hash: Option<&str>,
    ) -> Result<(String, bool), BundlebaseError> {
        use crate::io::readable_file_from_path;

        let file = readable_file_from_path(location, self.data_dir(), self.config()).await?;
        let actual_hash = file.compute_hash().await?;

        let passed = match expected_hash {
            Some(expected) => expected == actual_hash,
            None => true, // No expected hash means we can't verify, treat as passed
        };

        Ok((actual_hash, passed))
    }

    /// Verify an index file exists.
    async fn verify_index_exists(&self, path: &str) -> FileVerificationResult {
        use crate::io::plugin::object_store::ObjectStoreFile;
        use crate::io::IOReadFile;

        match ObjectStoreFile::from_str(path, self.data_dir().as_ref(), self.config()) {
            Ok(file) => match file.exists().await {
                Ok(true) => FileVerificationResult {
                    location: path.to_string(),
                    file_type: "index".to_string(),
                    expected_hash: None,
                    actual_hash: None,
                    passed: true,
                    error: None,
                    version_updated: false,
                },
                Ok(false) => FileVerificationResult {
                    location: path.to_string(),
                    file_type: "index".to_string(),
                    expected_hash: None,
                    actual_hash: None,
                    passed: false,
                    error: Some("Index file not found".to_string()),
                    version_updated: false,
                },
                Err(e) => FileVerificationResult {
                    location: path.to_string(),
                    file_type: "index".to_string(),
                    expected_hash: None,
                    actual_hash: None,
                    passed: false,
                    error: Some(format!("Failed to check index file: {}", e)),
                    version_updated: false,
                },
            },
            Err(e) => FileVerificationResult {
                location: path.to_string(),
                file_type: "index".to_string(),
                expected_hash: None,
                actual_hash: None,
                passed: false,
                error: Some(format!("Failed to create file handle: {}", e)),
                version_updated: false,
            },
        }
    }
}

#[async_trait]
impl BundleFacade for Bundle {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn id(&self) -> String {
        self.id.read().clone()
    }

    /// Retrieve the bundle name, if set.
    fn name(&self) -> Option<String> {
        self.name.read().clone()
    }

    /// Retrieve the bundle description, if set.
    fn description(&self) -> Option<String> {
        self.description.read().clone()
    }

    /// Retrieve the URL of the base bundle this was loaded from, if any.
    fn url(&self) -> Url {
        self.data_dir.read().url().clone()
    }

    fn from(&self) -> Option<Url> {
        let current_data_dir_url = self.data_dir.read().url().clone();
        self.commits
            .read()
            .iter()
            .filter(|x| x.data_dir != Some(current_data_dir_url.clone()))
            .last()
            .and_then(|c| c.data_dir.clone())
    }

    fn version(&self) -> String {
        if self.has_temporary_udf() {
            return "TEMP".to_string();
        }
        self.version.read().clone()
    }

    /// Returns the commit history for this bundle, starting with any base bundles
    fn history(&self) -> Vec<BundleCommit> {
        self.commits.read().clone()
    }

    fn operations(&self) -> Vec<AnyOperation> {
        self.operations.read().clone()
    }

    fn bundle_schema(&self) -> BundleSchema {
        self.resolved_bundle_schema()
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

        // Check if base pack exists and has data
        let base_pack_has_data = self
            .packs
            .read()
            .get(&ObjectId::BASE_PACK)
            .is_some_and(|p| !p.is_empty());

        let df = if base_pack_has_data {
            let table_name = format!("packs.{}", Pack::table_name(&ObjectId::BASE_PACK));
            let mut df = self.ctx.table(&table_name).await?;

            // Snapshot packs so we can look up join packs by ID
            let packs_snapshot = self.packs.read().clone();

            // Clone operations to avoid holding lock across async calls
            let ops = self.operations.read().clone();

            // Process operations in order. When we encounter a CreateJoin,
            // execute the join at that point so that prior operations (renames,
            // drops, etc.) are already reflected in the DataFrame schema.
            debug!(
                "dataframe: Applying {} operations to dataframe...",
                ops.len()
            );

            let mut bundle_schema = BundleSchema::initial(&ops);
            let cast_active = resolve_cast_ops(&ops);
            for (op, active) in ops.iter().zip(cast_active.iter()) {
                if !active {
                    continue;
                }
                debug!("Applying to dataframe: {}", &op.describe());
                if let AnyOperation::CreateJoin(create_join) = op {
                    if let Some(pack) = packs_snapshot.get(&create_join.id) {
                        if pack.is_join() && !pack.is_empty() {
                            df = self.dataframe_join(df, pack, &mut bundle_schema).await?;
                        }
                    }
                } else {
                    df = op
                        .apply_dataframe(df, self.ctx.clone(), &mut bundle_schema)
                        .await?;
                }
            }
            debug!(
                "dataframe: Applying {} operations to dataframe...DONE",
                ops.len()
            );

            // Final rename: replace internal names with user-visible names
            df = bundle_schema.rename_to_real_names(df)?;

            // Cache the schema on the BundleSchema
            let schema = Arc::new(df.schema().as_arrow().clone());
            bundle_schema.set_schema(schema);
            *self.bundle_schema.write() = bundle_schema;

            df
        } else {
            // No base pack, or base pack has no data yet
            debug!("No base pack or empty base pack, using no-data dataframe");
            no_data_dataframe(&self.ctx())?
        };
        self.dataframe.replace(df);
        debug!("Building dataframe...DONE");
        Ok(self.dataframe.dataframe())
    }

    async fn extend(&self, data_dir: Option<&str>) -> Result<Arc<BundleBuilder>, BundlebaseError> {
        BundleBuilder::extend(Arc::new(self.clone()), data_dir).await
    }

    async fn query(
        &self,
        sql: &str,
        params: Vec<ScalarValue>,
        hard_limit: Option<usize>,
    ) -> Result<SendableRecordBatchStream, BundlebaseError> {
        let ctx = self.ctx();

        let plan = ctx.state().create_logical_plan(sql).await?;

        // Apply parameter values using DataFusion's native binding
        let plan = plan.with_param_values(params)?;

        // Execute the parameterized plan
        let mut result_df = ctx.execute_logical_plan(plan).await?;

        // Apply hard row limit if specified (DataFusion optimizes this in the physical plan)
        if let Some(n) = hard_limit {
            result_df = result_df.limit(0, Some(n))?;
        }

        Ok(result_df.execute_stream().await?)
    }

    async fn view(&self, identifier: &str) -> Result<Arc<Bundle>, BundlebaseError> {
        // Look up view by name or ID
        let (view_id, _name) = self.get_view_id_by_name_or_id(identifier)?;

        // Construct view path: view_{id}/
        let view_path = self
            .data_dir()
            .subdir(&format!("view_{}", view_id))?
            .url()
            .to_string();

        // Open view as Bundle (automatically loads parent via FROM)
        let passed = (*self.config.passed_config()).clone();
        Bundle::open(&view_path, Some(passed)).await
    }

    fn views(&self) -> HashMap<ObjectId, String> {
        // Reverse the name->id HashMap to id->name
        self.views
            .read()
            .iter()
            .map(|(name, id)| (*id, name.clone()))
            .collect()
    }

    async fn export_tar(&self, tar_path: &str, gzip: bool) -> Result<String, BundlebaseError> {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use futures::StreamExt;
        use std::fs::File;
        use tar::{Builder, Header};

        let tar_file = File::create(tar_path)
            .map_err(|e| format!("Failed to create tar file '{}': {}", tar_path, e))?;
        // When `gzip = true`, wrap the file in a GzEncoder so tar bytes stream
        // through gzip on the way to disk. The encoder is unwrapped at the end
        // and explicitly `finish()`-ed so any final compressed block lands on
        // disk before we report success.
        let mut builder = if gzip {
            Builder::new(WriteEither::Gz(GzEncoder::new(tar_file, Compression::default())))
        } else {
            Builder::new(WriteEither::Plain(tar_file))
        };

        // Get all files from the bundle's data_dir
        let data_dir = self.data_dir();
        let files = data_dir.list_files().await?;

        debug!("Exporting {} files to tar archive", files.len());

        // First pass: collect relative paths and sizes for the manifest.
        // FileInfo.size comes from list_files() metadata, so no extra I/O needed.
        let mut manifest_entries: Vec<serde_json::Value> = Vec::with_capacity(files.len());
        let base_url = data_dir.url();
        let mut relative_paths: Vec<String> = Vec::with_capacity(files.len());

        for file in &files {
            let file_url = &file.url;

            let relative_path = if file_url.as_str().starts_with(base_url.as_str()) {
                &file_url.as_str()[base_url.as_str().len()..]
            } else {
                return Err(format!(
                    "File URL '{}' is not under base URL '{}'",
                    file_url, base_url
                )
                .into());
            };

            let relative_path = relative_path.trim_start_matches('/').to_string();
            let size = file.size.unwrap_or(0);

            manifest_entries.push(serde_json::json!({
                "name": relative_path,
                "size": size,
            }));
            relative_paths.push(relative_path);
        }

        // Write _bundlebase_manifest.json as the first tar entry
        let manifest_json = serde_json::to_vec(&manifest_entries)
            .map_err(|e| format!("Failed to serialize tar manifest: {}", e))?;

        let mtime = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("BUG: current time should be after Unix epoch")
            .as_secs();

        let mut manifest_header = Header::new_gnu();
        manifest_header.set_size(manifest_json.len() as u64);
        manifest_header.set_mode(0o644);
        manifest_header.set_mtime(mtime);
        manifest_header.set_cksum();

        builder
            .append_data(
                &mut manifest_header,
                "_bundlebase_manifest.json",
                &manifest_json[..],
            )
            .map_err(|e| format!("Failed to write tar manifest: {}", e))?;

        // Second pass: write each file's data
        for (i, file) in files.iter().enumerate() {
            let relative_path = &relative_paths[i];

            debug!("Adding file to tar: {}", relative_path);

            // Read file contents via stream
            let io_file = readable_file_from_url(&file.url, self.config()).await?;
            let mut stream = io_file
                .read_stream()
                .await?
                .ok_or_else(|| BundlebaseError::from(format!("File not found: {}", file.url)))?;

            // Collect stream into buffer (tar API requires &[u8])
            let mut buffer = Vec::new();
            while let Some(chunk_result) = stream.next().await {
                let chunk = chunk_result?;
                buffer.extend_from_slice(&chunk);
            }

            // Create tar header
            let mut header = Header::new_gnu();
            header.set_size(buffer.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(mtime);
            header.set_cksum();

            // Append to tar
            builder
                .append_data(&mut header, relative_path.as_str(), &buffer[..])
                .map_err(|e| format!("Failed to append file '{}' to tar: {}", relative_path, e))?;
        }

        // Finish writing tar (writes footer)
        builder
            .finish()
            .map_err(|e| format!("Failed to finalize tar archive: {}", e))?;

        // Recover the underlying writer from the tar Builder; for the gzip
        // case explicitly finish the encoder so its trailing block flushes.
        let writer = builder
            .into_inner()
            .map_err(|e| format!("Failed to flush tar archive: {}", e))?;
        if let WriteEither::Gz(encoder) = writer {
            encoder
                .finish()
                .map_err(|e| format!("Failed to finalize gzip stream: {}", e))?;
        }

        info!("Exported bundle to tar archive: {}", tar_path);
        Ok(format!("Exported bundle to {}", tar_path))
    }

    fn status_changes(&self) -> Vec<operation::BundleChange> {
        Vec::new() // Bundle (read-only) always has empty status
    }

    fn status(&self) -> BundleStatus {
        BundleStatus::new() // Bundle (read-only) always has empty status
    }

    fn indexes(&self) -> Vec<Arc<IndexDefinition>> {
        self.indexes.read().clone()
    }

    fn packs(&self) -> HashMap<ObjectId, Arc<Pack>> {
        self.packs.read().clone()
    }

    fn sources(&self) -> HashMap<ObjectId, Arc<crate::bundle::Source>> {
        self.sources.read().clone()
    }

    fn views_by_name(&self) -> HashMap<String, ObjectId> {
        self.views.read().clone()
    }

    fn reports(&self) -> HashMap<String, ReportEntry> {
        Bundle::reports(self)
    }

    fn always_delete_rules(&self) -> Vec<String> {
        Bundle::always_delete_rules(self)
    }

    fn always_update_rules(&self) -> Vec<AlwaysUpdateRule> {
        Bundle::always_update_rules(self)
    }

    fn data_dir(&self) -> Arc<dyn IOReadWriteDir> {
        Bundle::data_dir(self)
    }

    fn config(&self) -> Arc<BundleConfig> {
        Bundle::config(self)
    }

    async fn drop_temp_connector(
        &self,
        name: &str,
        platform: Option<&crate::platform::Platform>,
    ) -> Result<usize, BundlebaseError> {
        Ok(self
            .connector_registry
            .write()
            .remove_entry(name, platform, true))
    }

    async fn drop_temp_function(
        &self,
        name: &str,
        platform: Option<&crate::platform::Platform>,
    ) -> Result<usize, BundlebaseError> {
        self.function_registry.write().drop_temp(name, platform)
    }

    async fn rename_temp_connector(
        &self,
        old_name: &str,
        new_name: &str,
    ) -> Result<(), BundlebaseError> {
        let new_namespaced = crate::NamespacedName::parse(new_name, "Connector")?;

        // Validate old name has temporary entries
        {
            let registry = self.connector_registry.read();
            let has_temp = registry
                .entries()
                .iter()
                .any(|e| e.temporary && e.name == old_name);
            if !has_temp {
                return Err(format!(
                    "No temporary connector entries found for '{}'. Use IMPORT TEMP CONNECTOR first.",
                    old_name
                ).into());
            }
            // Check new name doesn't conflict
            if registry.has_entry(new_name) {
                return Err(format!(
                    "Connector '{}' already exists. Drop it first or choose a different name.",
                    new_name
                )
                .into());
            }
        }

        self.connector_registry
            .write()
            .rename_temp_entries(old_name, &new_namespaced);

        // Update sources referencing the old connector name
        let sources = self.sources.read();
        for (_, source) in sources.iter() {
            if source.connector() == old_name {
                source.set_connector_name(new_name.to_string());
            }
        }

        self.function_registry
            .read()
            .refresh_version_udf("TEMP".to_string());
        Ok(())
    }

    async fn rename_temp_function(
        &self,
        old_name: &str,
        new_name: &str,
    ) -> Result<(), BundlebaseError> {
        let new_namespaced = crate::NamespacedName::parse(new_name, "Function")?;
        self.function_registry
            .write()
            .rename_temp(&old_name, &new_namespaced)?;
        self.function_registry
            .read()
            .refresh_version_udf("TEMP".to_string());
        Ok(())
    }

    async fn set_config(
        &self,
        scope: &Scope,
        key: &str,
        value: &str,
    ) -> Result<(), BundlebaseError> {
        self.config.set(
            scope,
            key,
            value,
            crate::bundle_config::ConfigSource::Runtime,
        )?;
        self.refresh_data_dir().await?;
        Ok(())
    }

    fn connector_registry(&self) -> Arc<RwLock<ConnectorRegistry>> {
        Bundle::connector_registry(self)
    }

    fn function_registry(&self) -> Arc<RwLock<FunctionRegistry>> {
        Bundle::function_registry(self)
    }

    fn ctx(&self) -> Arc<SessionContext> {
        Bundle::ctx(self)
    }
}

fn no_data_dataframe(ctx: &SessionContext) -> Result<DataFrame, BundlebaseError> {
    use arrow::datatypes::{DataType, Field, Schema};
    use datafusion::common::{DFSchema, DFSchemaRef};
    use datafusion::logical_expr::EmptyRelation;

    let arrow_schema = Schema::new(vec![Field::new("no_data", DataType::Utf8, true)]);
    let df_schema = DFSchema::try_from(arrow_schema)?;

    Ok(DataFrame::new(
        ctx.state(),
        LogicalPlan::EmptyRelation(EmptyRelation {
            produce_one_row: false,
            schema: DFSchemaRef::new(df_schema),
        }),
    ))
}

#[derive(Debug)]
pub struct DataFrameHolder {
    pub(crate) dataframe: Arc<RwLock<Option<Arc<DataFrame>>>>,
}

impl DataFrameHolder {
    fn new(df: Option<DataFrame>) -> Self {
        Self {
            dataframe: Arc::new(RwLock::new(df.map(Arc::new))),
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
            dataframe: Arc::clone(&self.dataframe),
        }
    }
}

/// Writer enum used by `Bundle::export_tar` so the tar `Builder` can target
/// either a plain file (no compression) or a gzip-encoded file with one
/// concrete type. The `Gz` arm holds the encoder so the caller can call
/// `finish()` after the tar footer is written, ensuring trailing compressed
/// bytes flush to disk before we return success.
enum WriteEither {
    Plain(std::fs::File),
    Gz(flate2::write::GzEncoder<std::fs::File>),
}

impl std::io::Write for WriteEither {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            WriteEither::Plain(f) => f.write(buf),
            WriteEither::Gz(g) => g.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            WriteEither::Plain(f) => f.flush(),
            WriteEither::Gz(g) => g.flush(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::operation::SetNameOp;

    /// Install a minimal schema provider hook for unit tests.
    ///
    /// This avoids the diamond dependency issue that occurs when bundlebase-catalog
    /// (which depends on bundlebase) is used as a dev-dependency of bundlebase itself.
    /// Instead, we register schema providers using only types from within this crate.
    fn init() {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            crate::catalog::set_schema_provider_hook(test_register_schema_providers);
        });
    }

    /// Minimal schema provider registration for unit tests.
    /// Registers only the "default" and "temp" schemas needed by most unit tests.
    fn test_register_schema_providers(
        ctx: &datafusion::prelude::SessionContext,
        facade: std::sync::Weak<dyn crate::bundle::BundleFacade>,
    ) -> Result<(), crate::BundlebaseError> {
        use crate::catalog::{
            BundleViewTable, BUNDLE_INFO_SCHEMA, BUNDLE_TABLE, CATALOG_NAME, DEFAULT_SCHEMA,
        };

        let catalog = ctx
            .catalog(CATALOG_NAME)
            .expect("Default catalog not found");

        // Register temp schema
        catalog.register_schema(
            "temp",
            Arc::new(datafusion::catalog::MemorySchemaProvider::new()),
        )?;

        // Register a minimal default schema provider inline
        struct TestDefaultSchemaProvider {
            bundle: std::sync::Weak<dyn crate::bundle::BundleFacade>,
        }
        impl std::fmt::Debug for TestDefaultSchemaProvider {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct("TestDefaultSchemaProvider").finish()
            }
        }
        #[async_trait]
        impl datafusion::catalog::SchemaProvider for TestDefaultSchemaProvider {
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn table_names(&self) -> Vec<String> {
                vec![BUNDLE_TABLE.to_string()]
            }
            async fn table(
                &self,
                name: &str,
            ) -> datafusion::error::Result<Option<Arc<dyn datafusion::catalog::TableProvider>>>
            {
                if name == BUNDLE_TABLE {
                    let facade = self.bundle.upgrade().ok_or_else(|| {
                        datafusion::error::DataFusionError::Internal("Bundle dropped".to_string())
                    })?;
                    let df = facade
                        .dataframe()
                        .await
                        .map_err(|e| datafusion::error::DataFusionError::External(e.into()))?;
                    Ok(Some(Arc::new(BundleViewTable::new((*df).clone()))))
                } else {
                    Ok(None)
                }
            }
            fn table_exist(&self, name: &str) -> bool {
                name == BUNDLE_TABLE
            }
        }

        catalog.register_schema(
            DEFAULT_SCHEMA,
            Arc::new(TestDefaultSchemaProvider {
                bundle: facade.clone(),
            }),
        )?;

        // Register empty schemas for blocks and packs (some tests may need them)
        catalog.register_schema(
            "blocks",
            Arc::new(datafusion::catalog::MemorySchemaProvider::new()),
        )?;
        catalog.register_schema(
            "packs",
            Arc::new(datafusion::catalog::MemorySchemaProvider::new()),
        )?;
        catalog.register_schema(
            BUNDLE_INFO_SCHEMA,
            Arc::new(datafusion::catalog::MemorySchemaProvider::new()),
        )?;

        Ok(())
    }

    #[tokio::test]
    async fn test_version() -> Result<(), BundlebaseError> {
        init();
        let c = Bundle::empty(None).await?;
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

    #[tokio::test]
    async fn test_version_udf_sql() -> Result<(), BundlebaseError> {
        init();
        use arrow::array::StringArray;

        let c = Bundle::empty(None).await?;

        // Execute SQL query using version() UDF
        let df = c.ctx().sql("SELECT version() AS ver").await?;
        let batches = df.collect().await?;

        assert_eq!(batches.len(), 1);
        let batch = &batches[0];
        assert_eq!(batch.num_rows(), 1);

        let ver_col = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("version() should return StringArray");
        assert_eq!(ver_col.value(0), "empty");

        Ok(())
    }

    #[tokio::test]
    async fn test_empty_bundle_schema() -> Result<(), BundlebaseError> {
        init();
        let bundle = Bundle::empty(None).await?;

        let schema = bundle.schema().await?;
        assert_eq!(schema.fields().len(), 1, "Empty bundle should have 1 field");
        assert_eq!(schema.field(0).name(), "no_data");
        assert_eq!(
            schema.field(0).data_type(),
            &arrow::datatypes::DataType::Utf8
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_empty_bundle_query() -> Result<(), BundlebaseError> {
        init();
        use futures::TryStreamExt;

        let bundle = Bundle::empty(None).await?;

        let stream = bundle.query("SELECT * FROM bundle", vec![], None).await?;
        let result_schema = stream.schema().clone();
        let batches: Vec<_> = stream.try_collect().await?;

        let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total_rows, 0, "Empty bundle should have 0 rows");

        // Schema should have the no_data column
        assert_eq!(result_schema.fields().len(), 1);
        assert_eq!(result_schema.field(0).name(), "no_data");

        Ok(())
    }

    #[tokio::test]
    async fn test_search_udtf_registered() -> Result<(), BundlebaseError> {
        init();
        let bundle = Bundle::empty(None).await?;

        // search should be a recognized table function even on an empty bundle.
        let ctx = bundle.ctx();
        let result = ctx.table_function("search");
        assert!(
            result.is_ok(),
            "search UDTF should be registered: {:?}",
            result.err()
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_empty_bundle_query_with_alias() -> Result<(), BundlebaseError> {
        init();
        use futures::TryStreamExt;

        let bundle = Bundle::empty(None).await?;

        // This previously failed with "Invalid qualifier t" when the bundle had 0 columns
        let stream = bundle
            .query("SELECT t.* FROM bundle t", vec![], None)
            .await?;
        let batches: Vec<_> = stream.try_collect().await?;

        let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total_rows, 0, "Empty bundle should have 0 rows");

        Ok(())
    }

    #[tokio::test]
    #[ignore = "Needs bundlebase-command extension trait"]
    async fn test_import_temp_connector_changes_version_to_temp() -> Result<(), BundlebaseError> {
        use crate::bundle::facade::BundleFacade;

        let bundle = Bundle::empty(None).await?;
        assert_eq!(bundle.version(), "empty");

        todo!("Uses bundlebase-command extension trait");

        assert_eq!(bundle.version(), "TEMP");

        Ok(())
    }

    #[tokio::test]
    #[ignore = "Needs bundlebase-command extension trait"]
    async fn test_import_temp_connector_version_udf_returns_temp() -> Result<(), BundlebaseError> {
        use arrow::array::StringArray;

        let _bundle = Bundle::empty(None).await?;

        todo!("Uses bundlebase-command extension trait");

        let df = _bundle.ctx().sql("SELECT version() AS ver").await?;
        let batches = df.collect().await?;
        let ver_col = batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("version() should return StringArray");
        assert_eq!(ver_col.value(0), "TEMP");

        Ok(())
    }
}
