use bundlebase_common::command_response::{CommandResponse, single_batch_stream, OutputShape};
use bundlebase_common::impl_dyn_command_response;
use crate::bundle::facade::BundleFacade;
use crate::bundle::init::InitCommit;
use crate::bundle::operation::AnyOperation;
use crate::bundle::operation::{BundleChange, IndexBlocksOp, Operation};
use crate::bundle::{commit, Pack, INIT_FILENAME, META_DIR};
use crate::bundle::function_entry::FunctionRegistry;
use crate::bundle::{column_metadata, sql, Bundle};
use crate::source::ConnectorRegistry;
use crate::data::{BlockId, ObjectId, ObjectIdAlias, VersionedBlockId};
use crate::index::{IndexDefinition};
use crate::io::{writable_dir_from_str, writable_dir_from_url, write_yaml, IOReadWriteDir};
use crate::bundle_config::{PassedBundleConfig, Scope};
use crate::BundleConfig;
use crate::BundlebaseError;
use arrow::array::{Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use chrono::DateTime;
use datafusion::execution::SendableRecordBatchStream;
use datafusion::prelude::{DataFrame, SessionContext};
use datafusion::scalar::ScalarValue;
use parking_lot::RwLock;
use sha2::{Digest, Sha256};
use tracing::{debug, info};
use std::collections::HashMap;
use std::future::Future;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::{Arc, Weak};
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
        let operation_counts: Vec<i32> = changes
            .iter()
            .map(|c| c.operations.len() as i32)
            .collect();

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
    pending_tombstones: RwLock<std::collections::HashSet<bundlebase_common::RowId>>,
    /// Updated cell values accumulated by UPDATE commands, written to an overlay parquet on commit.
    /// Maps RowId → (ColumnId → ScalarValue).
    pending_updates: RwLock<std::collections::HashMap<bundlebase_common::RowId, std::collections::HashMap<crate::object_id::ColumnId, datafusion::scalar::ScalarValue>>>,
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
            pending_tombstones: RwLock::new(self.pending_tombstones.read().clone()),
            pending_updates: RwLock::new(self.pending_updates.read().clone()),
        }
    }
}

/// Type alias for boxed futures used in do_change closures
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

impl BundleBuilder {
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
            pending_tombstones: RwLock::new(std::collections::HashSet::new()),
            pending_updates: RwLock::new(std::collections::HashMap::new()),
        });

        // Re-register schema providers with BundleBuilder as facade (using Weak to avoid Arc cycle).
        // This overwrites the Bundle-facade providers registered by empty_internal(),
        // so bundle_info tables show uncommitted changes from BundleBuilder.
        crate::catalog::register_schema_providers(&builder.bundle.ctx, Arc::downgrade(&builder) as Weak<dyn BundleFacade>)?;

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
            pending_tombstones: RwLock::new(std::collections::HashSet::new()),
            pending_updates: RwLock::new(std::collections::HashMap::new()),
        });

        // Re-register schema providers with BundleBuilder as facade (using Weak to avoid Arc cycle).
        // This overwrites the Bundle-facade providers registered by Bundle::open(),
        // so bundle_info tables show uncommitted changes from BundleBuilder.
        crate::catalog::register_schema_providers(&builder.bundle.ctx, Arc::downgrade(&builder) as Weak<dyn BundleFacade>)?;

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
            let init_commit = InitCommit {
                id: if from.is_none() { Some(bundle_id) } else { None },
                from: from.clone(),
                view: None,
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

        // Write tombstone file if there are pending tombstones from DELETE commands
        let tombstoned_ids = std::mem::take(&mut *self.pending_tombstones.write());
        if !tombstoned_ids.is_empty() {
            use crate::bundle::tombstone;
            use crate::bundle::operation::DeleteOp;

            // Serialize and write tombstone file via content-addressed storage
            let tomb_bytes = tombstone::serialize_tombstone(&tombstoned_ids);
            debug!("[DELETE] Writing tombstone file ({} bytes)", tomb_bytes.len());
            let stream = futures::stream::iter(vec![Ok::<_, std::io::Error>(tomb_bytes)]);
            let write_result = manifest_dir.write_stream(Box::pin(stream), "tomb").await?;
            let tomb_filename = manifest_dir.relative_path(write_result.file.as_ref())?;
            debug!("[DELETE] Tombstone file written: {}", tomb_filename);

            // Add DeleteOp alongside the existing FilterOp.
            // The FilterOp handles query-time exclusion, the DeleteOp records the tombstone.
            // On future opens, the tombstone will be loaded for scan-level filtering.
            let delete_op = DeleteOp::new(&tomb_filename);
            if let Some(last_change) = changes.last_mut() {
                last_change.operations.push(AnyOperation::Delete(delete_op));
            }
        }

        // Write update overlay file if there are pending updates from UPDATE commands
        let pending_upd = std::mem::take(&mut *self.pending_updates.write());
        if !pending_upd.is_empty() {
            use crate::bundle::update_overlay;
            use crate::bundle::operation::UpdateDataOp;

            // Collect column types from the bundle schema
            let schema = self.bundle.schema().await?;
            let col_names = column_metadata::resolved_column_names(&self.operations());
            let mut column_types: std::collections::HashMap<crate::object_id::ColumnId, arrow::datatypes::DataType> = std::collections::HashMap::new();
            for (col_id, col_name) in &col_names {
                if let Some((_, field)) = schema.column_with_name(col_name) {
                    column_types.insert(*col_id, field.data_type().clone());
                }
            }

            let overlay_bytes = update_overlay::write_overlay_parquet(&pending_upd, &column_types)?;
            debug!("[UPDATE] Writing overlay file ({} bytes, {} rows)", overlay_bytes.len(), pending_upd.len());
            let stream = futures::stream::iter(vec![Ok::<_, std::io::Error>(overlay_bytes)]);
            let write_result = manifest_dir.write_stream(Box::pin(stream), "update").await?;
            let overlay_filename = manifest_dir.relative_path(write_result.file.as_ref())?;
            debug!("[UPDATE] Overlay file written: {}", overlay_filename);

            let update_op = UpdateDataOp::new(&overlay_filename);
            if let Some(last_change) = changes.last_mut() {
                last_change.operations.push(AnyOperation::UpdateData(update_op));
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

        // Update base to reflect the committed version
        // Preserve explicit_config from current bundle
        let new_bundle = Bundle::open(&url, passed_config).await?;

        // Replace the bundle contents using reload_from to preserve Arc references
        // open_to_bundle returns Arc<Bundle> so we dereference to get the Bundle
        self.bundle.reload_from((*new_bundle).clone());

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
        self.pending_tombstones.write().clear();
        self.pending_updates.write().clear();

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
    pub async fn undo(&self) -> Result<&Self, BundlebaseError> {
        if self.status().is_empty() {
            return Err("No uncommitted changes to undo".into());
        }

        // Remove the last change
        self.status.write().pop();

        // Reload the bundle from the last committed state
        self.reload_bundle().await?;

        // Reapply all remaining operations
        let changes = self.status.read().changes().clone();
        for change in &changes {
            for op in &change.operations {
                self.bundle.apply_operation(op.clone()).await?;
            }
        }

        info!("Last operation undone");

        Ok(self)
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
        let temp_names = self.bundle.function_registry().read().temporary_only_names();
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
                ).into());
            } else {
                return Err(format!(
                    "Cannot create view: SQL references temporary function '{}'. \
                     Views are persisted and must not depend on temporary functions. \
                     Import '{}' as a persistent function (IMPORT FUNCTION) first.",
                    name, name
                ).into());
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
            // empty() returns Arc<Bundle>, clone inner Bundle for reload_from
            let arc = Bundle::empty(Some(passed_config)).await?;
            let bundle = (*arc).clone();
            bundle.refresh_data_dir().await?;
            *bundle.data_dir.write() = writable_dir_from_url(&Url::parse(&url)?, bundle.config()).await?;
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
            return Err(format!(
                "Operation '{}' is not allowed on a view",
                op.describe()
            )
            .into());
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

        // Resolve user-visible column names to ColumnIds
        let ops = self.operations();
        let resolved = column_metadata::resolved_column_names(&ops);
        let name_to_id: std::collections::HashMap<String, crate::object_id::ColumnId> = resolved
            .iter()
            .map(|(id, name)| (name.clone(), *id))
            .collect();

        // Build rename map for physical → user-visible names (reuse from select_row_ids)
        let initial = column_metadata::initial_column_names(&ops);
        let mut rename_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for (id, resolved_name) in &resolved {
            if let Some(initial_name) = initial.get(id) {
                if initial_name != resolved_name {
                    rename_map.insert(initial_name.clone(), resolved_name.clone());
                }
            }
        }

        // Validate column names
        let col_ids: Vec<(String, crate::object_id::ColumnId)> = columns.iter()
            .map(|col_name| {
                let col_id = name_to_id.get(col_name)
                    .ok_or_else(|| BundlebaseError::from(format!("Column '{}' not found", col_name)))?;
                Ok((col_name.clone(), *col_id))
            })
            .collect::<Result<Vec<_>, BundlebaseError>>()?;

        // Build SELECT clause for expression evaluation
        let select_exprs: Vec<String> = columns.iter().zip(expressions.iter())
            .map(|(col, expr)| format!("{} AS {}", expr, col))
            .collect();
        let select_list = select_exprs.join(", ");

        let mut updated_count = 0usize;

        let base_pack = self.bundle.packs().read()
            .get(&ObjectId::BASE_PACK)
            .cloned();

        if let Some(pack) = base_pack {
            let blocks = pack.blocks();
            for (idx, block) in blocks.iter().enumerate() {
                let block_ref = ObjectIdAlias::from(idx as u16);
                let mut stream = block.reader()
                    .extract_rowids_stream(block_ref, self.bundle.ctx(), None)
                    .await?;

                while let Some(batch_result) = stream.next().await {
                    let rowid_batch = batch_result?;
                    let batch = &rowid_batch.batch;
                    let row_ids = &rowid_batch.row_ids;

                    // Rename columns to user-visible names
                    let batch = if rename_map.is_empty() {
                        batch.clone()
                    } else {
                        let schema = batch.schema();
                        let new_fields: Vec<arrow::datatypes::Field> = schema.fields().iter().map(|f| {
                            if let Some(new_name) = rename_map.get(f.name()) {
                                f.as_ref().clone().with_name(new_name)
                            } else {
                                f.as_ref().clone()
                            }
                        }).collect();
                        let new_schema = Arc::new(arrow::datatypes::Schema::new_with_metadata(
                            new_fields,
                            schema.metadata().clone(),
                        ));
                        arrow::record_batch::RecordBatch::try_new(new_schema, batch.columns().to_vec())?
                    };

                    // Evaluate: SELECT _rowid, <set_exprs> FROM (data with _rowid) WHERE <condition>
                    let eval_sql = format!(
                        "SELECT CAST(_idx AS BIGINT) AS _idx, {} FROM (SELECT *, ROW_NUMBER() OVER () - 1 AS _idx FROM __update_batch) WHERE {}",
                        select_list,
                        where_clause
                    );

                    let mut config = datafusion::prelude::SessionConfig::new();
                    config.options_mut().sql_parser.enable_ident_normalization = false;
                    let temp_ctx = SessionContext::new_with_config_rt(
                        config,
                        self.bundle.ctx().runtime_env(),
                    );
                    let mem_table = datafusion::datasource::MemTable::try_new(
                        batch.schema(),
                        vec![vec![batch.clone()]],
                    )?;
                    temp_ctx.register_table("__update_batch", Arc::new(mem_table))?;

                    let result_df = temp_ctx.sql(&eval_sql).await
                        .map_err(|e| BundlebaseError::from(e.to_string()))?;
                    let result_batches = result_df.collect().await
                        .map_err(|e| BundlebaseError::from(e.to_string()))?;

                    let mut pending = self.pending_updates.write();
                    for result_batch in &result_batches {
                        let idx_col = result_batch.column(0)
                            .as_any()
                            .downcast_ref::<arrow::array::Int64Array>()
                            .ok_or_else(|| BundlebaseError::from("Expected Int64 _idx column"))?;

                        for row in 0..result_batch.num_rows() {
                            let batch_idx = idx_col.value(row) as usize;
                            if batch_idx >= row_ids.len() {
                                continue;
                            }
                            let row_id = row_ids[batch_idx];

                            let cell_updates = pending.entry(row_id).or_insert_with(std::collections::HashMap::new);
                            for (col_idx, (_, col_id)) in col_ids.iter().enumerate() {
                                // Column is at position col_idx + 1 (after _idx)
                                let value = datafusion::scalar::ScalarValue::try_from_array(
                                    result_batch.column(col_idx + 1),
                                    row,
                                ).map_err(|e| BundlebaseError::from(e.to_string()))?;
                                cell_updates.insert(*col_id, value);
                            }
                            updated_count += 1;
                        }
                    }
                }
            }
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
        log::debug!("[UPDATE] Flushing {} pending updates to blocks", pending.len());

        // Group by block_ref
        let mut by_block: std::collections::HashMap<u16, crate::bundle::update_overlay::UpdateOverlay> = std::collections::HashMap::new();
        for (row_id, cell_updates) in pending.iter() {
            let block_idx = row_id.block_ref().as_u16();
            let overlay = by_block.entry(block_idx).or_insert_with(|| {
                crate::bundle::update_overlay::UpdateOverlay {
                    updates: std::collections::HashMap::new(),
                }
            });
            overlay.updates.insert(*row_id, cell_updates.clone());
        }

        let packs = self.bundle.packs().read().clone();
        if let Some(pack) = packs.get(&ObjectId::BASE_PACK) {
            let blocks = pack.blocks();
            for (block_idx, overlay) in by_block {
                if let Some(block) = blocks.get(block_idx as usize) {
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

    /// Add RowIds to the pending tombstone set.
    ///
    /// These will be written to a tombstone file on commit.
    pub fn mark_deleted(&self, row_ids: std::collections::HashSet<bundlebase_common::RowId>) {
        self.pending_tombstones.write().extend(row_ids);
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

        // Build a physical-name → user-visible-name mapping from column renames
        let ops = self.operations();
        let initial = column_metadata::initial_column_names(&ops);
        let resolved = column_metadata::resolved_column_names(&ops);
        let mut rename_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for (id, resolved_name) in &resolved {
            if let Some(initial_name) = initial.get(id) {
                if initial_name != resolved_name {
                    rename_map.insert(initial_name.clone(), resolved_name.clone());
                }
            }
        }

        let mut matching_ids = std::collections::HashSet::new();
        let base_pack = self.bundle.packs().read()
            .get(&ObjectId::BASE_PACK)
            .cloned();

        if let Some(pack) = base_pack {
            let blocks = pack.blocks();
            for (idx, block) in blocks.iter().enumerate() {
                let block_ref = ObjectIdAlias::from(idx as u16);
                let mut stream = block.reader()
                    .extract_rowids_stream(block_ref, self.bundle.ctx(), None)
                    .await?;

                while let Some(batch_result) = stream.next().await {
                    let rowid_batch = batch_result?;
                    let batch = &rowid_batch.batch;
                    let row_ids = &rowid_batch.row_ids;

                    // Rename columns in the batch to match user-visible names
                    let batch = if rename_map.is_empty() {
                        batch.clone()
                    } else {
                        let schema = batch.schema();
                        let new_fields: Vec<arrow::datatypes::Field> = schema.fields().iter().map(|f| {
                            if let Some(new_name) = rename_map.get(f.name()) {
                                f.as_ref().clone().with_name(new_name)
                            } else {
                                f.as_ref().clone()
                            }
                        }).collect();
                        let new_schema = Arc::new(arrow::datatypes::Schema::new_with_metadata(
                            new_fields,
                            schema.metadata().clone(),
                        ));
                        arrow::record_batch::RecordBatch::try_new(new_schema, batch.columns().to_vec())?
                    };

                    let filter_sql = format!(
                        "SELECT CAST(_idx AS BIGINT) AS _idx FROM (SELECT *, ROW_NUMBER() OVER () - 1 AS _idx FROM __delete_batch) WHERE {}",
                        where_clause
                    );

                    let mut config = datafusion::prelude::SessionConfig::new();
                    config.options_mut().sql_parser.enable_ident_normalization = false;
                    let temp_ctx = SessionContext::new_with_config_rt(
                        config,
                        self.bundle.ctx().runtime_env(),
                    );
                    let mem_table = datafusion::datasource::MemTable::try_new(
                        batch.schema(),
                        vec![vec![batch.clone()]],
                    )?;
                    temp_ctx.register_table("__delete_batch", Arc::new(mem_table))?;

                    let idx_df = temp_ctx.sql(&filter_sql).await
                        .map_err(|e| BundlebaseError::from(e.to_string()))?;
                    let idx_batches = idx_df.collect().await
                        .map_err(|e| BundlebaseError::from(e.to_string()))?;

                    for idx_batch in &idx_batches {
                        let idx_col = idx_batch.column(0);
                        for i in 0..idx_batch.num_rows() {
                            use arrow::array::{Array, AsArray};
                            use arrow::datatypes::DataType;
                            let val = match idx_col.data_type() {
                                DataType::UInt64 => idx_col.as_primitive::<arrow::datatypes::UInt64Type>().value(i) as usize,
                                DataType::Int64 => idx_col.as_primitive::<arrow::datatypes::Int64Type>().value(i) as usize,
                                dt => return Err(format!(
                                    "Unexpected column type {:?} from ROW_NUMBER()", dt
                                ).into()),
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
                    if let Some(change) = self.in_progress_change.write().take() {
                        self.status.write().push_change(change);
                        // Re-register version UDF to reflect builder state (e.g., "UNCOMMITTED")
                        self.bundle.function_registry().read().refresh_version_udf(self.version());
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
                    if let Some(change) = self.in_progress_change.write().take() {
                        self.status.write().push_change(change);
                        // Re-register version UDF to reflect builder state (e.g., "UNCOMMITTED")
                        self.bundle.function_registry().read().refresh_version_udf(self.version());
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
        Ok(self.bundle().connector_registry().write().remove_entry(name, platform.as_ref(), true))
    }

    /// Drop runtime-only function (session-only, no operation created).
    pub async fn drop_temp_function(
        &self,
        name: &str,
        platform: Option<&str>,
    ) -> Result<usize, BundlebaseError> {
        use crate::platform::Platform;
        let platform: Option<Platform> = platform.map(|s| s.parse()).transpose()?;
        let _ = self.bundle().ctx().deregister_udf(name);
        let _ = self.bundle().ctx().deregister_udaf(name);
        Ok(self.bundle().function_registry().write().remove(name, platform.as_ref(), true))
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
            let first_col_id = index_column_ids.first()
                .ok_or_else(|| BundlebaseError::from(
                    format!("Index '{}' has no columns defined", index_id)
                ))?;

            debug!("Checking index on column IDs {:?}", &index_column_ids);

            // Use blocks_for_column to find which blocks need indexing
            let candidate_blocks = column_metadata::blocks_for_column(&operations, first_col_id);

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
        let mut ops = self.bundle.operations.read().clone();
        ops.append(&mut self.status().operations().clone());
        ops
    }

    fn column_names(&self) -> column_metadata::ColumnNames {
        column_metadata::resolved_column_names(&self.operations())
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

    async fn extend(
        &self,
        data_dir: Option<&str>,
    ) -> Result<Arc<BundleBuilder>, BundlebaseError> {
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

    async fn export_tar(&self, tar_path: &str) -> Result<String, BundlebaseError> {
        // Check for uncommitted changes
        if !self.status().is_empty() {
            return Err("Cannot export tar with uncommitted changes. Please commit first.".into());
        }

        self.bundle.export_tar(tar_path).await
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

    fn views_by_name(&self) -> HashMap<String, ObjectId> {
        self.bundle.views.read().clone()
    }

    fn always_delete_rules(&self) -> Vec<String> {
        self.bundle.always_delete_rules()
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
        Ok(self.bundle.connector_registry().write().remove_entry(name, platform, true))
    }

    async fn drop_temp_function(
        &self,
        name: &str,
        platform: Option<&crate::platform::Platform>,
    ) -> Result<usize, BundlebaseError> {
        let _ = self.bundle.ctx().deregister_udf(name);
        let _ = self.bundle.ctx().deregister_udaf(name);
        Ok(self.bundle.function_registry().write().remove(name, platform, true))
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
