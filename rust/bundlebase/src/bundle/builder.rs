use crate::bundle::facade::BundleFacade;
use crate::bundle::function_entry::FunctionRegistry;
use crate::bundle::init::InitCommit;
use crate::bundle::operation::AnyOperation;
use crate::bundle::operation::{BundleChange, IndexBlocksOp, Operation, SharedAttachContext};
use crate::bundle::{bundle_schema, sql, AlwaysUpdateRule, Bundle, ReportEntry};
use crate::bundle::{commit, Pack, INIT_FILENAME, META_DIR};
use crate::bundle_config::{PassedBundleConfig, Scope};
use crate::data::{BlockId, ObjectId, ObjectIdAlias, VersionedBlockId};
use crate::index::IndexDefinition;
use crate::io::{writable_dir_from_str, writable_dir_from_url, write_yaml, IOReadWriteDir};
use crate::source::ConnectorRegistry;
use crate::BundleConfig;
use crate::BundlebaseError;
use arrow::array::{Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use bundlebase_common::command_response::{single_batch_stream, CommandResponse, OutputShape};
use bundlebase_common::impl_dyn_command_response;
use chrono::DateTime;
use datafusion::execution::SendableRecordBatchStream;
use datafusion::prelude::{DataFrame, SessionContext};
use datafusion::scalar::ScalarValue;
use parking_lot::RwLock;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::future::Future;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::{Arc, Weak};
use tracing::{debug, info};
use url::Url;

/// Format a system time as ISO8601 UTC string (e.g., "2024-01-01T12:34:56Z")
fn to_iso(time: std::time::SystemTime) -> String {
    let datetime: DateTime<chrono::Utc> = time.into();
    datetime.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Bundle status showing uncommitted changes.
///
/// Represents the current state of a BundleBuilder with information about
/// all the operations that have been queued but not yet committed.
#[derive(Debug, Clone, Default)]
pub struct BundleStatus {
    /// The changes that represent the changes since creation/extension
    changes: Vec<BundleChange>,
}

impl BundleStatus {
    /// Create a new bundle status from changes
    pub fn new() -> Self {
        BundleStatus { changes: vec![] }
    }

    /// Check if there are any changes
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    pub(in crate::bundle) fn clear(&mut self) {
        self.changes.clear();
    }

    pub fn pop(&mut self) {
        self.changes.pop();
    }

    pub fn pop_change(&mut self) -> Option<BundleChange> {
        self.changes.pop()
    }

    pub fn push_change(&mut self, change: BundleChange) {
        self.changes.push(change);
    }

    pub fn truncate(&mut self, len: usize) {
        self.changes.truncate(len);
    }

    pub fn changes(&self) -> &Vec<BundleChange> {
        &self.changes
    }

    pub fn operations(&self) -> Vec<AnyOperation> {
        self.changes
            .iter()
            .flat_map(|g| g.operations.clone())
            .collect()
    }

    /// Get the total number of operations across all changes
    pub fn operations_count(&self) -> usize {
        self.changes.iter().map(|g| g.operations.len()).sum()
    }
}

impl std::fmt::Display for BundleStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_empty() {
            write!(f, "No uncommitted changes")
        } else {
            writeln!(
                f,
                "Bundle Status: {} change(s), {} total operation(s)",
                self.changes().len(),
                self.operations_count()
            )?;
            for (idx, change) in self.changes.iter().enumerate() {
                write!(
                    f,
                    "  [{}] {} ({} operation{})",
                    idx + 1,
                    change.description,
                    change.operations.len(),
                    if change.operations.len() == 1 {
                        ""
                    } else {
                        "s"
                    }
                )?;
                if idx < self.changes.len() - 1 {
                    writeln!(f)?;
                }
            }
            Ok(())
        }
    }
}

impl CommandResponse for BundleStatus {
    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("change_id", DataType::Utf8, false),
            Field::new("description", DataType::Utf8, false),
            Field::new("operation_count", DataType::Int32, false),
        ]))
    }

    fn output_shape() -> OutputShape {
        OutputShape::Table
    }

    fn into_stream(self: Box<Self>) -> Result<SendableRecordBatchStream, BundlebaseError> {
        let changes = self.changes();

        let ids: Vec<i32> = (0..changes.len() as i32).collect();
        let change_ids: Vec<String> = changes.iter().map(|c| c.id.to_string()).collect();
        let descriptions: Vec<&str> = changes.iter().map(|c| c.description.as_str()).collect();
        let operation_counts: Vec<i32> =
            changes.iter().map(|c| c.operations.len() as i32).collect();

        let batch = RecordBatch::try_new(
            Self::schema(),
            vec![
                Arc::new(Int32Array::from(ids)),
                Arc::new(StringArray::from(
                    change_ids.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(descriptions)),
                Arc::new(Int32Array::from(operation_counts)),
            ],
        )
        .map_err(|e| BundlebaseError::from(format!("Failed to create record batch: {}", e)))?;
        single_batch_stream(Self::schema(), batch)
    }

    impl_dyn_command_response!(BundleStatus);
}

/// A modifiable Bundle with interior mutability for thread-safe access.
///
/// `BundleBuilder` represents a bundle during the development/transformation phase.
/// It tracks both operations that have been previously committed (via the `existing` base) and
/// new operations added since the working copy was created or extended.
///
/// # Key Characteristics
/// - **Interior Mutability**: Methods take `&self` and use internal locking
/// - **Thread-Safe**: Can be shared via `Arc<BundleBuilder>` across threads
/// - **Fluent API**: Methods return `Result<&Self, BundlebaseError>` enabling chaining with `?`
/// - **Commit**: Call `commit()` to persist all operations to disk
///
/// # Lock Acquisition Order
///
/// When acquiring multiple locks, always follow this order to prevent deadlocks:
/// 1. `bundle` lock (read or write)
/// 2. `in_progress_change` lock (read or write)
///
/// Never acquire `in_progress_change` first and then `bundle`. If you need both locks,
/// acquire `bundle` first, release it if needed, then acquire `in_progress_change`.
///
/// **Note:** Due to async await points, locks should generally not be held across awaits.
/// The pattern used is: acquire lock, extract/clone needed data, release lock, then await.
///
/// # Example
/// ```ignore
/// let builder = BundleBuilder::create("memory://work", None).await?;
/// builder.attach("data.parquet", None).await?
///     .filter("amount > 100", vec![]).await?
///     .commit("Filter high-value transactions").await?;
/// ```
pub struct BundleBuilder {
    /// The underlying bundle data. Bundle is internally thread-safe via Arc<RwLock<T>> fields.
    bundle: Arc<Bundle>,
    /// Tracks the current in-progress change being built.
    in_progress_change: RwLock<Option<BundleChange>>,
    /// Tracks uncommitted changes for this builder.
    status: RwLock<BundleStatus>,
    /// RowIds accumulated by DELETE commands, written to a tombstone file on commit.
    pending_deletes: RwLock<std::collections::HashSet<bundlebase_common::RowId>>,
    /// WHERE clauses from DELETE commands, stored for historical reference in the operation log.
    pending_delete_wheres: RwLock<Vec<String>>,
    /// Updated cell values accumulated by UPDATE commands, written to an overlay parquet on commit.
    /// Maps RowId → (ColumnId → ScalarValue).
    pending_updates: RwLock<
        std::collections::HashMap<
            bundlebase_common::RowId,
            std::collections::HashMap<crate::object_id::ColumnId, datafusion::scalar::ScalarValue>,
        >,
    >,
    /// WHERE clauses from UPDATE commands, stored for historical reference in the operation log.
    pending_update_wheres: RwLock<Vec<String>>,
}

impl bundlebase_data::DataContext for BundleBuilder {
    fn config_provider(&self) -> Arc<dyn crate::ConfigProvider> {
        self.bundle.config() as Arc<dyn crate::ConfigProvider>
    }

    fn data_context_dir(&self) -> Arc<dyn crate::io::IOReadWriteDir> {
        self.bundle.data_dir()
    }

    fn session_context(&self) -> Arc<datafusion::prelude::SessionContext> {
        self.bundle.ctx()
    }
}

impl Clone for BundleBuilder {
    fn clone(&self) -> Self {
        Self {
            bundle: Arc::clone(&self.bundle),
            in_progress_change: RwLock::new(self.in_progress_change.read().clone()),
            status: RwLock::new(self.status.read().clone()),
            pending_deletes: RwLock::new(self.pending_deletes.read().clone()),
            pending_delete_wheres: RwLock::new(self.pending_delete_wheres.read().clone()),
            pending_updates: RwLock::new(self.pending_updates.read().clone()),
            pending_update_wheres: RwLock::new(self.pending_update_wheres.read().clone()),
        }
    }
}

/// Type alias for boxed futures used in do_change closures
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

impl BundleBuilder {
    /// Build a [`SharedAttachContext`] seeded with the bundle's existing
    /// column-name -> ColumnId map. Pass into attach setup for a batch of
    /// parallel attaches.
    pub fn shared_attach_context(&self) -> Arc<SharedAttachContext> {
        let existing_ops = self.operations();
        let resolved = bundle_schema::BundleSchema::resolved(&existing_ops);
        let map: HashMap<String, crate::object_id::ColumnId> = resolved
            .columns()
            .iter()
            .map(|(id, name)| (name.clone(), *id))
            .collect();
        Arc::new(SharedAttachContext {
            name_to_id: parking_lot::Mutex::new(map),
            schema_paths: parking_lot::Mutex::new(HashMap::new()),
            column_ids_paths: parking_lot::Mutex::new(HashMap::new()),
        })
    }

    /// Creates a new empty BundleBuilder in a working directory.
    ///
    /// # Arguments
    /// * `path` - Path to the working directory for the bundle. Can be a URL or a filesystem path (local or relative). e.g., `memory://work`, `file:///tmp/bundle`
    ///
    /// # Returns
    /// An empty bundle ready for data attachment and transformations.
    ///
    /// # Example
    /// ```ignore
    /// let builder = BundleBuilder::create("memory://work", None).await?;
    /// builder.attach("data.parquet", None).await?;
    /// ```
    pub async fn create(
        path: &str,
        config: Option<PassedBundleConfig>,
    ) -> Result<Arc<BundleBuilder>, BundlebaseError> {
        let bundle = Bundle::empty(config).await?;
        bundle.refresh_data_dir().await?;
        *bundle.data_dir.write() = writable_dir_from_str(path, bundle.config()).await?;

        // Check if a bundle already exists at this location
        let meta_dir = bundle.data_dir().writable_subdir(META_DIR)?;
        let init_file = meta_dir.file(INIT_FILENAME)?;
        if init_file.exists().await? {
            return Err(format!(
                "A bundle already exists at '{}'. Use open() to access an existing bundle.",
                path
            )
            .into());
        }

        // Automatically create the base pack with a well-known ID
        bundle.add_pack(ObjectId::BASE_PACK, Arc::new(Pack::new_base()));

        let builder = Arc::new(BundleBuilder {
            bundle,
            in_progress_change: RwLock::new(None),
            status: RwLock::new(BundleStatus::new()),
            pending_deletes: RwLock::new(std::collections::HashSet::new()),
            pending_delete_wheres: RwLock::new(Vec::new()),
            pending_updates: RwLock::new(std::collections::HashMap::new()),
            pending_update_wheres: RwLock::new(Vec::new()),
        });

        // Re-register schema providers and the search() UDTF with BundleBuilder as
        // facade (using Weak to avoid Arc cycle). This overwrites the Bundle-facade
        // registrations from empty_internal(), so bundle_info tables and search()
        // see uncommitted changes from BundleBuilder.
        let facade_weak = Arc::downgrade(&builder) as Weak<dyn BundleFacade>;
        crate::catalog::register_schema_providers(&builder.bundle.ctx, facade_weak.clone())?;
        builder.bundle.ctx.register_udtf(
            "search",
            Arc::new(crate::index::SearchTableFunction::new(facade_weak)),
        );

        Ok(builder)
    }

    /// Creates a new BundleBuilder extending from an existing Bundle.
    ///
    /// # Arguments
    /// * `bundle` - The source bundle to extend from
    /// * `data_dir` - Optional new data directory. If None, uses the current bundle's data_dir.
    ///
    /// # Status Independence
    ///
    /// The returned builder has **independent** status tracking from the source bundle.
    /// Changes made to this builder will not appear in the original bundle's status,
    /// and vice versa.
    pub async fn extend(
        bundle: Arc<Bundle>,
        data_dir: Option<&str>,
    ) -> Result<Arc<BundleBuilder>, BundlebaseError> {
        let mut new_bundle = bundle.deref().clone();

        // Detach data_dir and last_manifest_version so modifications don't affect the original
        new_bundle.detach_for_extend();

        // If data_dir is provided and not empty, use it; otherwise keep the current bundle's data_dir
        if let Some(dir) = data_dir {
            if !dir.is_empty() {
                let new_data_dir = writable_dir_from_str(dir, bundle.config()).await?;
                if *new_data_dir.url() != bundle.url() {
                    *new_bundle.last_manifest_version.write() = 0;
                }
                *new_bundle.data_dir.write() = new_data_dir;
            }
        }

        let builder = Arc::new(BundleBuilder {
            bundle: Arc::new(new_bundle),
            in_progress_change: RwLock::new(None),
            status: RwLock::new(BundleStatus::new()),
            pending_deletes: RwLock::new(std::collections::HashSet::new()),
            pending_delete_wheres: RwLock::new(Vec::new()),
            pending_updates: RwLock::new(std::collections::HashMap::new()),
            pending_update_wheres: RwLock::new(Vec::new()),
        });

        // Re-register schema providers and the search() UDTF with BundleBuilder as
        // facade (using Weak to avoid Arc cycle). This overwrites the Bundle-facade
        // registrations from Bundle::open(), so bundle_info tables and search()
        // see uncommitted changes from BundleBuilder. The search() UDTF re-registration
        // is also load-bearing on its own: Bundle::open()'s weak ref points at the
        // original Arc<Bundle>, which is dropped after extend() clones the Bundle into
        // a new Arc — without this re-registration the upgrade fails at query time.
        let facade_weak = Arc::downgrade(&builder) as Weak<dyn BundleFacade>;
        crate::catalog::register_schema_providers(&builder.bundle.ctx, facade_weak.clone())?;
        builder.bundle.ctx.register_udtf(
            "search",
            Arc::new(crate::index::SearchTableFunction::new(facade_weak)),
        );

        Ok(builder)
    }

    /// Read access to the inner bundle
    pub fn bundle(&self) -> &Bundle {
        &self.bundle
    }

    /// Returns the bundle status showing uncommitted changes.
    pub fn status(&self) -> BundleStatus {
        self.status.read().clone()
    }

    /// Upgrade a bundle's format version to the current bundlebase version.
    ///
    /// Writes a new commit directly to the manifest directory without opening
    /// the bundle, since opening would fail if the version is out of range.
    pub async fn upgrade_bundle(
        path: &str,
        config: Option<PassedBundleConfig>,
    ) -> Result<(), BundlebaseError> {
        use crate::bundle::commit::BundleCommit;
        use crate::bundle::operation::{BundleChange, SetMaxVersionOp, SetMinVersionOp};

        let bundle_config = Arc::new(BundleConfig::new(config.as_ref())?);
        let config_provider: Arc<dyn crate::ConfigProvider> =
            Arc::clone(&bundle_config) as Arc<dyn crate::ConfigProvider>;
        let data_dir = crate::io::writable_dir_from_str(path, config_provider.clone()).await?;
        let manifest_dir = data_dir.writable_subdir(META_DIR)?;

        // Verify this is a bundle (init file exists)
        let init_file = manifest_dir.file(INIT_FILENAME)?;
        if !init_file.exists().await? {
            return Err(format!("No bundle found at '{}'", path).into());
        }

        // Find the latest manifest version
        let manifest_files = manifest_dir.list_files().await?;
        let last_version = manifest_files
            .iter()
            .filter(|f| f.filename() != Some(INIT_FILENAME))
            .filter_map(|f| f.filename())
            .filter(|name| !name.contains('/'))
            .map(|name| commit::manifest_version(name))
            .max()
            .unwrap_or(0);

        let next_version = last_version + 1;
        let version = bundlebase_common::format_version_string();
        let desc = format!("Upgrade bundle format to {}", version);

        let timestamp = to_iso(std::time::SystemTime::now());
        let author = std::env::var("BUNDLEBASE_AUTHOR")
            .unwrap_or_else(|_| std::env::var("USER").unwrap_or_else(|_| "unknown".to_string()));

        let mut change = BundleChange::new(&desc);
        change
            .operations
            .push(SetMinVersionOp::setup(&version).into());
        change
            .operations
            .push(SetMaxVersionOp::setup(&version).into());

        let commit_struct = BundleCommit {
            url: None,
            data_dir: None,
            message: desc,
            author,
            timestamp,
            changes: vec![change],
        };

        let yaml = serde_yaml_ng::to_string(&commit_struct)?;

        let mut hasher = Sha256::new();
        hasher.update(yaml.as_bytes());
        let hash_bytes = hasher.finalize();
        let hash_hex = hex::encode(hash_bytes);
        let hash_short = &hash_hex[..12];

        let filename = format!("{:05}{}.yaml", next_version, hash_short);
        let manifest_file = manifest_dir.writable_file(&filename)?;

        let data = bytes::Bytes::from(yaml);
        let stream = futures::stream::iter(vec![Ok::<_, std::io::Error>(data)]);
        manifest_file.write_stream(Box::pin(stream)).await?;

        Ok(())
    }

    /// Convert a JSON file to Parquet using normalization options, storing the result in the data dir.
    ///
    /// Used by fetch and create_source when `json_record_path` is present in connector args.
    /// Returns the relative path of the new Parquet file and its SHA256 hash.
    pub async fn convert_json_attachment_to_parquet(
        &self,
        location: &str,
        json_opts: &HashMap<String, String>,
    ) -> Result<(String, String), BundlebaseError> {
        use crate::source::fetch::download_to_data_dir;
        use bundlebase_common::source_utils::json_to_parquet_with_options;

        let file =
            crate::io::readable_file_from_path(location, self.data_dir(), self.config()).await?;
        let json_bytes = file
            .read_bytes()
            .await?
            .ok_or_else(|| BundlebaseError::from(format!("File not found: {}", location)))?;

        let record_path = json_opts
            .get("json_record_path")
            .map(|s| s.as_str())
            .unwrap_or("");
        let sep = json_opts.get("json_sep").map(|s| s.as_str()).unwrap_or("_");
        let meta_paths: Vec<&str> = json_opts
            .get("json_meta")
            .map(|s| {
                s.split(',')
                    .filter(|p| !p.is_empty())
                    .map(|p| p.trim())
                    .collect()
            })
            .unwrap_or_default();

        let parquet_bytes =
            json_to_parquet_with_options(&json_bytes, record_path, sep, &meta_paths)?;

        let data_dir = self.data_dir();
        let address = bundlebase_common::ContentAddress::with_sub_type(
            bundlebase_common::ContentCategory::Block,
            "data",
            bundlebase_common::ContentFormat::Parquet,
        )?;
        let write_result = download_to_data_dir(parquet_bytes, &address, data_dir.as_ref()).await?;
        let relative_path = data_dir.relative_path(write_result.file.as_ref())?;

        Ok((relative_path, write_result.hash))
    }

    /// Commits all operations in the bundle to persistent storage.
    ///
    /// # Arguments
    /// * `message` - Human-readable description of the changes (e.g., "Filter to Q4 data")
    ///
    /// # Example
    /// ```ignore
    /// builder.attach("data.parquet", None).await?;
    /// builder.filter("amount > 100", vec![]).await?;
    /// builder.commit("Filter high-value transactions").await?;
    /// ```
    pub async fn commit(&self, message: &str) -> Result<&Self, BundlebaseError> {
        // Validate no pending filters reference temporary-only functions
        let mut changes = self.status.read().changes().clone();
        for change in &changes {
            for op in &change.operations {
                if let AnyOperation::Filter(filter_op) = op {
                    self.check_no_temp_functions_in_sql(&filter_op.query, "commit")?;
                }
            }
        }

        let manifest_dir = self.bundle.data_dir().writable_subdir(META_DIR)?;
        let last_manifest_version = *self.bundle.last_manifest_version.read();
        let from = self.bundle.from();
        let passed_config = Some((*self.bundle.config.passed_config()).clone());
        let url = self.bundle.url().to_string();
        let bundle_id = self.bundle.id();

        if last_manifest_version == 0 {
            let init_file = manifest_dir.writable_file(INIT_FILENAME)?;
            // Use the bundle's existing ID rather than generating a new one
            let version_str = bundlebase_common::format_version_string();
            let init_commit = InitCommit {
                id: if from.is_none() {
                    Some(bundle_id)
                } else {
                    None
                },
                from: from.clone(),
                view: None,
                min_version: Some(version_str.clone()),
                max_version: Some(version_str),
            };
            write_yaml(init_file.as_ref(), &init_commit).await?;
        };

        // Calculate next version number
        let next_version = last_manifest_version + 1;

        // Get current timestamp in UTC ISO format
        let now = std::time::SystemTime::now();
        let timestamp = to_iso(now);

        // Get author from environment or use default
        let author = std::env::var("BUNDLEBASE_AUTHOR")
            .unwrap_or_else(|_| std::env::var("USER").unwrap_or_else(|_| "unknown".to_string()));

        // Write tombstone file if there are pending deletes
        let deleted_ids = std::mem::take(&mut *self.pending_deletes.write());
        let delete_wheres = std::mem::take(&mut *self.pending_delete_wheres.write());
        if !deleted_ids.is_empty() {
            use crate::bundle::operation::DeleteOp;
            use crate::bundle::tombstone;

            // Serialize and write tombstone file via content-addressed storage
            let tomb_bytes = tombstone::serialize_rowids(&deleted_ids);
            debug!(
                "[DELETE] Writing tombstone file ({} bytes)",
                tomb_bytes.len()
            );
            let data_dir = self.bundle.data_dir();
            let stream = futures::stream::iter(vec![Ok::<_, std::io::Error>(tomb_bytes)]);
            let address = bundlebase_common::ContentAddress::with_sub_type(
                bundlebase_common::ContentCategory::Overlay,
                "tomb",
                bundlebase_common::ContentFormat::Rowids,
            )?;
            let write_result = data_dir.write_stream(Box::pin(stream), &address).await?;
            let tomb_filename = data_dir.relative_path(write_result.file.as_ref())?;
            debug!("[DELETE] Tombstone file written: {}", tomb_filename);

            // Add DeleteOp alongside the existing FilterOp.
            // The FilterOp handles query-time exclusion, the DeleteOp records the tombstone.
            // On future opens, the tombstone will be loaded for scan-level filtering.
            let delete_op = DeleteOp::new(&tomb_filename, delete_wheres.join("; "));
            if let Some(last_change) = changes.last_mut() {
                last_change.operations.push(AnyOperation::Delete(delete_op));
            }
        }

        // Write update overlay file if there are pending updates from UPDATE commands
        let pending_upd = std::mem::take(&mut *self.pending_updates.write());
        let update_wheres = std::mem::take(&mut *self.pending_update_wheres.write());
        if !pending_upd.is_empty() {
            use crate::bundle::operation::UpdateDataOp;
            use crate::bundle::update_overlay;

            // Collect column types from the bundle schema
            let schema = self.bundle.schema().await?;
            let col_names = bundle_schema::BundleSchema::resolved(&self.operations());
            let mut column_types: std::collections::HashMap<
                crate::object_id::ColumnId,
                arrow::datatypes::DataType,
            > = std::collections::HashMap::new();
            for (col_id, real_name) in col_names.columns() {
                if let Some((_, field)) = schema.column_with_name(real_name) {
                    column_types.insert(*col_id, field.data_type().clone());
                }
            }

            let overlay_bytes = update_overlay::write_overlay_parquet(&pending_upd, &column_types)?;
            debug!(
                "[UPDATE] Writing overlay file ({} bytes, {} rows)",
                overlay_bytes.len(),
                pending_upd.len()
            );
            let data_dir = self.bundle.data_dir();
            let stream = futures::stream::iter(vec![Ok::<_, std::io::Error>(overlay_bytes)]);
            let address = bundlebase_common::ContentAddress::with_sub_type(
                bundlebase_common::ContentCategory::Overlay,
                "update",
                bundlebase_common::ContentFormat::Parquet,
            )?;
            let write_result = data_dir.write_stream(Box::pin(stream), &address).await?;
            let overlay_filename = data_dir.relative_path(write_result.file.as_ref())?;
            debug!("[UPDATE] Overlay file written: {}", overlay_filename);

            let update_op = UpdateDataOp::new(&overlay_filename, update_wheres.join("; "));
            if let Some(last_change) = changes.last_mut() {
                last_change
                    .operations
                    .push(AnyOperation::UpdateData(update_op));
            }
        }

        let commit_struct = commit::BundleCommit {
            url: None, //no need to set, we're just writing it and then will re-read it back
            data_dir: None,
            message: message.to_string(),
            author,
            timestamp,
            changes,
        };

        // Serialize directly using serde_yaml_ng
        let yaml = serde_yaml_ng::to_string(&commit_struct)?;

        // Calculate SHA256 hash of the YAML content
        let mut hasher = Sha256::new();
        hasher.update(yaml.as_bytes());
        let hash_bytes = hasher.finalize();
        let hash_hex = hex::encode(hash_bytes);
        let hash_short = &hash_hex[..12];

        // Create versioned filename: {5-digit-version}{12-char-hash}.yaml
        let filename = format!("{:05}{}.yaml", next_version, hash_short);
        let manifest_file = manifest_dir.writable_file(filename.as_str())?;

        // Write as stream
        let data = bytes::Bytes::from(yaml);
        let stream = futures::stream::iter(vec![Ok::<_, std::io::Error>(data)]);
        manifest_file.write_stream(Box::pin(stream)).await?;

        // Update metadata to reflect the committed version.
        // The bundle state (operations, version, schema) is already correct in memory —
        // we just need to record the commit, advance the manifest version, and apply
        // any commit-time operations (DeleteOp, UpdateDataOp) that were added to the
        // manifest but not yet applied to the in-memory bundle.
        *self.bundle.last_manifest_version.write() = next_version;

        // Apply commit-time operations that were created during commit serialization
        // (tombstone DeleteOps and overlay UpdateDataOps)
        for change in &commit_struct.changes {
            for op in &change.operations {
                let is_commit_time_op =
                    matches!(op, AnyOperation::Delete(_) | AnyOperation::UpdateData(_));
                if is_commit_time_op {
                    self.bundle.apply_operation(op.clone()).await?;
                }
            }
        }

        // Set data_dir on the commit to match what open_recursive would have set,
        // so that from() derivation works correctly for extended bundles.
        let mut commit_with_metadata = commit_struct;
        commit_with_metadata.data_dir = Some(self.bundle.data_dir().url().clone());
        self.bundle.commits.write().push(commit_with_metadata);

        // Clear status since the operations have been persisted
        self.status.write().clear();

        info!("Committed version {}", self.bundle.version());

        Ok(self)
    }

    /// Resets all uncommitted operations, reverting to the last committed state.
    ///
    /// This method clears all pending operations and reloads the bundle from
    /// the last committed version. Any changes made since the last commit are discarded.
    ///
    /// # Example
    /// ```ignore
    /// builder.attach("data.parquet", None).await?;
    /// builder.filter("amount > 100", vec![]).await?;
    /// builder.reset().await?;  // Discards attach and filter operations
    /// ```
    pub async fn reset(&self) -> Result<&Self, BundlebaseError> {
        if self.status().is_empty() {
            return Err("No uncommitted changes".into());
        }

        // Clear all uncommitted changes
        self.status.write().clear();
        self.pending_deletes.write().clear();
        self.pending_delete_wheres.write().clear();
        self.pending_updates.write().clear();
        self.pending_update_wheres.write().clear();

        // Reload the bundle from the last committed state
        self.reload_bundle().await?;

        info!("All uncommitted changes discarded");

        Ok(self)
    }

    /// Undoes the last uncommitted change, reverting one logical unit of work at a time.
    ///
    /// This method removes the most recent change from the uncommitted changes list
    /// and reloads the bundle to reflect the state before that change was applied.
    /// Use this for incremental undo functionality.
    ///
    /// # Example
    /// ```ignore
    /// builder.attach("data.parquet", None).await?;
    /// builder.filter("amount > 100", vec![]).await?;
    /// builder.undo().await?; // Discards only the filter change
    /// // Bundle now has only the attach change pending
    /// ```
    pub async fn undo(&self) -> Result<String, BundlebaseError> {
        if self.status().is_empty() {
            return Err("No uncommitted changes to undo".into());
        }

        // Remove the last change and capture its description
        let undone = self
            .status
            .write()
            .pop_change()
            .expect("status was non-empty");
        let description = undone.description.clone();

        // Reload the bundle from the last committed state
        self.reload_bundle().await?;

        // Reapply all remaining operations
        let changes = self.status.read().changes().clone();
        for change in &changes {
            for op in &change.operations {
                self.bundle.apply_operation(op.clone()).await?;
            }
        }

        info!("UNDONE: {}", description);

        Ok(description)
    }

    /// Check that a SQL string does not reference any temporary-only functions.
    ///
    /// Returns an error naming the temporary function if one is found,
    /// or Ok(()) if the SQL is safe to persist.
    pub fn check_no_temp_functions_in_sql(
        &self,
        sql: &str,
        context: &str,
    ) -> Result<(), BundlebaseError> {
        let temp_names = self
            .bundle
            .function_registry()
            .read()
            .temporary_only_names();
        if temp_names.is_empty() {
            return Ok(());
        }

        let conflicts = sql::find_temp_functions_in_sql(sql, &temp_names);

        if let Some(name) = conflicts.first() {
            if context == "commit" {
                return Err(format!(
                    "Cannot commit: filter query references temporary function '{}'. \
                     Temporary functions are session-only and will not be available after \
                     the bundle is reopened. Either import '{}' as a persistent function \
                     (IMPORT FUNCTION) or remove the filter before committing.",
                    name, name
                )
                .into());
            } else {
                return Err(format!(
                    "Cannot create view: SQL references temporary function '{}'. \
                     Views are persisted and must not depend on temporary functions. \
                     Import '{}' as a persistent function (IMPORT FUNCTION) first.",
                    name, name
                )
                .into());
            }
        }

        Ok(())
    }

    pub(in crate::bundle) async fn reload_bundle(&self) -> Result<(), BundlebaseError> {
        // Reload the bundle from the last committed state
        let empty = self.bundle.commits.read().is_empty();
        let passed_config = (*self.bundle.config.passed_config()).clone();
        let url = self.bundle.url().to_string();

        // Note: reload_from preserves the original ctx and its schema providers
        // which already have the correct facade set
        let new_bundle: Bundle = if empty {
            // No commits yet: restore the post-create state.
            // Bundle::empty() generates a fresh UUID and no packs, so we must:
            //   1. Preserve the original bundle ID (never committed, but consistent within session)
            //   2. Restore the BASE_PACK that BundleBuilder::create() always sets up
            let original_id = self.bundle.id();
            let arc = Bundle::empty(Some(passed_config)).await?;
            let bundle = (*arc).clone();
            bundle.refresh_data_dir().await?;
            *bundle.data_dir.write() =
                writable_dir_from_url(&Url::parse(&url)?, bundle.config()).await?;
            *bundle.id.write() = original_id;
            bundle.add_pack(ObjectId::BASE_PACK, Arc::new(Pack::new_base()));
            bundle
        } else {
            // Preserve explicit_config when reopening
            // open returns Arc<Bundle>, so we clone the inner Bundle
            let arc_bundle = Bundle::open(&url, Some(passed_config)).await?;
            (*arc_bundle).clone()
        };

        // Update bundle contents using reload_from to preserve Arc references
        self.bundle.reload_from(new_bundle);
        Ok(())
    }

    pub async fn apply_operation(&self, op: AnyOperation) -> Result<(), BundlebaseError> {
        if self.bundle.is_view() && !op.allowed_on_view() {
            return Err(format!("Operation '{}' is not allowed on a view", op.describe()).into());
        }

        self.bundle.apply_operation(op.clone()).await?;

        self.in_progress_change
            .write()
            .as_mut()
            .expect("apply_operation called without an in-progress change")
            .operations
            .push(op);

        Ok(())
    }

    /// Evaluate an UPDATE statement: find matching rows, evaluate SET expressions,
    /// and store results in pending_updates. Returns count of updated rows.
    /// Evaluate an UPDATE: given column names, expressions, and a WHERE clause,
    /// find matching rows, evaluate expressions, and store in pending_updates.
    ///
    /// `columns` and `expressions` are parallel vectors: columns[i] gets value expressions[i].
    pub async fn evaluate_update_cols(
        &self,
        columns: &[String],
        expressions: &[String],
        where_clause: &str,
    ) -> Result<usize, BundlebaseError> {
        use futures::StreamExt;

        // Resolve column names to ColumnIds.
        // columns/expressions/where_clause use internal names.
        let col_ids: Vec<(String, crate::object_id::ColumnId)> = columns
            .iter()
            .map(|internal_name| {
                let col_id =
                    bundle_schema::parse_internal_name(internal_name).ok_or_else(|| {
                        BundlebaseError::from(format!(
                            "Column '{}' is not a valid internal name",
                            internal_name
                        ))
                    })?;
                Ok((internal_name.clone(), col_id))
            })
            .collect::<Result<Vec<_>, BundlebaseError>>()?;

        // Build SELECT clause for expression evaluation (already in internal name terms)
        let select_exprs: Vec<String> = columns
            .iter()
            .zip(expressions.iter())
            .map(|(col, expr)| format!("{} AS {}", expr, col))
            .collect();
        let select_list = select_exprs.join(", ");

        let mut updated_count = 0usize;

        let base_pack = self
            .bundle
            .packs()
            .read()
            .get(&ObjectId::BASE_PACK)
            .cloned();

        if let Some(pack) = base_pack {
            let blocks = pack.blocks();
            for (idx, block) in blocks.iter().enumerate() {
                let block_ref = ObjectIdAlias::from(idx as u16);
                let mut stream = block
                    .reader()
                    .extract_rowids_stream(block_ref, self.bundle.ctx(), None)
                    .await?;

                while let Some(batch_result) = stream.next().await {
                    let rowid_batch = batch_result?;
                    let batch = &rowid_batch.batch;
                    let row_ids = &rowid_batch.row_ids;

                    // Rename columns from physical names to internal names
                    let batch = {
                        let schema = batch.schema();
                        let col_id_list = block.column_ids();
                        let new_fields: Vec<arrow::datatypes::Field> = schema
                            .fields()
                            .iter()
                            .zip(col_id_list.iter())
                            .map(|(f, col_id)| {
                                f.as_ref()
                                    .clone()
                                    .with_name(bundle_schema::generate_internal_name(col_id))
                            })
                            .collect();
                        let new_schema = Arc::new(arrow::datatypes::Schema::new_with_metadata(
                            new_fields,
                            schema.metadata().clone(),
                        ));
                        arrow::record_batch::RecordBatch::try_new(
                            new_schema,
                            batch.columns().to_vec(),
                        )?
                    };

                    // Evaluate: SELECT _rowid, <set_exprs> FROM (data with _rowid) WHERE <condition>
                    let eval_sql = format!(
                        "SELECT CAST(_idx AS BIGINT) AS _idx, {} FROM (SELECT *, ROW_NUMBER() OVER () - 1 AS _idx FROM __update_batch) WHERE {}",
                        select_list,
                        where_clause
                    );

                    let mut config = datafusion::prelude::SessionConfig::new();
                    config.options_mut().sql_parser.enable_ident_normalization = false;
                    let temp_ctx =
                        SessionContext::new_with_config_rt(config, self.bundle.ctx().runtime_env());
                    let mem_table = datafusion::datasource::MemTable::try_new(
                        batch.schema(),
                        vec![vec![batch.clone()]],
                    )?;
                    temp_ctx.register_table("__update_batch", Arc::new(mem_table))?;

                    let result_df = temp_ctx
                        .sql(&eval_sql)
                        .await
                        .map_err(|e| BundlebaseError::from(e.to_string()))?;
                    let result_batches = result_df
                        .collect()
                        .await
                        .map_err(|e| BundlebaseError::from(e.to_string()))?;

                    let mut pending = self.pending_updates.write();
                    for result_batch in &result_batches {
                        let idx_col = result_batch
                            .column(0)
                            .as_any()
                            .downcast_ref::<arrow::array::Int64Array>()
                            .ok_or_else(|| BundlebaseError::from("Expected Int64 _idx column"))?;

                        for row in 0..result_batch.num_rows() {
                            let batch_idx = idx_col.value(row) as usize;
                            if batch_idx >= row_ids.len() {
                                continue;
                            }
                            let row_id = row_ids[batch_idx];

                            let cell_updates = pending
                                .entry(row_id)
                                .or_insert_with(std::collections::HashMap::new);
                            for (col_idx, (_, col_id)) in col_ids.iter().enumerate() {
                                // Column is at position col_idx + 1 (after _idx)
                                let value = datafusion::scalar::ScalarValue::try_from_array(
                                    result_batch.column(col_idx + 1),
                                    row,
                                )
                                .map_err(|e| BundlebaseError::from(e.to_string()))?;
                                cell_updates.insert(*col_id, value);
                            }
                            updated_count += 1;
                        }
                    }
                }
            }
        }

        if updated_count > 0 {
            self.pending_update_wheres
                .write()
                .push(where_clause.to_string());
        }

        Ok(updated_count)
    }

    /// Push pending updates to DataBlocks for immediate in-session visibility.
    pub fn flush_pending_updates_to_blocks(&self) {
        let pending = self.pending_updates.read();
        if pending.is_empty() {
            log::debug!("[UPDATE] No pending updates to flush");
            return;
        }
        log::debug!(
            "[UPDATE] Flushing {} pending updates to blocks",
            pending.len()
        );

        // Group by block_ref
        let mut by_block: std::collections::HashMap<
            u16,
            std::collections::HashMap<
                bundlebase_common::RowId,
                std::collections::HashMap<
                    crate::object_id::ColumnId,
                    datafusion::scalar::ScalarValue,
                >,
            >,
        > = std::collections::HashMap::new();
        for (row_id, cell_updates) in pending.iter() {
            let block_idx = row_id.block_ref().as_u16();
            by_block
                .entry(block_idx)
                .or_default()
                .insert(*row_id, cell_updates.clone());
        }

        let packs = self.bundle.packs().read().clone();
        if let Some(pack) = packs.get(&ObjectId::BASE_PACK) {
            let blocks = pack.blocks();
            for (block_idx, block_updates) in by_block {
                if let Some(block) = blocks.get(block_idx as usize) {
                    let overlay =
                        crate::bundle::update_overlay::UpdateOverlay::from_pending(&block_updates);
                    block.add_update_overlay(overlay);
                }
            }
        }

        // Clear cached dataframe so next query picks up the overlay
        self.bundle.dataframe.clear();
    }

    /// Returns the current always-delete rules.
    pub fn always_delete_rules(&self) -> Vec<String> {
        self.bundle.always_delete_rules()
    }

    /// Returns the current always-update rules.
    pub fn always_update_rules(&self) -> Vec<AlwaysUpdateRule> {
        self.bundle.always_update_rules()
    }

    /// Add RowIds to the pending delete set.
    ///
    /// These will be written to a tombstone file on commit.
    pub fn mark_deleted(
        &self,
        row_ids: std::collections::HashSet<bundlebase_common::RowId>,
        where_clause: &str,
    ) {
        self.pending_deletes.write().extend(row_ids);
        self.pending_delete_wheres
            .write()
            .push(where_clause.to_string());
    }

    /// Collect RowIds of rows matching a WHERE clause from all blocks.
    ///
    /// Streams through each block with RowIds, evaluates the WHERE condition,
    /// and returns the set of matching RowIds.
    pub async fn select_row_ids(
        &self,
        where_clause: &str,
    ) -> Result<std::collections::HashSet<bundlebase_common::RowId>, BundlebaseError> {
        use futures::StreamExt;

        let mut matching_ids = std::collections::HashSet::new();
        let base_pack = self
            .bundle
            .packs()
            .read()
            .get(&ObjectId::BASE_PACK)
            .cloned();

        if let Some(pack) = base_pack {
            let blocks = pack.blocks();
            for (idx, block) in blocks.iter().enumerate() {
                let block_ref = ObjectIdAlias::from(idx as u16);
                let mut stream = block
                    .reader()
                    .extract_rowids_stream(block_ref, self.bundle.ctx(), None)
                    .await?;

                while let Some(batch_result) = stream.next().await {
                    let rowid_batch = batch_result?;
                    let batch = &rowid_batch.batch;
                    let row_ids = &rowid_batch.row_ids;

                    // Rename columns from physical names to internal names
                    let batch = {
                        let schema = batch.schema();
                        let col_ids = block.column_ids();
                        let new_fields: Vec<arrow::datatypes::Field> = schema
                            .fields()
                            .iter()
                            .zip(col_ids.iter())
                            .map(|(f, col_id)| {
                                f.as_ref()
                                    .clone()
                                    .with_name(bundle_schema::generate_internal_name(col_id))
                            })
                            .collect();
                        let new_schema = Arc::new(arrow::datatypes::Schema::new_with_metadata(
                            new_fields,
                            schema.metadata().clone(),
                        ));
                        arrow::record_batch::RecordBatch::try_new(
                            new_schema,
                            batch.columns().to_vec(),
                        )?
                    };

                    let filter_sql = format!(
                        "SELECT CAST(_idx AS BIGINT) AS _idx FROM (SELECT *, ROW_NUMBER() OVER () - 1 AS _idx FROM __delete_batch) WHERE {}",
                        where_clause
                    );

                    let mut config = datafusion::prelude::SessionConfig::new();
                    config.options_mut().sql_parser.enable_ident_normalization = false;
                    let temp_ctx =
                        SessionContext::new_with_config_rt(config, self.bundle.ctx().runtime_env());
                    let mem_table = datafusion::datasource::MemTable::try_new(
                        batch.schema(),
                        vec![vec![batch.clone()]],
                    )?;
                    temp_ctx.register_table("__delete_batch", Arc::new(mem_table))?;

                    let idx_df = temp_ctx.sql(&filter_sql).await.map_err(|e| {
                        BundlebaseError::from(format!(
                            "Failed to evaluate WHERE clause '{}' on block {}: {}",
                            where_clause, idx, e
                        ))
                    })?;
                    let idx_batches = idx_df.collect().await.map_err(|e| {
                        BundlebaseError::from(format!(
                            "Failed to collect filtered rows for WHERE '{}' on block {}: {}",
                            where_clause, idx, e
                        ))
                    })?;

                    for idx_batch in &idx_batches {
                        let idx_col = idx_batch.column(0);
                        for i in 0..idx_batch.num_rows() {
                            use arrow::array::{Array, AsArray};
                            use arrow::datatypes::DataType;
                            let val = match idx_col.data_type() {
                                DataType::UInt64 => idx_col
                                    .as_primitive::<arrow::datatypes::UInt64Type>()
                                    .value(i)
                                    as usize,
                                DataType::Int64 => idx_col
                                    .as_primitive::<arrow::datatypes::Int64Type>()
                                    .value(i)
                                    as usize,
                                dt => {
                                    return Err(format!(
                                        "Unexpected column type {:?} from ROW_NUMBER()",
                                        dt
                                    )
                                    .into())
                                }
                            };
                            if val < row_ids.len() {
                                matching_ids.insert(row_ids[val]);
                            }
                        }
                    }
                }
            }
        }

        Ok(matching_ids)
    }

    /// Execute a closure within a change context, managing the change lifecycle automatically.
    ///
    /// This method creates a new change, executes the provided closure, and adds the change
    /// to the status on success. If a change is already in progress, it logs a debug message
    /// and executes the closure without creating a nested change.
    ///
    /// # Arguments
    /// * `description` - Human-readable description of the change
    /// * `f` - Closure that performs operations within the change context
    ///
    /// # Errors
    /// Returns any error from the closure. On error, the in-progress change is discarded.
    pub async fn do_change<F>(&self, description: &str, f: F) -> Result<(), BundlebaseError>
    where
        F: for<'a> FnOnce(&'a Self) -> BoxFuture<'a, Result<(), BundlebaseError>>,
    {
        // Check for nested changes - track whether we created this change
        let is_nested = {
            let in_progress = self.in_progress_change.read();
            match &*in_progress {
                Some(in_progress_change) => {
                    debug!(
                        "Change {} already in progress, not going to separately track {}",
                        in_progress_change.description, description
                    );
                    true
                }
                None => false,
            }
        };

        if !is_nested {
            let change = BundleChange::new(description);
            *self.in_progress_change.write() = Some(change);
        }

        // Execute the closure
        let result = f(self).await;

        // Only finalize the change if we created it (not nested)
        match result {
            Ok(_) => {
                if !is_nested {
                    if let Err(e) = self.auto_reindex_if_attach_or_replace().await {
                        self.in_progress_change.write().take();
                        return Err(e);
                    }
                    self.prune_stale_index_entries_if_block_versions_changed();
                    if let Some(change) = self.in_progress_change.write().take() {
                        self.status.write().push_change(change);
                        // Re-register version UDF to reflect builder state (e.g., "UNCOMMITTED")
                        self.bundle
                            .function_registry()
                            .read()
                            .refresh_version_udf(self.version());
                    }
                }
                Ok(())
            }
            Err(e) => {
                if !is_nested {
                    *self.in_progress_change.write() = None;
                }
                Err(e)
            }
        }
    }

    /// Execute an async operation within a change-tracking context.
    ///
    /// This wraps any async operation with change tracking: creating a change record
    /// before execution, and finalizing or discarding it based on the result.
    ///
    /// # Arguments
    /// * `description` - Human-readable description of the change
    /// * `future` - The future to execute within the change context
    ///
    /// # Returns
    /// * `Ok(T)` - Operation result on success
    /// * `Err(BundlebaseError)` - Operation failed
    pub async fn run_command<T>(
        &self,
        description: String,
        future: impl std::future::Future<Output = Result<T, BundlebaseError>>,
    ) -> Result<T, BundlebaseError> {
        use crate::bundle::operation::BundleChange;

        // Check for nested changes
        let is_nested = {
            let in_progress = self.in_progress_change.read();
            match &*in_progress {
                Some(in_progress_change) => {
                    debug!(
                        "Change {} already in progress, not going to separately track {}",
                        in_progress_change.description, description
                    );
                    true
                }
                None => false,
            }
        };

        if !is_nested {
            let change = BundleChange::new(&description);
            *self.in_progress_change.write() = Some(change);
        }

        // Execute the command
        debug!("Executing command: {}", description);
        let result = future.await;

        // Only finalize the change if we created it (not nested)
        match &result {
            Ok(_) => {
                debug!("Command succeeded: {}", description);
                if !is_nested {
                    if let Err(e) = self.auto_reindex_if_attach_or_replace().await {
                        self.in_progress_change.write().take();
                        return Err(e);
                    }
                    self.prune_stale_index_entries_if_block_versions_changed();
                    if let Some(change) = self.in_progress_change.write().take() {
                        self.status.write().push_change(change);
                        // Re-register version UDF to reflect builder state (e.g., "UNCOMMITTED")
                        self.bundle
                            .function_registry()
                            .read()
                            .refresh_version_udf(self.version());
                    }
                }
            }
            Err(e) => {
                debug!("Command failed: {}: {}", description, e);
                if !is_nested {
                    // On failure, discard the in-progress change
                    self.in_progress_change.write().take();
                }
            }
        }

        result
    }

    /// Drop runtime-only connector (session-only, no operation created).
    pub async fn drop_temp_connector(
        &self,
        name: &str,
        platform: Option<&str>,
    ) -> Result<usize, BundlebaseError> {
        use crate::platform::Platform;
        let platform: Option<Platform> = platform.map(|s| s.parse()).transpose()?;
        Ok(self
            .bundle()
            .connector_registry()
            .write()
            .remove_entry(name, platform.as_ref(), true))
    }

    /// Drop runtime-only function (session-only, no operation created).
    pub async fn drop_temp_function(
        &self,
        name: &str,
        platform: Option<&str>,
    ) -> Result<usize, BundlebaseError> {
        use crate::platform::Platform;
        let platform: Option<Platform> = platform.map(|s| s.parse()).transpose()?;
        self.bundle()
            .function_registry()
            .write()
            .drop_temp(name, platform.as_ref())
    }

    /// If the still-open in-progress change contains any AttachBlock or
    /// ReplaceBlock op AND the bundle has at least one index defined, run
    /// `reindex_internal` so the new IndexBlocksOp(s) ride into the same
    /// change. Called once per user-facing command at the outermost
    /// finalize point of `do_change` / `run_command` — never mid-command.
    async fn auto_reindex_if_attach_or_replace(&self) -> Result<(), BundlebaseError> {
        let needs = self
            .in_progress_change
            .read()
            .as_ref()
            .is_some_and(|c| {
                !c.suppress_auto_reindex
                    && c.operations.iter().any(|op| {
                        matches!(
                            op,
                            AnyOperation::AttachBlock(_) | AnyOperation::ReplaceBlock(_)
                        )
                    })
            });
        if !needs {
            return Ok(());
        }
        if self.bundle.indexes.read().is_empty() {
            return Ok(());
        }
        self.reindex_internal().await
    }

    /// Mark the in-progress change as opting out of the auto-reindex hook.
    /// Used by `ATTACH … NO INDEX` / `FETCH … NO INDEX` so the user can
    /// defer indexing until they explicitly run `REINDEX`. Silently no-ops
    /// when called outside a change.
    pub fn suppress_auto_reindex_for_current_change(&self) {
        if let Some(change) = self.in_progress_change.write().as_mut() {
            change.suppress_auto_reindex = true;
        }
    }

    /// Drop `IndexedBlocks` entries from each `IndexDefinition` whose blocks
    /// are no longer at their current version. Called at the outermost
    /// finalize point of `do_change` / `run_command` when the just-completed
    /// change contains a block-version-changing op (AttachBlock,
    /// ReplaceBlock, DetachBlock). Without this, the runtime
    /// `Vec<Arc<IndexedBlocks>>` grows unboundedly with commit history.
    ///
    /// The on-disk index files are NOT deleted — older manifests still
    /// reference them, so opening the bundle pinned to a previous version
    /// continues to find the matching index data.
    fn prune_stale_index_entries_if_block_versions_changed(&self) {
        // Combine committed bundle ops + completed pending changes + the
        // still-open in-progress change. The latter is critical because
        // the just-applied AttachBlock/ReplaceBlock/DetachBlock lives
        // there and won't reach `status` until after this method returns.
        let mut all_ops: Vec<AnyOperation> = self.bundle.operations.read().clone();
        all_ops.extend(self.status().operations().into_iter());
        let in_progress = self.in_progress_change.read();
        let in_progress_ops: &[AnyOperation] = in_progress
            .as_ref()
            .map(|c| c.operations.as_slice())
            .unwrap_or(&[]);
        let touched = in_progress_ops.iter().any(|op| {
            matches!(
                op,
                AnyOperation::AttachBlock(_)
                    | AnyOperation::ReplaceBlock(_)
                    | AnyOperation::DetachBlock(_)
            )
        });
        if !touched {
            return;
        }
        all_ops.extend(in_progress_ops.iter().cloned());
        drop(in_progress);

        let indexes = self.bundle.indexes.read().clone();
        if indexes.is_empty() {
            return;
        }

        // Walk every block-affecting op once to derive the current
        // (BlockId → version) map. We can't use BundleSchema for this
        // because BundleSchema::resolved doesn't process DetachBlock —
        // it only tracks columns, not block lifecycle.
        let mut current_versions: HashMap<BlockId, String> = HashMap::new();
        for op in &all_ops {
            match op {
                AnyOperation::AttachBlock(attach) => {
                    current_versions.insert(attach.id, attach.version.clone());
                }
                AnyOperation::ReplaceBlock(replace) => {
                    current_versions.insert(replace.id, replace.new_version.clone());
                }
                AnyOperation::DetachBlock(detach) => {
                    current_versions.remove(&detach.id);
                }
                _ => {}
            }
        }

        for index_def in &indexes {
            index_def.prune_stale_blocks(&current_versions);
        }
    }

    /// Internal reindex implementation that doesn't wrap in do_change.
    ///
    /// This is used by commands that need to reindex within their own change context.
    pub async fn reindex_internal(&self) -> Result<(), BundlebaseError> {
        use crate::object_id::ColumnId;

        // Group blocks by (index_id, column_ids) for batching
        let mut blocks_to_index: HashMap<(ObjectId, Vec<ColumnId>), Vec<(BlockId, String)>> =
            HashMap::new();

        // Collect index definitions before the loop to avoid holding the lock across awaits
        let index_defs: Vec<Arc<IndexDefinition>> =
            self.bundle.indexes.read().iter().cloned().collect();

        // Use bundle's operations directly (not self.operations() which includes
        // pending changes and would duplicate operations already applied to bundle)
        let operations = self.bundle.operations.read().clone();

        for index_def in &index_defs {
            let index_id = index_def.id();
            let index_column_ids: Vec<ColumnId> = index_def.column_ids().to_vec();

            // Use the first column ID for "needs reindex" checks
            let first_col_id = index_column_ids.first().ok_or_else(|| {
                BundlebaseError::from(format!("Index '{}' has no columns defined", index_id))
            })?;

            debug!("Checking index on column IDs {:?}", &index_column_ids);

            // Use blocks_for_column to find which blocks need indexing
            let candidate_blocks = self.bundle_schema().blocks_for_column(first_col_id);

            for (block_id, block_version) in candidate_blocks {
                let versioned_block = VersionedBlockId::new(block_id, block_version.clone());

                // Check if index already exists at this version
                let needs_index = self
                    .bundle()
                    .get_index(first_col_id, &versioned_block)
                    .is_none();

                if needs_index {
                    blocks_to_index
                        .entry((*index_id, index_column_ids.clone()))
                        .or_default()
                        .push((block_id, block_version));
                }
            }
        }

        // Create IndexBlocksOp for each group of blocks
        for ((index_id, column_ids), blocks) in blocks_to_index {
            if !blocks.is_empty() {
                debug!(
                    "Creating IndexBlocksOp for column IDs {:?} with {} blocks",
                    column_ids,
                    blocks.len()
                );

                let op = IndexBlocksOp::setup(&index_id, column_ids, blocks, self).await?;
                self.apply_operation(op.into()).await?;
            }
        }

        info!("Reindexed all columns");

        Ok(())
    }

    /// Resolve a pack name to its ObjectId.
    ///
    /// This is a helper method used by commands that operate on packs.
    ///
    /// # Arguments
    /// * `pack` - The pack name: `None` or `"base"` for the base pack,
    ///            otherwise a join name.
    ///
    /// # Returns
    /// * `Ok(ObjectId)` - The resolved pack ID
    /// * `Err(BundlebaseError)` - If the join name doesn't exist
    pub fn resolve_pack_id(&self, pack: Option<&str>) -> Result<ObjectId, BundlebaseError> {
        match pack {
            None | Some("base") => Ok(ObjectId::BASE_PACK),
            Some(join_name) => self
                .bundle()
                .pack_by_name(join_name)
                .map(|p| *p.id())
                .ok_or_else(|| format!("Unknown join '{}'", join_name).into()),
        }
    }
}

#[async_trait]
impl BundleFacade for BundleBuilder {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn id(&self) -> String {
        self.bundle.id()
    }

    fn name(&self) -> Option<String> {
        self.bundle.name()
    }

    fn description(&self) -> Option<String> {
        self.bundle.description()
    }

    fn url(&self) -> Url {
        self.bundle.url()
    }

    fn from(&self) -> Option<Url> {
        self.bundle.from()
    }

    fn version(&self) -> String {
        let has_changes = !self.status.read().is_empty();
        let has_temp = self.bundle.has_temporary_udf();

        match (has_changes, has_temp) {
            (true, true) => "UNCOMMITTED+TEMP".to_string(),
            (true, false) => "UNCOMMITTED".to_string(),
            (false, true) => "TEMP".to_string(),
            (false, false) => self.bundle.version.read().clone(),
        }
    }

    fn history(&self) -> Vec<commit::BundleCommit> {
        self.bundle.history()
    }

    fn operations(&self) -> Vec<AnyOperation> {
        // `BundleBuilder::apply_operation` writes to BOTH `bundle.operations`
        // (eagerly, so queries see the new state) AND the in-progress change
        // (so commit() can persist a structured changelog). After do_change /
        // run_command finalize, the in-progress change moves into `status`.
        // Status is therefore a *subset* of `bundle.operations` until commit()
        // clears it. Returning the union double-counts every uncommitted op,
        // which silently inflated BundleSchema's per-column block lists and
        // caused reindex_internal to feed each block to the index builder
        // twice (visible as 2× doc counts and 5% BM25 score drift before the
        // unified-search work surfaced it).
        self.bundle.operations.read().clone()
    }

    fn bundle_schema(&self) -> bundle_schema::BundleSchema {
        bundle_schema::BundleSchema::resolved(&self.operations())
    }

    async fn schema(&self) -> Result<SchemaRef, BundlebaseError> {
        self.bundle.schema().await
    }

    async fn num_rows(&self) -> Result<usize, BundlebaseError> {
        self.bundle.num_rows().await
    }

    async fn dataframe(&self) -> Result<Arc<DataFrame>, BundlebaseError> {
        self.bundle.dataframe().await
    }

    async fn extend(&self, data_dir: Option<&str>) -> Result<Arc<BundleBuilder>, BundlebaseError> {
        // Create a new builder based on the current bundle state without modifying self
        let current_bundle = Arc::new(self.bundle.deref().clone());
        BundleBuilder::extend(current_bundle, data_dir).await
    }

    async fn query(
        &self,
        sql: &str,
        params: Vec<ScalarValue>,
        hard_limit: Option<usize>,
    ) -> Result<SendableRecordBatchStream, BundlebaseError> {
        Ok(self.bundle().query(sql, params, hard_limit).await?)
    }

    fn views(&self) -> HashMap<ObjectId, String> {
        self.bundle.views()
    }

    async fn view(&self, identifier: &str) -> Result<Arc<Bundle>, BundlebaseError> {
        self.bundle.view(identifier).await
    }

    async fn export_tar(&self, tar_path: &str, gzip: bool) -> Result<String, BundlebaseError> {
        // Check for uncommitted changes
        if !self.status().is_empty() {
            return Err("Cannot export tar with uncommitted changes. Please commit first.".into());
        }

        self.bundle.export_tar(tar_path, gzip).await
    }

    fn status_changes(&self) -> Vec<BundleChange> {
        self.status.read().changes().clone()
    }

    fn status(&self) -> BundleStatus {
        self.status.read().clone()
    }

    fn indexes(&self) -> Vec<Arc<IndexDefinition>> {
        self.bundle.indexes.read().clone()
    }

    fn packs(&self) -> HashMap<ObjectId, Arc<Pack>> {
        self.bundle.packs.read().clone()
    }

    fn sources(&self) -> HashMap<ObjectId, Arc<crate::bundle::Source>> {
        self.bundle.sources.read().clone()
    }

    fn views_by_name(&self) -> HashMap<String, ObjectId> {
        self.bundle.views.read().clone()
    }

    fn reports(&self) -> HashMap<String, ReportEntry> {
        self.bundle.reports()
    }

    fn always_delete_rules(&self) -> Vec<String> {
        self.bundle.always_delete_rules()
    }

    fn always_update_rules(&self) -> Vec<AlwaysUpdateRule> {
        self.bundle.always_update_rules()
    }

    fn data_dir(&self) -> Arc<dyn IOReadWriteDir> {
        self.bundle.data_dir()
    }

    fn config(&self) -> Arc<BundleConfig> {
        self.bundle.config()
    }

    async fn drop_temp_connector(
        &self,
        name: &str,
        platform: Option<&crate::platform::Platform>,
    ) -> Result<usize, BundlebaseError> {
        Ok(self
            .bundle
            .connector_registry()
            .write()
            .remove_entry(name, platform, true))
    }

    async fn drop_temp_function(
        &self,
        name: &str,
        platform: Option<&crate::platform::Platform>,
    ) -> Result<usize, BundlebaseError> {
        self.bundle
            .function_registry()
            .write()
            .drop_temp(name, platform)
    }

    async fn rename_temp_connector(
        &self,
        old_name: &str,
        new_name: &str,
    ) -> Result<(), BundlebaseError> {
        self.bundle.rename_temp_connector(old_name, new_name).await
    }

    async fn rename_temp_function(
        &self,
        old_name: &str,
        new_name: &str,
    ) -> Result<(), BundlebaseError> {
        self.bundle.rename_temp_function(old_name, new_name).await
    }

    async fn set_config(
        &self,
        scope: &Scope,
        key: &str,
        value: &str,
    ) -> Result<(), BundlebaseError> {
        self.bundle.set_config(scope, key, value).await
    }

    fn connector_registry(&self) -> Arc<RwLock<ConnectorRegistry>> {
        self.bundle.connector_registry()
    }

    fn function_registry(&self) -> Arc<RwLock<FunctionRegistry>> {
        self.bundle.function_registry()
    }

    fn ctx(&self) -> Arc<SessionContext> {
        self.bundle.ctx()
    }
}

// NOTE: Builder convenience method tests (attach, filter, drop_column, etc.) are covered
// by integration tests in tests/ which can use BundleBuilderExt from bundlebase-command.
// Unit tests here only test core BundleBuilder functionality that doesn't require commands.
// Due to Rust's dev-dependency type identity limitation, bundlebase-command extension traits
// cannot be used in unit tests within the bundlebase crate itself.
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_empty_bundle() {
        let bundle = BundleBuilder::create("memory:///test_bundle", None)
            .await
            .unwrap();
        assert_eq!(0, bundle.history().len());
    }

    #[tokio::test]
    async fn test_schema_empty_bundle() {
        let bundle = BundleBuilder::create("memory:///test_bundle", None)
            .await
            .unwrap();
        let schema = bundle.bundle.schema().await.unwrap();
        assert_eq!(
            schema.fields().len(),
            1,
            "Empty bundle should have sentinel no_data field"
        );
        assert_eq!(schema.field(0).name(), "no_data");
    }

    #[tokio::test]
    async fn test_create_fails_if_bundle_exists() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let path = tmp_dir.path().to_str().unwrap();

        // Create and commit a bundle
        let bundle = BundleBuilder::create(path, None).await.unwrap();
        bundle.commit("Initial").await.unwrap();

        // Attempting to create at the same path should fail
        let result = BundleBuilder::create(path, None).await;
        assert!(result.is_err());
        let err_msg = match result {
            Err(e) => e.to_string(),
            Ok(_) => panic!("Expected error"),
        };
        assert!(
            err_msg.contains("already exists"),
            "Error should mention bundle already exists: {}",
            err_msg
        );
    }
}
