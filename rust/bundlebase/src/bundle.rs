mod builder;
mod column_lineage;
mod command;
mod commit;
mod data_block;
mod pack;
mod facade;
mod indexed_blocks;
mod init;
mod operation;
mod source;
mod sql;

use crate::io::EMPTY_SCHEME;
pub use builder::BundleBuilder;
pub use builder::BundleStatus;
pub use column_lineage::{ColumnLineageAnalyzer, ColumnSource};
pub use command::parser::{is_command_statement, parse_command};
pub use command::BundleCommand;
pub use command::CommandOutput;
pub use command::{CommitCommand, ResetCommand, UndoCommand};
pub use commit::{manifest_version, BundleCommit};
pub use data_block::DataBlock;
pub use pack::Pack;
pub use pack::JoinTypeOption;
pub use facade::BundleFacade;
pub use indexed_blocks::IndexedBlocks;
pub use init::{InitCommit, INIT_FILENAME};
pub use operation::{AnyOperation, BundleChange, CreateSourceOp, Operation};
pub use source::Source;
use std::collections::{HashMap, HashSet};

use crate::catalog::{BlockSchemaProvider, BundleInfoSchemaProvider, DefaultSchemaProvider, PackSchemaProvider, CATALOG_NAME};
use crate::udf::VersionUdf;
use crate::data::{DataReaderFactory, ObjectId, VersionedBlockId};
use crate::source::SourceFunctionRegistry;
use crate::functions::FunctionRegistry;
use crate::index::IndexDefinition;
use crate::io::{read_yaml, readable_file_from_url, writable_dir_from_str, writable_dir_from_url, DataStorage, IOReadWriteDir, EMPTY_URL};
use crate::{BundleConfig, BundlebaseError};
use arrow::array::Array;
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use datafusion::catalog::MemorySchemaProvider;
use datafusion::common::{DFSchema, DFSchemaRef};
use datafusion::datasource::object_store::ObjectStoreUrl;
use datafusion::logical_expr::{EmptyRelation, ExplainFormat, ExplainOption, LogicalPlan, ScalarUDF};
use datafusion::prelude::*;
use datafusion::scalar::ScalarValue;
use log::{debug, info};
use parking_lot::RwLock;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use url::Url;
use uuid::Uuid;

pub static META_DIR: &str = "_bundlebase";

/// Result of verifying a single file
#[derive(Debug, Clone)]
pub struct FileVerificationResult {
    pub location: String,
    pub file_type: String, // "data" or "index"
    pub expected_hash: Option<String>,
    pub actual_hash: Option<String>,
    pub passed: bool,
    pub error: Option<String>,
    pub version_updated: bool, // True if version was updated
}

/// Complete verification results for a bundle
#[derive(Debug, Clone)]
pub struct VerificationResults {
    pub files: Vec<FileVerificationResult>,
    pub passed_count: usize,
    pub failed_count: usize,
    pub skipped_count: usize,
    pub versions_updated_count: usize,
    pub all_passed: bool,
}

impl std::fmt::Display for VerificationResults {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.all_passed {
            write!(f, "All {} files verified successfully", self.passed_count)
        } else {
            writeln!(
                f,
                "Verification: {} passed, {} failed",
                self.passed_count, self.failed_count
            )?;
            for file in self.files.iter().filter(|file| !file.passed) {
                write!(f, "  FAILED: {}", file.location)?;
                if let Some(ref err) = file.error {
                    write!(f, " ({})", err)?;
                }
                writeln!(f)?;
            }
            Ok(())
        }
    }
}

impl VerificationResults {
    /// Create a new VerificationResults from a list of file results
    pub fn from_files(files: Vec<FileVerificationResult>) -> Self {
        let passed_count = files.iter().filter(|f| f.passed).count();
        let failed_count = files.iter().filter(|f| !f.passed).count();
        let skipped_count = files
            .iter()
            .filter(|f| f.passed && f.expected_hash.is_none())
            .count();
        let versions_updated_count = files.iter().filter(|f| f.version_updated).count();
        let all_passed = failed_count == 0;

        Self {
            files,
            passed_count,
            failed_count,
            skipped_count,
            versions_updated_count,
            all_passed,
        }
    }

    /// Check verification results and return error if any files failed.
    ///
    /// Throws an error if:
    /// - Any file has a checksum mismatch (hash doesn't match)
    /// - Any file has a verification error
    pub fn check(&self) -> Result<(), BundlebaseError> {
        let failures: Vec<&FileVerificationResult> =
            self.files.iter().filter(|f| !f.passed).collect();

        if failures.is_empty() {
            return Ok(());
        }

        let messages: Vec<String> = failures
            .iter()
            .map(|f| {
                if let Some(ref err) = f.error {
                    format!("{}: {}", f.location, err)
                } else if f.expected_hash != f.actual_hash {
                    format!(
                        "{}: hash mismatch (expected {}, got {})",
                        f.location,
                        f.expected_hash.as_deref().unwrap_or("none"),
                        f.actual_hash.as_deref().unwrap_or("none")
                    )
                } else {
                    format!("{}: verification failed", f.location)
                }
            })
            .collect();

        Err(BundlebaseError::from(format!(
            "Data verification failed for {} file(s):\n{}",
            failures.len(),
            messages.join("\n")
        )))
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

    pub(crate) operations: Arc<RwLock<Vec<AnyOperation>>>,

    packs: Arc<RwLock<HashMap<ObjectId, Arc<Pack>>>>,
    sources: Arc<RwLock<HashMap<ObjectId, Arc<Source>>>>,
    indexes: Arc<RwLock<Vec<Arc<IndexDefinition>>>>,
    views: Arc<RwLock<HashMap<String, ObjectId>>>,
    dataframe: DataFrameHolder,

    ctx: Arc<SessionContext>,
    storage: Arc<DataStorage>,
    pub(crate) reader_factory: Arc<DataReaderFactory>,
    function_registry: Arc<RwLock<FunctionRegistry>>,
    source_function_registry: Arc<RwLock<SourceFunctionRegistry>>,

    /// Final merged configuration (explicit + stored), used for all operations
    /// This is computed once and updated when SetConfigOp is applied
    config: Arc<RwLock<Arc<BundleConfig>>>,

    /// Config passed to create()/open() (preserved for re-merging after SetConfigOp)
    passed_config: Arc<RwLock<Option<BundleConfig>>>,

    /// Config stored via SetConfigOp operations (preserved for re-merging)
    stored_config: Arc<RwLock<BundleConfig>>,

    /// True if this bundle is a view (has a view field in init commit)
    is_view: Arc<RwLock<bool>>,
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
            dataframe: DataFrameHolder {
                dataframe: Arc::new(RwLock::new(self.dataframe.dataframe.read().clone())),
            },
            ctx: Arc::clone(&self.ctx),
            storage: Arc::clone(&self.storage),
            reader_factory: Arc::clone(&self.reader_factory),
            function_registry: Arc::clone(&self.function_registry),
            source_function_registry: Arc::clone(&self.source_function_registry),
            config: Arc::clone(&self.config),
            passed_config: Arc::clone(&self.passed_config),
            stored_config: Arc::clone(&self.stored_config),
            is_view: Arc::clone(&self.is_view),
        }
    }
}

impl Bundle {
    /// Creates an empty bundle wrapped in Arc with schema providers registered.
    ///
    /// Returns `Arc<Self>` ready for use. Schema providers are registered with the
    /// Bundle as the facade. BundleBuilder will re-register with itself as facade.
    pub async fn empty() -> Result<Arc<Self>, BundlebaseError> {
        let url = Url::parse(EMPTY_URL)?;

        let storage = Arc::new(DataStorage::new());
        let function_registry = Arc::new(RwLock::new(FunctionRegistry::new()));
        let source_function_registry = Arc::new(RwLock::new(SourceFunctionRegistry::new()));

        let mut config =
            SessionConfig::new().with_default_catalog_and_schema(CATALOG_NAME, "default");
        let options = config.options_mut();
        options.sql_parser.enable_ident_normalization = false;
        let ctx = Arc::new(SessionContext::new_with_config(config));

        let packs = Arc::new(RwLock::new(HashMap::new()));
        let commits = Arc::new(RwLock::new(vec![]));
        let indexes = Arc::new(RwLock::new(Vec::new()));
        let views = Arc::new(RwLock::new(HashMap::new()));
        let sources = Arc::new(RwLock::new(HashMap::new()));
        let operations = Arc::new(RwLock::new(Vec::new()));

        let id = Arc::new(RwLock::new(Uuid::new_v4().to_string()));
        let name = Arc::new(RwLock::new(None));
        let description = Arc::new(RwLock::new(None));
        let version = Arc::new(RwLock::new("empty".to_string()));

        let empty_dataframe = DataFrame::new(
            ctx.state(),
            LogicalPlan::EmptyRelation(EmptyRelation {
                produce_one_row: false,
                schema: DFSchemaRef::new(DFSchema::empty()),
            }),
        );

        let dataframe = DataFrameHolder::new(Some(empty_dataframe));

        // Register version() UDF with initial "empty" version
        ctx.register_udf(ScalarUDF::new_from_impl(VersionUdf::new("empty".to_string())));

        ctx.register_object_store(
            ObjectStoreUrl::parse("memory://")?.as_ref(),
            crate::io::get_memory_store(),
        );
        ctx.register_object_store(
            ObjectStoreUrl::parse(format!("{}://", EMPTY_SCHEME))?.as_ref(),
            crate::io::get_null_store(),
        );

        let data_dir = Arc::new(RwLock::new(writable_dir_from_url(&url, BundleConfig::default().into())?));
        let bundle_config = Arc::new(RwLock::new(Arc::new(crate::BundleConfig::new())));

        let bundle = Arc::new(Self {
            ctx: Arc::clone(&ctx),
            id,
            packs,
            sources,
            indexes,
            views,
            storage: Arc::clone(&storage),
            reader_factory: DataReaderFactory::new(
                Arc::clone(&function_registry),
                Arc::clone(&storage),
            )
                .into(),
            function_registry,
            source_function_registry,
            name,
            description,
            operations,
            last_manifest_version: Arc::new(RwLock::new(0)),
            version,
            data_dir,
            commits,
            dataframe,
            config: bundle_config,
            passed_config: Arc::new(RwLock::new(None)),
            stored_config: Arc::new(RwLock::new(BundleConfig::new())),
            is_view: Arc::new(RwLock::new(false)),
        });

        // Register schema providers with Bundle as the facade
        Self::register_schema_providers(&ctx, bundle.clone())?;

        Ok(bundle)
    }

    /// Register schema providers with the SessionContext's catalog.
    ///
    /// Called after Bundle/BundleBuilder is wrapped in Arc. Creates all schema providers
    /// with the facade reference and registers them with the catalog.
    pub(crate) fn register_schema_providers(
        ctx: &SessionContext,
        facade: Arc<dyn BundleFacade>,
    ) -> Result<(), BundlebaseError> {
        let catalog = ctx.catalog(CATALOG_NAME).expect("Default catalog not found");

        // Register temp schema (doesn't need facade)
        catalog.register_schema("temp", Arc::new(MemorySchemaProvider::new()))?;

        catalog.register_schema(
            "blocks",
            Arc::new(BlockSchemaProvider::new(facade.clone())),
        )?;
        catalog.register_schema(
            "packs",
            Arc::new(PackSchemaProvider::new(facade.clone())),
        )?;
        catalog.register_schema(
            "default",
            Arc::new(DefaultSchemaProvider::new(facade.clone())),
        )?;
        catalog.register_schema(
            "bundle_info",
            Arc::new(BundleInfoSchemaProvider::new(facade)),
        )?;

        Ok(())
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
    pub async fn open(path: &str, config: Option<BundleConfig>) -> Result<Arc<Self>, BundlebaseError> {
        let mut visited = HashSet::new();
        let arc_bundle = Self::empty().await?;

        arc_bundle.add_pack(ObjectId::BASE_PACK, Arc::new(Pack::new_base()));

        // Set explicit config if provided and recompute merged config
        *arc_bundle.passed_config.write() = config;
        arc_bundle.recompute_config()?;

        Self::open_recursive(
            writable_dir_from_str(path, BundleConfig::default().into())?
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

        let data_dir = writable_dir_from_str(url, bundle.config())?;
        let manifest_dir = data_dir.writable_subdir(META_DIR)?;

        debug!("Loading initial commit from {}", INIT_FILENAME);

        let init_commit: Option<InitCommit> = read_yaml(manifest_dir.file(INIT_FILENAME)?.as_ref()).await?;
        let init_commit = init_commit
            .expect(format!("No {}/{} found in {}", META_DIR, INIT_FILENAME, url).as_str());

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

        // Load and apply each manifest in order
        for manifest_file_info in manifest_files {
            *bundle.last_manifest_version.write() = manifest_version(manifest_file_info.filename().unwrap_or(""));
            // Create IOFile from FileInfo to read the manifest
            let manifest_file = readable_file_from_url(&manifest_file_info.url, bundle.config())?;
            let mut commit: BundleCommit = read_yaml(manifest_file.as_ref()).await?.ok_or_else(|| {
                BundlebaseError::from(format!("Failed to read manifest: {}", manifest_file_info.url))
            })?;
            commit.url = Some(manifest_file_info.url.clone());
            commit.data_dir = Some(data_dir.url().clone());

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
                            AnyOperation::CreateView(_) | AnyOperation::RenameView(_) | AnyOperation::DropView(_) => {
                                debug!("    Skipping (view operation in view): {}", op.describe());
                                continue;
                            }
                            _ => {}
                        }
                    }
                    debug!("    Applying: {}", op.describe());
                    bundle.apply_operation(op).await?;
                }
            }
        }
        Ok(())
    }

    /// Creates a BundleBuilder that extends this bundle.
    /// If data_dir is provided, stores the new bundle there; otherwise uses the current bundle's data_dir.
    pub fn extend(&self, data_dir: Option<&str>) -> Result<Arc<BundleBuilder>, BundlebaseError> {
        BundleBuilder::extend(Arc::new(self.clone()), data_dir)
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
        self.operations.write().push(op);

        self.compute_version();
        // clear cached values
        self.dataframe.clear();
        debug!("Cleared dataframe");

        debug!("Applying operation to bundle: {}...DONE", &description);

        Ok(())
    }

    pub fn data_dir(&self) -> Arc<dyn IOReadWriteDir> {
        Arc::clone(&*self.data_dir.read())
    }

    pub fn config(&self) -> Arc<BundleConfig> {
        Arc::clone(&*self.config.read())
    }

    /// Recompute the merged config and recreate data_dir with it
    ///
    /// Merges stored_config and explicit_config (with explicit taking priority),
    /// then recreates data_dir with the new merged config.
    ///
    /// Priority order:
    /// 1. Explicit config passed to create()/open() (highest)
    /// 2. Config stored via SetConfigOp operations (lowest)
    pub(crate) fn recompute_config(&self) -> Result<(), BundlebaseError> {
        // Merge stored_config with explicit_config (explicit takes priority)
        let stored_config = self.stored_config.read().clone();
        let passed_config = self.passed_config.read().clone();
        let merged = if let Some(ref explicit) = passed_config {
            stored_config.merge(explicit)
        } else {
            stored_config
        };

        // Update the config field
        let new_config = Arc::new(merged);
        *self.config.write() = Arc::clone(&new_config);

        // Recreate data_dir with the new config
        let url = self.data_dir.read().url().clone();
        *self.data_dir.write() = writable_dir_from_url(&url, new_config)?;

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
        *self.stored_config.write() = other.stored_config.read().clone();
        *self.passed_config.write() = other.passed_config.read().clone();
        *self.config.write() = Arc::clone(&*other.config.read());
        *self.data_dir.write() = Arc::clone(&*other.data_dir.read());
        *self.is_view.write() = *other.is_view.read();
        self.dataframe.clear();
    }

    pub fn ctx(&self) -> Arc<SessionContext> {
        self.ctx.clone()
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

    /// Joins the pack with join metadata to the base dataframe
    async fn dataframe_join(
        &self,
        base_df: DataFrame,
        pack: &Pack,
    ) -> Result<DataFrame, BundlebaseError> {
        let base_table = format!(
            "packs.{}",
            Pack::table_name(&ObjectId::BASE_PACK)
        );
        let join_table = format!("packs.{}", Pack::table_name(pack.id()));

        let expr = sql::parse_join_expr(&self.ctx, &base_table, pack).await?;

        let base_df = base_df.alias(sql::BASE_PACK_NAME)?;

        let name = pack.name();

        // Safe to unwrap since we only call this for packs with join metadata
        let join_type = pack.join_type().expect("Pack must have join_type for join");

        Ok(base_df.join_on(
            self.ctx.table(&join_table).await?.alias(name)?,
            join_type.to_datafusion(),
            expr,
        )?)
    }

    fn compute_version(&self) {
        let mut hasher = Sha256::new();

        for op in self.operations.read().iter() {
            hasher.update(op.version().as_bytes());
        }

        let new_version = hex::encode(hasher.finalize())[0..12].to_string();
        *self.version.write() = new_version.clone();

        // Re-register version() UDF with the updated version
        self.ctx
            .register_udf(ScalarUDF::new_from_impl(VersionUdf::new(new_version)));
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
    }

    /// Find a join pack by its name
    pub(crate) fn pack_by_name(&self, name: &str) -> Option<Arc<Pack>> {
        self.packs
            .read()
            .values()
            .find(|p| p.name() == name)
            .cloned()
    }

    /// Get a pack's name by its ID
    pub(crate) fn pack_name(&self, pack_id: &ObjectId) -> Option<String> {
        self.packs
            .read()
            .get(pack_id)
            .map(|p| p.name().to_string())
    }

    /// Get all join pack names
    pub(crate) fn join_names(&self) -> Vec<String> {
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

    /// Check if an index already exists at the correct version
    pub(crate) fn get_index(
        &self,
        column: &str,
        block: &VersionedBlockId,
    ) -> Option<Arc<IndexedBlocks>> {
        for index in self.indexes.read().iter() {
            if index.column() == column {
                if let Some(indexed_blocks) = index.indexed_blocks(block) {
                    return Some(indexed_blocks);
                }
            }
        }
        None
    }

    /// Add a source definition to the bundle
    pub(crate) fn add_source(&self, op: CreateSourceOp) {
        let registry = self.source_function_registry.read();
        if let Ok(source) = Source::from_op(&op, &registry) {
            self.sources.write().insert(op.id, Arc::new(source));
        }
    }

    /// Get a source by its ID
    pub(crate) fn get_source(&self, source_id: &ObjectId) -> Option<Arc<Source>> {
        self.sources.read().get(source_id).cloned()
    }

    /// Get all sources for a specific pack
    pub(crate) fn get_sources_for_pack(&self, pack_id: &ObjectId) -> Vec<Arc<Source>> {
        self.sources
            .read()
            .values()
            .filter(|s| s.pack() == pack_id)
            .cloned()
            .collect()
    }

    /// Get all sources
    pub(crate) fn sources(&self) -> HashMap<ObjectId, Arc<Source>> {
        self.sources.read().clone()
    }

    /// Find a block by ID across all packs
    pub(crate) fn find_block(&self, block_id: &ObjectId) -> Option<Arc<DataBlock>> {
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

    /// Get the source function registry
    pub(crate) fn source_function_registry(&self) -> Arc<RwLock<SourceFunctionRegistry>> {
        Arc::clone(&self.source_function_registry)
    }

    /// Build a map of block IDs to their expected hashes from operations.
    ///
    /// Searches through AttachBlockOp and ReplaceBlockOp operations to build
    /// a mapping from block ID to the expected hash. For blocks that have been
    /// replaced, uses the hash from the most recent ReplaceBlockOp.
    pub fn build_block_hash_map(&self) -> HashMap<ObjectId, String> {
        let mut block_hashes: HashMap<ObjectId, String> = HashMap::new();

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
    fn build_block_location_map(&self) -> HashMap<ObjectId, String> {
        let mut block_locations: HashMap<ObjectId, String> = HashMap::new();

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
                let location = block_locations.get(block_id).cloned().unwrap_or_else(|| {
                    block.reader().url().to_string()
                });

                // Skip function:// URLs (generated data has no file to verify)
                if location.starts_with("function://") {
                    results.push(FileVerificationResult {
                        location,
                        file_type: "data".to_string(),
                        expected_hash: None,
                        actual_hash: None,
                        passed: true,
                        error: None,
                        version_updated: false,
                    });
                    continue;
                }

                let expected_hash = block_hashes.get(block_id).cloned();

                match self.verify_block_hash(&location, expected_hash.as_deref()).await {
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

        let file = readable_file_from_path(location, self.data_dir(), self.config())?;
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
        self.version.read().clone()
    }

    /// Returns the commit history for this bundle, starting with any base bundles
    fn history(&self) -> Vec<BundleCommit> {
        self.commits.read().clone()
    }

    fn operations(&self) -> Vec<AnyOperation> {
        self.operations.read().clone()
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

            // Collect join packs first (release lock before async calls)
            let join_packs: Vec<Arc<Pack>> = self
                .packs
                .read()
                .values()
                .filter(|p| p.is_join())
                .cloned()
                .collect();

            // Join all packs that have join metadata
            for pack in join_packs {
                debug!("Executing join with pack {}", pack.id());
                df = self.dataframe_join(df, &pack).await?;
            }

            // Clone operations to avoid holding lock across async calls
            let ops = self.operations.read().clone();

            // Apply operations to the base DataFrame
            debug!(
                    "dataframe: Applying {} operations to dataframe...",
                    ops.len()
                );

            for op in ops.iter() {
                debug!("Applying to dataframe: {}", &op.describe());
                df = op.apply_dataframe(df, self.ctx.clone()).await?;
            }
            debug!(
                    "dataframe: Applying {} operations to dataframe...DONE",
                    ops.len()
                );

            df
        } else {
            // No base pack, or base pack has no data yet
            debug!("No base pack or empty base pack, using empty dataframe");
            DataFrame::new(
                self.ctx().state(),
                LogicalPlan::EmptyRelation(EmptyRelation {
                    produce_one_row: false,
                    schema: DFSchemaRef::new(DFSchema::empty()),
                }),
            )
        };
        self.dataframe.replace(df);
        debug!("Building dataframe...DONE");
        Ok(self.dataframe.dataframe())
    }

    async fn select(
        &self,
        sql: &str,
        params: Vec<ScalarValue>,
    ) -> Result<Arc<BundleBuilder>, BundlebaseError> {
        let bundle = BundleBuilder::extend(Arc::new(self.clone()), None)?;
        bundle.select(sql, params).await
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
        // Preserve explicit_config from current bundle
        let config = self.passed_config.read().clone();
        Bundle::open(&view_path, config).await
    }

    fn views(&self) -> HashMap<ObjectId, String> {
        // Reverse the name->id HashMap to id->name
        self.views
            .read()
            .iter()
            .map(|(name, id)| (*id, name.clone()))
            .collect()
    }

    async fn export_tar(&self, tar_path: &str) -> Result<String, BundlebaseError> {
        use futures::StreamExt;
        use std::fs::File;
        use tar::{Builder, Header};

        let tar_file = File::create(tar_path).map_err(|e| {
            format!("Failed to create tar file '{}': {}", tar_path, e)
        })?;
        let mut builder = Builder::new(tar_file);

        // Get all files from the bundle's data_dir
        let data_dir = self.data_dir();
        let files = data_dir.list_files().await?;

        debug!("Exporting {} files to tar archive", files.len());

        for file in files {
            // Extract relative path from file URL
            let file_url = &file.url;
            let base_url = data_dir.url();

            let relative_path = if file_url.as_str().starts_with(base_url.as_str()) {
                &file_url.as_str()[base_url.as_str().len()..]
            } else {
                return Err(format!(
                    "File URL '{}' is not under base URL '{}'",
                    file_url, base_url
                )
                    .into());
            };

            // Remove leading slash if present
            let relative_path = relative_path.trim_start_matches('/');

            debug!("Adding file to tar: {}", relative_path);

            // Read file contents via stream
            let io_file = readable_file_from_url(&file.url, self.config())?;
            let mut stream = io_file.read_stream().await?.ok_or_else(|| {
                BundlebaseError::from(format!("File not found: {}", file.url))
            })?;

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
            header.set_mtime(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("BUG: current time should be after Unix epoch")
                    .as_secs(),
            );
            header.set_cksum();

            // Append to tar
            builder
                .append_data(&mut header, relative_path, &buffer[..])
                .map_err(|e| {
                    format!("Failed to append file '{}' to tar: {}", relative_path, e)
                })?;
        }

        // Finish writing tar (writes footer)
        builder.finish().map_err(|e| {
            format!("Failed to finalize tar archive: {}", e)
        })?;

        info!("Exported bundle to tar archive: {}", tar_path);
        Ok(format!("Exported bundle to {}", tar_path))
    }

    async fn explain(&self) -> Result<String, BundlebaseError> {
        Bundle::explain(self).await
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

    fn views_by_name(&self) -> HashMap<String, ObjectId> {
        self.views.read().clone()
    }

    fn data_dir(&self) -> Arc<dyn IOReadWriteDir> {
        Bundle::data_dir(self)
    }

    fn config(&self) -> Arc<BundleConfig> {
        Bundle::config(self)
    }

    fn ctx(&self) -> Arc<SessionContext> {
        Bundle::ctx(self)
    }
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
        let c = Bundle::empty().await?;
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
        use arrow::array::StringArray;

        let c = Bundle::empty().await?;

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
}
