use crate::bundle::command::{BundleBuilderCommand, BundleCommand, CommandResponse, FacadeCommand, FetchAllCommand, FetchCommand};
use crate::impl_dyn_command_response;
use crate::bundle::command::response::OutputShape;
use crate::bundle::facade::BundleFacade;
use crate::bundle::init::InitCommit;
use crate::bundle::operation::AnyOperation;
use crate::bundle::operation::{BundleChange, IndexBlocksOp, Operation};
use crate::bundle::{commit, Pack, INIT_FILENAME, META_DIR};
use crate::bundle::{sql, Bundle};
use super::DataBlock;
use crate::data::{ObjectId, VersionedBlockId};
use crate::source::FetchResults;
use crate::functions::FunctionImpl;
use crate::functions::FunctionSignature;
use crate::index::{IndexDefinition, IndexType};
use crate::io::{writable_dir_from_str, writable_dir_from_url, write_yaml, IOReadWriteDir};
use crate::BundleConfig;
use crate::BundlebaseError;
use arrow::array::{ArrayRef, Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use chrono::DateTime;
use datafusion::execution::SendableRecordBatchStream;
use datafusion::prelude::{DataFrame, SessionContext};
use datafusion::scalar::ScalarValue;
use parking_lot::RwLock;
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};
use std::collections::HashMap;
use std::future::Future;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::Arc;
use url::Url;
use crate::bundle::pack::JoinTypeOption;

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

    fn to_record_batch(&self) -> Result<RecordBatch, BundlebaseError> {
        let changes = self.changes();

        let ids: Vec<i32> = (0..changes.len() as i32).collect();
        let change_ids: Vec<String> = changes.iter().map(|c| c.id.to_string()).collect();
        let descriptions: Vec<&str> = changes.iter().map(|c| c.description.as_str()).collect();
        let operation_counts: Vec<i32> = changes
            .iter()
            .map(|c| c.operations.len() as i32)
            .collect();

        RecordBatch::try_new(
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
        .map_err(|e| BundlebaseError::from(format!("Failed to create record batch: {}", e)))
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
}

impl Clone for BundleBuilder {
    fn clone(&self) -> Self {
        Self {
            bundle: Arc::clone(&self.bundle),
            in_progress_change: RwLock::new(self.in_progress_change.read().clone()),
            status: RwLock::new(self.status.read().clone()),
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
        config: Option<BundleConfig>,
    ) -> Result<Arc<BundleBuilder>, BundlebaseError> {
        let bundle = Bundle::empty().await?;

        // Modify the bundle via interior mutability
        *bundle.passed_config.write() = config;
        bundle.recompute_config()?;
        *bundle.data_dir.write() = writable_dir_from_str(path, bundle.config())?;

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
        });

        // Re-register schema providers with BundleBuilder as facade.
        // This overwrites the Bundle-facade providers registered by empty_internal(),
        // so bundle_info tables show uncommitted changes from BundleBuilder.
        Bundle::register_schema_providers(&builder.bundle.ctx, builder.clone())?;

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
    pub fn extend(
        bundle: Arc<Bundle>,
        data_dir: Option<&str>,
    ) -> Result<Arc<BundleBuilder>, BundlebaseError> {
        let mut new_bundle = bundle.deref().clone();

        // Detach data_dir and last_manifest_version so modifications don't affect the original
        new_bundle.detach_for_extend();

        // If data_dir is provided and not empty, use it; otherwise keep the current bundle's data_dir
        if let Some(dir) = data_dir {
            if !dir.is_empty() {
                let new_data_dir = writable_dir_from_str(dir, bundle.config())?;
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
        });

        // Re-register schema providers with BundleBuilder as facade.
        // This overwrites the Bundle-facade providers registered by Bundle::open(),
        // so bundle_info tables show uncommitted changes from BundleBuilder.
        Bundle::register_schema_providers(&builder.bundle.ctx, builder.clone())?;

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
        let manifest_dir = self.bundle.data_dir().writable_subdir(META_DIR)?;
        let last_manifest_version = *self.bundle.last_manifest_version.read();
        let from = self.bundle.from();
        let changes = self.status.read().changes().clone();
        let config = self.bundle.passed_config.read().clone();
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
        let new_bundle = Bundle::open(&url, config).await?;

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

    pub(in crate::bundle) async fn reload_bundle(&self) -> Result<(), BundlebaseError> {
        // Reload the bundle from the last committed state
        let empty = self.bundle.commits.read().is_empty();
        let passed_config = self.bundle.passed_config.read().clone();
        let url = self.bundle.url().to_string();

        // Note: reload_from preserves the original ctx and its schema providers
        // which already have the correct facade set
        let new_bundle: Bundle = if empty {
            // empty() returns Arc<Bundle>, clone inner Bundle for reload_from
            let arc = Bundle::empty().await?;
            let bundle = (*arc).clone();
            *bundle.passed_config.write() = passed_config;
            bundle.recompute_config()?;
            *bundle.data_dir.write() = writable_dir_from_url(&Url::parse(&url)?, bundle.config())?;
            bundle
        } else {
            // Preserve explicit_config when reopening
            // open returns Arc<Bundle>, so we clone the inner Bundle
            let arc_bundle = Bundle::open(&url, passed_config).await?;
            (*arc_bundle).clone()
        };

        // Update bundle contents using reload_from to preserve Arc references
        self.bundle.reload_from(new_bundle);
        Ok(())
    }

    pub(in crate::bundle) async fn apply_operation(&self, op: AnyOperation) -> Result<(), BundlebaseError> {
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
    pub(in crate::bundle) async fn do_change<F>(&self, description: &str, f: F) -> Result<(), BundlebaseError>
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

    /// Execute a builder command on this BundleBuilder.
    ///
    /// This is the primary way to execute commands that implement the `BundleBuilderCommand` trait.
    /// The command's description is used as the change description for tracking.
    ///
    /// # Arguments
    /// * `cmd` - The command to execute
    ///
    /// # Returns
    /// * `Ok(C::Output)` - Command's output on success
    /// * `Err(BundlebaseError)` - Execution failed
    pub async fn execute_command<C: BundleBuilderCommand + 'static>(
        &self,
        cmd: C,
    ) -> Result<C::Output, BundlebaseError> {
        use crate::bundle::operation::BundleChange;

        let description = cmd.to_statement();

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
        let result = Box::new(cmd).execute(self).await;

        // Only finalize the change if we created it (not nested)
        match &result {
            Ok(_) => {
                debug!("Command succeeded: {}", description);
                if !is_nested {
                    if let Some(change) = self.in_progress_change.write().take() {
                        self.status.write().push_change(change);
                    }
                }
            }
            Err(e) => {
                warn!("Command failed: {}: {}", description, e);
                if !is_nested {
                    // On failure, discard the in-progress change
                    self.in_progress_change.write().take();
                }
            }
        }

        result
    }

    /// Attach a data block to the bundle.
    ///
    /// # Arguments
    /// * `path` - The location/URL of the data to attach
    /// * `pack` - The pack to attach to. Use `None` or `"base"` for the base pack,
    ///            or a join name to attach to that join's pack.
    pub async fn attach(
        &self,
        path: &str,
        pack: Option<&str>,
    ) -> Result<&Self, BundlebaseError> {
        use crate::bundle::command::AttachCommand;
        self.execute_command(AttachCommand::new(path, pack.map(|s| s.to_string()))).await?;
        Ok(self)
    }

    /// Detach a data block from the bundle by its location.
    ///
    /// This removes a previously attached block from the bundle. The block
    /// is identified by its location (URL), and the operation stores the
    /// block ID for deterministic replay.
    ///
    /// # Arguments
    /// * `location` - The location (URL) of the block to detach
    ///
    /// # Example
    /// ```ignore
    /// builder.detach_block("s3://bucket/data.parquet").await?;
    /// ```
    pub async fn detach_block(&self, location: &str) -> Result<&Self, BundlebaseError> {
        use crate::bundle::command::DetachBlockCommand;
        self.execute_command(DetachBlockCommand::new(location)).await?;
        Ok(self)
    }

    /// Replace a block's location in the bundle.
    ///
    /// This changes where a block's data is read from without changing the
    /// block's identity. Useful when data files are moved to a new location.
    ///
    /// # Arguments
    /// * `old_location` - The current location (URL) of the block
    /// * `new_location` - The new location (URL) to read data from
    ///
    /// # Example
    /// ```ignore
    /// builder.replace_block(
    ///     "s3://old-bucket/data.parquet",
    ///     "s3://new-bucket/data.parquet"
    /// ).await?;
    /// ```
    pub async fn replace_block(
        &self,
        old_location: &str,
        new_location: &str,
    ) -> Result<&Self, BundlebaseError> {
        use crate::bundle::command::ReplaceBlockCommand;
        self.execute_command(ReplaceBlockCommand::new(old_location, new_location)).await?;
        Ok(self)
    }

    /// Create a data source for a pack.
    ///
    /// A source specifies where to look for data files (e.g., S3 bucket prefix)
    /// and patterns to filter which files to include. This enables the `fetch()`
    /// functionality to discover and auto-attach new files.
    ///
    /// # Arguments
    /// * `function` - Source function name (e.g., "remote_dir")
    /// * `args` - Function-specific arguments. For "remote_dir":
    ///   - "url" (required): Directory URL to list (e.g., "s3://bucket/data/")
    ///   - "patterns" (optional): Comma-separated glob patterns (e.g., "**/*.parquet,**/*.csv")
    /// * `pack` - Which pack to create the source for:
    ///   - `None` or `Some("base")`: The base pack (default)
    ///   - `Some(join_name)`: A joined pack by its join name
    ///
    /// # Example
    /// ```ignore
    /// let builder = BundleBuilder::create("memory:///work", None).await?;
    /// let mut args = HashMap::new();
    /// args.insert("url".to_string(), "s3://bucket/data/".to_string());
    /// args.insert("patterns".to_string(), "**/*.parquet".to_string());
    /// builder.create_source("remote_dir", args, None).await?;
    /// builder.fetch(None).await?;  // Fetch from base pack sources
    /// builder.commit("Initial data from source").await?;
    /// ```
    pub async fn create_source(
        &self,
        function: &str,
        args: HashMap<String, String>,
        pack: Option<&str>,
    ) -> Result<&Self, BundlebaseError> {
        use crate::bundle::command::CreateSourceCommand;
        self.execute_command(CreateSourceCommand::new(function, args, pack.map(|s| s.to_string()))).await?;
        Ok(self)
    }

    /// Fetch from sources for a pack - discover and attach new files.
    ///
    /// Lists files from the source URLs, compares with already-attached files,
    /// and auto-attaches any new files.
    ///
    /// # Arguments
    /// * `pack` - Which pack to fetch sources for:
    ///   - `None` or `Some("base")`: The base pack (default)
    ///   - `Some(join_name)`: A joined pack by its join name
    ///
    /// # Returns
    /// A list of `FetchResults`, one for each source in the pack.
    /// Each result contains details about blocks added, replaced, and removed.
    ///
    /// # Example
    /// ```ignore
    /// let builder = BundleBuilder::create("memory:///work", None).await?;
    /// let mut args = HashMap::new();
    /// args.insert("url".to_string(), "s3://bucket/data/".to_string());
    /// args.insert("patterns".to_string(), "**/*.parquet".to_string());
    /// builder.create_source("remote_dir", args, None).await?;
    /// let results = builder.fetch(None).await?;  // Fetch from base pack sources
    /// for result in &results {
    ///     println!("Source {}: {} added", result.source_function, result.added.len());
    /// }
    /// ```
    pub async fn fetch(&self, pack: Option<&str>) -> Result<Vec<FetchResults>, BundlebaseError> {
        self.execute_command(FetchCommand::new(pack.map(|s| s.to_string()))).await
    }

    /// Fetch from all defined sources - discover and attach new files.
    ///
    /// Lists files from each source URL, compares with already-attached files,
    /// and auto-attaches any new files.
    ///
    /// # Returns
    /// A list of `FetchResults`, one for each source across all packs.
    /// Includes results for sources with no changes (empty results).
    ///
    /// # Example
    /// ```ignore
    /// let builder = BundleBuilder::create("memory:///work", None).await?;
    /// // Create multiple sources...
    /// let results = builder.fetch_all().await?;
    /// for result in &results {
    ///     println!("Source {}: {} added, {} replaced, {} removed",
    ///         result.source_function,
    ///         result.added.len(),
    ///         result.replaced.len(),
    ///         result.removed.len());
    /// }
    /// ```
    pub async fn fetch_all(&self) -> Result<Vec<FetchResults>, BundlebaseError> {
        self.execute_command(FetchAllCommand::new()).await
    }

    /// Create a view from a SQL statement
    ///
    /// Creates a named view defined by the SQL query. The view is stored in a subdirectory
    /// under view_{id}/ and automatically inherits data from the parent bundle.
    ///
    /// # Arguments
    /// * `name` - Name of the view
    /// * `sql` - SQL query that defines the view (e.g., "SELECT * FROM bundle WHERE age > 21")
    ///
    /// # Returns
    /// The BundleBuilder for the created view
    ///
    /// # Example
    /// ```ignore
    /// let c = BundleBuilder::create("memory:///container", None).await?;
    /// c.attach("data.csv", None).await?;
    /// c.commit("Initial").await?;
    ///
    /// let view = c.create_view("adults", "SELECT * FROM bundle WHERE age > 21").await?;
    /// c.commit("Add adults view").await?;
    /// ```
    pub async fn create_view(
        &self,
        name: &str,
        sql: &str,
    ) -> Result<Arc<BundleBuilder>, BundlebaseError> {
        use crate::bundle::operation::CreateViewOp;

        let name_clone = name.to_string();
        let sql_clone = sql.to_string();

        // Use a cell to capture the view_builder from inside the closure.
        // We use parking_lot::RwLock which doesn't poison on panic.
        let view_builder_cell: Arc<parking_lot::RwLock<Option<Arc<BundleBuilder>>>> =
            Arc::new(parking_lot::RwLock::new(None));
        let view_builder_cell_clone = view_builder_cell.clone();

        self.do_change(&format!("Create view '{}'", name), |builder| {
            let name = name_clone.clone();
            let sql = sql_clone.clone();
            let cell = view_builder_cell_clone.clone();
            Box::pin(async move {
                let (op, view_builder) = CreateViewOp::setup(&name, &sql, builder).await?;
                *cell.write() = Some(view_builder);
                builder.apply_operation(op.into()).await?;
                info!("Created view '{}'", name);
                Ok(())
            })
        })
        .await?;

        // Extract the view builder from the cell
        let view_builder = view_builder_cell
            .read()
            .clone()
            .ok_or_else(|| BundlebaseError::from("View builder not created"))?;

        Ok(view_builder)
    }

    /// Rename an existing view
    ///
    /// # Arguments
    /// * `old_name` - The current name of the view
    /// * `new_name` - The new name for the view
    ///
    /// # Example
    /// ```ignore
    /// let c = BundleBuilder::create("memory:///example", None).await?;
    /// c.attach("data.csv", None).await?;
    /// c.create_view("adults", "SELECT * FROM bundle WHERE age > 21").await?;
    /// c.rename_view("adults", "adults_view").await?;
    /// c.commit("Renamed view").await?;
    /// ```
    pub async fn rename_view(
        &self,
        old_name: &str,
        new_name: &str,
    ) -> Result<&Self, BundlebaseError> {
        use crate::bundle::command::RenameViewCommand;
        self.execute_command(RenameViewCommand::new(old_name, new_name)).await?;
        Ok(self)
    }

    /// Drop an existing view
    ///
    /// # Arguments
    /// * `view_name` - The name of the view to drop
    ///
    /// # Example
    /// ```ignore
    /// let c = BundleBuilder::create("memory:///example", None).await?;
    /// c.attach("data.csv", None).await?;
    /// c.create_view("adults", "SELECT * FROM bundle WHERE age > 21").await?;
    /// c.drop_view("adults").await?;
    /// c.commit("Dropped view").await?;
    /// ```
    pub async fn drop_view(
        &self,
        view_name: &str,
    ) -> Result<&Self, BundlebaseError> {
        use crate::bundle::command::DropViewCommand;
        self.execute_command(DropViewCommand::new(view_name)).await?;
        Ok(self)
    }

    /// Drop an existing join
    ///
    /// # Arguments
    /// * `join_name` - The name of the join to drop
    ///
    /// # Example
    /// ```ignore
    /// let c = BundleBuilder::create("memory:///example", None).await?;
    /// c.attach("data.csv", None).await?;
    /// c.join("customers", "base.customer_id = customers.id", Some("customers.parquet"), JoinTypeOption::Left).await?;
    /// c.drop_join("customers").await?;
    /// c.commit("Dropped join").await?;
    /// ```
    pub async fn drop_join(&self, join_name: &str) -> Result<&Self, BundlebaseError> {
        use crate::bundle::command::DropJoinCommand;
        self.execute_command(DropJoinCommand::new(join_name)).await?;
        Ok(self)
    }

    /// Rename an existing join
    ///
    /// # Arguments
    /// * `old_name` - The current name of the join
    /// * `new_name` - The new name for the join
    ///
    /// # Example
    /// ```ignore
    /// let c = BundleBuilder::create("memory:///example", None).await?;
    /// c.attach("data.csv", None).await?;
    /// c.join("customers", "base.customer_id = customers.id", Some("customers.parquet"), JoinTypeOption::Left).await?;
    /// c.rename_join("customers", "clients").await?;
    /// c.commit("Renamed join").await?;
    /// ```
    pub async fn rename_join(
        &self,
        old_name: &str,
        new_name: &str,
    ) -> Result<&Self, BundlebaseError> {
        use crate::bundle::command::RenameJoinCommand;
        self.execute_command(RenameJoinCommand::new(old_name, new_name)).await?;
        Ok(self)
    }

    /// Drop a column
    pub async fn drop_column(&self, name: &str) -> Result<&Self, BundlebaseError> {
        use crate::bundle::command::DropColumnCommand;
        self.execute_command(DropColumnCommand::new(name)).await?;
        Ok(self)
    }

    /// Rename a column
    pub async fn rename_column(
        &self,
        old_name: &str,
        new_name: &str,
    ) -> Result<&Self, BundlebaseError> {
        use crate::bundle::command::RenameColumnCommand;
        self.execute_command(RenameColumnCommand::new(old_name, new_name)).await?;
        Ok(self)
    }

    /// Filter rows with a SELECT query.
    /// Parameters can be referenced as $1, $2, etc. in the query.
    pub async fn filter(
        &self,
        query: &str,
        params: Vec<ScalarValue>,
    ) -> Result<&Self, BundlebaseError> {
        use crate::bundle::command::FilterCommand;
        self.execute_command(FilterCommand::new(query, params)).await?;
        Ok(self)
    }

    /// Join with another data source
    ///
    /// If `location` is None, the join point is created without any initial data.
    /// Data can be attached later using `attach()` or `create_source()` with the `pack` parameter.
    pub async fn join(
        &self,
        name: &str,
        expression: &str,
        location: Option<&str>,
        join_type: JoinTypeOption,
    ) -> Result<&Self, BundlebaseError> {
        use crate::bundle::command::JoinCommand;
        self.execute_command(JoinCommand::new(name, expression, location.map(|s| s.to_string()), join_type)).await?;
        Ok(self)
    }

    /// Create a custom function
    pub async fn create_function(
        &self,
        signature: FunctionSignature,
    ) -> Result<&Self, BundlebaseError> {
        use crate::bundle::operation::CreateFunctionOp;

        let name = signature.name().to_string();

        self.do_change(&format!("Create function {}", name), |builder| {
            Box::pin(async move {
                builder
                    .apply_operation(CreateFunctionOp::setup(signature).into())
                    .await?;
                Ok(())
            })
        })
        .await?;

        Ok(self)
    }

    /// Set the implementation for a function
    pub async fn set_impl(
        &self,
        name: &str,
        def: Arc<dyn FunctionImpl>,
    ) -> Result<&Self, BundlebaseError> {
        self.bundle.function_registry.write().set_impl(name, def)?;
        Ok(self)
    }

    /// Set the bundle's name
    pub async fn set_name(&self, name: &str) -> Result<&Self, BundlebaseError> {
        use crate::bundle::command::SetNameCommand;
        self.execute_command(SetNameCommand::new(name)).await?;
        Ok(self)
    }

    /// Set the bundle's description
    pub async fn set_description(
        &self,
        description: &str,
    ) -> Result<&Self, BundlebaseError> {
        use crate::bundle::command::SetDescriptionCommand;
        self.execute_command(SetDescriptionCommand::new(description)).await?;
        Ok(self)
    }

    /// Set a configuration value
    ///
    /// Config stored via this operation has the lowest priority:
    /// 1. Explicit config passed to create()/open() (highest)
    /// 2. Environment variables
    /// 3. Config from set_config operations (lowest)
    ///
    /// # Arguments
    /// * `key` - Configuration key (e.g., "region", "access_key_id")
    /// * `value` - Configuration value
    /// * `url_prefix` - Optional URL prefix for URL-specific config (e.g., "s3://bucket/")
    pub async fn set_config(
        &self,
        key: &str,
        value: &str,
        url_prefix: Option<&str>,
    ) -> Result<&Self, BundlebaseError> {
        use crate::bundle::command::SetConfigCommand;
        self.execute_command(SetConfigCommand::new(key, value, url_prefix.map(|s| s.to_string()))).await?;
        Ok(self)
    }

    /// Create an index on a column
    ///
    /// # Arguments
    /// * `column` - The column name to index
    /// * `index_type` - The type of index to create (Column or Text), already configured
    ///
    /// # Example
    /// ```ignore
    /// use bundlebase::IndexType;
    ///
    /// // Column index
    /// builder.create_index("email", IndexType::Column).await?;
    ///
    /// // Text/BM25 index with English stemming
    /// builder.create_index("content", IndexType::Text {
    ///     tokenizer: TokenizerConfig::EnglishStem
    /// }).await?;
    /// ```
    pub async fn create_index(
        &self,
        column: &str,
        index_type: IndexType,
    ) -> Result<&Self, BundlebaseError> {
        use crate::bundle::command::CreateIndexCommand;
        self.execute_command(CreateIndexCommand::new(column, index_type)).await?;
        Ok(self)
    }

    /// Drop an index on a column
    pub async fn drop_index(&self, column: &str) -> Result<&Self, BundlebaseError> {
        use crate::bundle::command::DropIndexCommand;
        self.execute_command(DropIndexCommand::new(column)).await?;
        Ok(self)
    }

    /// Creates index files for anything missing based on the defined indexes.
    ///
    /// This method ensures that all blocks have index files for columns that have been
    /// defined as indexed (via `index()` method). It checks existing indexes to avoid
    /// redundant work and skips blocks that are already indexed at the current version.
    ///
    /// # Behavior
    /// - Analyzes the logical schema to find physical sources for indexed columns
    /// - Filters out blocks that already have up-to-date indexes
    /// - Streams data from each block to build value-to-rowid mappings
    /// - Registers indexes with the IndexManager
    /// - Continues processing other columns if one fails (logs warning)
    ///
    /// # Returns
    /// - `Ok(&Self)` - Successfully processed all indexes
    /// - `Err(BundlebaseError)` - If a critical operation fails (e.g., block not found during setup)
    ///
    /// # Note
    /// This is typically called automatically by `index()` method after defining a new index.
    /// Manual calls are useful when recovering from partial index creation failures.
    pub async fn reindex(&self) -> Result<&Self, BundlebaseError> {
        use crate::bundle::command::ReindexCommand;
        self.execute_command(ReindexCommand::new()).await?;
        Ok(self)
    }

    /// Internal reindex implementation that doesn't wrap in do_change.
    ///
    /// This is used by commands that need to reindex within their own change context.
    pub(in crate::bundle) async fn reindex_internal(&self) -> Result<(), BundlebaseError> {
        // Group blocks by (index_id, column_name) for batching
        let mut blocks_to_index: HashMap<(ObjectId, String), Vec<(ObjectId, String)>> =
            HashMap::new();

        // Ensure dataframe is set up for queries
        let df = self.dataframe().await?;

        // Collect index definitions before the loop to avoid holding the lock across awaits
        let index_defs: Vec<Arc<IndexDefinition>> =
            self.bundle.indexes.read().iter().cloned().collect();

        let packs = self.bundle.packs().clone();

        for index_def in &index_defs {
            let logical_col = index_def.column().to_string();
            let index_id = index_def.id();
            debug!("Checking index on {}", &logical_col);

            // Pass packs to expand pack tables into block tables
            let sources = match sql::column_sources_from_df(
                logical_col.as_str(),
                &df,
                Some(&packs),
            )
            .await
            {
                Ok(Some(s)) => s,
                Ok(None) => {
                    return Err(format!(
                        "No physical sources found for column '{}'",
                        logical_col
                    )
                    .into());
                }
                Err(e) => {
                    return Err(format!(
                        "Failed to find source for column '{}': {}",
                        logical_col, e
                    )
                    .into());
                }
            };

            for (source_table, source_col) in sources {
                // Extract block ID from table name "blocks.__block_{hex_id}"
                let block_id = DataBlock::parse_id(&source_table).ok_or_else(|| {
                    BundlebaseError::from(format!("Invalid table: {}", source_table))
                })?;

                // Find the block and get its version
                let block_version = self
                    .find_block_version(&block_id)
                    .ok_or_else(|| format!("Block {} not found in packs", block_id))?;
                debug!(
                    "Physical source: block {} version {}",
                    &block_id, &block_version
                );

                // Check if index already exists at this version
                let versioned_block =
                    VersionedBlockId::new(block_id, block_version.clone());
                let needs_index = self
                    .bundle()
                    .get_index(&source_col, &versioned_block)
                    .is_none();
                debug!("Needs index? {}", needs_index);

                if needs_index {
                    blocks_to_index
                        .entry((*index_id, source_col.clone()))
                        .or_default()
                        .push((block_id, block_version));
                }
            }
        }

        // Create IndexBlocksOp for each group of blocks
        for ((index_id, column), blocks) in blocks_to_index {
            if !blocks.is_empty() {
                debug!(
                    "Creating IndexBlocksOp for column {} with {} blocks",
                    column,
                    blocks.len()
                );

                // Bundle is internally thread-safe
                let op = IndexBlocksOp::setup(&index_id, &column, blocks, self).await?;
                self.apply_operation(op.into()).await?;
            }
        }

        info!("Reindexed all columns");

        Ok(())
    }

    /// Find the version of a block by its ID
    fn find_block_version(&self, block_id: &ObjectId) -> Option<String> {
        for pack in self.bundle.packs().read().values() {
            for block in pack.blocks() {
                if block.id() == block_id {
                    return Some(block.version());
                }
            }
        }
        None
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

    /// Rebuild an index on a column
    pub async fn rebuild_index(&self, column: &str) -> Result<&Self, BundlebaseError> {
        use crate::bundle::command::RebuildIndexCommand;
        self.execute_command(RebuildIndexCommand::new(column)).await?;
        Ok(self)
    }

    /// Get the physical source (pack name, column name) for a logical column
    ///
    /// This analyzes the DataFusion execution plan to trace a column back to its
    /// original source, accounting for renames and joins.
    ///
    /// # Returns
    /// - `Some(ColumnSource)` - The pack name and physical column name if found
    /// - `None` - For computed columns or columns that don't map to a single source
    pub async fn get_column_source(
        &self,
        logical_name: &str,
    ) -> Result<Option<crate::bundle::ColumnSource>, BundlebaseError> {
        // Get the logical plan
        let df = self.dataframe().await?;
        let plan = df.logical_plan();

        // Create analyzer with table-to-pack mappings
        let mut analyzer = crate::bundle::ColumnLineageAnalyzer::new();

        // Register base pack
        analyzer.register_table("__base_0".to_string(), "base".to_string());

        // Register joined packs
        for join_name in self.bundle.join_names() {
            analyzer.register_table(join_name.clone(), join_name.clone());
        }

        // Analyze the plan
        analyzer.analyze(plan).map_err(|e| {
            Box::new(std::io::Error::new(std::io::ErrorKind::Other, e)) as BundlebaseError
        })?;

        // Query for the specific column
        Ok(analyzer.get_source(logical_name))
    }

    /// Verify the integrity of all files in the bundle by checking SHA256 hashes.
    ///
    /// This method checks:
    /// - All data blocks: Verifies SHA256 hash matches the stored hash from operations
    /// - Index files: Verifies the files exist (no hash verification for indexes)
    ///
    /// # Arguments
    /// * `update_versions` - If true and hash matches but version changed, add UpdateVersionOp
    ///   to update stored version metadata
    ///
    /// # Returns
    /// `VerificationResults` with details for each file verified.
    pub async fn verify_data(
        &self,
        update_versions: bool,
    ) -> Result<super::VerificationResults, BundlebaseError> {
        use crate::bundle::command::VerifyDataCommand;

        let cmd = VerifyDataCommand::new(update_versions);
        Box::new(cmd).execute(self).await
    }
}

#[async_trait]
impl BundleFacade for BundleBuilder {
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
        self.bundle.version()
    }

    fn history(&self) -> Vec<commit::BundleCommit> {
        self.bundle.history()
    }

    fn operations(&self) -> Vec<AnyOperation> {
        let mut ops = self.bundle.operations.read().clone();
        ops.append(&mut self.status().operations().clone());
        ops
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

    fn extend(
        &self,
        data_dir: Option<&str>,
    ) -> Result<Arc<BundleBuilder>, BundlebaseError> {
        // Create a new builder based on the current bundle state without modifying self
        let current_bundle = Arc::new(self.bundle.deref().clone());
        BundleBuilder::extend(current_bundle, data_dir)
    }

    async fn query(
        &self,
        sql: &str,
        params: Vec<ScalarValue>,
    ) -> Result<SendableRecordBatchStream, BundlebaseError> {
        Ok(self.bundle().query(sql, params).await?)
    }

    /// Execute a SQL statement or command, returning streaming results.
    ///
    /// Unlike the default implementation in BundleFacade (which only handles read-only commands),
    /// BundleBuilder can execute ALL commands including mutating ones like ATTACH, FILTER, etc.
    async fn execute(
        &self,
        sql: &str,
        params: Vec<ScalarValue>,
    ) -> Result<SendableRecordBatchStream, BundlebaseError> {
        use super::command::parser::{is_command_statement, parse_command};
        use datafusion::physical_plan::stream::RecordBatchStreamAdapter;

        if is_command_statement(sql) {
            // Parse and execute as command (BundleBuilder can handle all commands)
            let cmd = parse_command(sql)?;
            let output = cmd.execute(self).await?;

            // Convert to stream using dyn methods
            let schema = output.dyn_schema();
            let batch = output.to_record_batch()?;
            let stream = futures::stream::iter(vec![Ok(batch)]);
            Ok(Box::pin(RecordBatchStreamAdapter::new(schema, stream)))
        } else {
            // Execute as regular SQL query
            self.query(sql, params).await
        }
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

    async fn explain(&self) -> Result<String, BundlebaseError> {
        self.bundle.explain().await
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

    fn data_dir(&self) -> Arc<dyn IOReadWriteDir> {
        self.bundle.data_dir()
    }

    fn config(&self) -> Arc<BundleConfig> {
        self.bundle.config()
    }

    fn ctx(&self) -> Arc<SessionContext> {
        self.bundle.ctx()
    }

    async fn execute_facade_command(
        &self,
        cmd: FacadeCommand,
    ) -> Result<Box<dyn CommandResponse>, BundlebaseError> {
        cmd.execute(self).await
    }

    async fn execute_command(
        &self,
        cmd: BundleCommand,
    ) -> Result<Box<dyn CommandResponse>, BundlebaseError> {
        // BundleBuilder can execute all commands
        cmd.execute(self).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::test_datafile;

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
    async fn test_schema_after_attach() {
        let bundle = BundleBuilder::create("memory:///test_bundle", None)
            .await
            .unwrap();
        bundle
            .attach(test_datafile("userdata.parquet"), None)
            .await
            .unwrap();

        let schema = bundle.bundle.schema().await.unwrap();
        assert!(
            !schema.fields().is_empty(),
            "After attach, schema should have fields"
        );
        assert_eq!(schema.fields().len(), 13, "userdata.parquet has 13 columns");

        // Verify specific column names exist
        let field_names: Vec<String> = schema.fields().iter().map(|f| f.name().clone()).collect();
        assert!(field_names.contains(&"id".to_string()));
        assert!(field_names.contains(&"first_name".to_string()));
        assert!(field_names.contains(&"email".to_string()));
    }

    #[tokio::test]
    async fn test_schema_after_drop_column() {
        let bundle = BundleBuilder::create("memory:///test_bundle", None)
            .await
            .unwrap();
        bundle
            .attach(test_datafile("userdata.parquet"), None)
            .await
            .unwrap();

        let schema_before = &bundle.bundle.schema().await.unwrap();
        assert_eq!(schema_before.fields().len(), 13);

        bundle.drop_column("title").await.unwrap();
        let schema_after = &bundle.bundle.schema().await.unwrap();
        assert_eq!(schema_after.fields().len(), 12);

        // Verify 'title' column is gone
        let field_names: Vec<String> = schema_after
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect();
        assert!(!field_names.contains(&"title".to_string()));
    }

    #[tokio::test]
    async fn test_set_and_get_name() {
        let bundle = BundleBuilder::create("memory:///test_bundle", None)
            .await
            .unwrap();
        assert_eq!(bundle.bundle.name.read().clone(), None, "Empty bundle should have no name");

        bundle.set_name("My Bundle").await.unwrap();
        let name = bundle.bundle.name.read().as_ref().unwrap().clone();
        assert_eq!(name, "My Bundle");
    }

    #[tokio::test]
    async fn test_set_and_get_description() {
        let bundle = BundleBuilder::create("memory:///test_bundle", None)
            .await
            .unwrap();
        assert_eq!(bundle.bundle.description.read().clone(), None);

        bundle
            .set_description("This is a test bundle")
            .await
            .unwrap();
        assert_eq!(
            bundle.bundle.description.read().clone().unwrap_or("NOT SET".to_string()),
            "This is a test bundle"
        );
    }

    #[tokio::test]
    async fn test_name_doesnt_affect_version() {
        let bundle = BundleBuilder::create("memory:///test_bundle", None)
            .await
            .unwrap();
        bundle
            .attach(test_datafile("userdata.parquet"), None)
            .await
            .unwrap();

        let v_no_name = bundle.bundle.version();

        bundle.set_name("Named Bundle").await.unwrap();
        let v_with_name = bundle.bundle.version();

        // Metadata operations now affect the version hash since they're proper operations
        assert_ne!(
            v_no_name, v_with_name,
            "Name should be tracked as an operation and change version"
        );
        // Verify the name was actually set
        assert_eq!(bundle.bundle.name(), Some("Named Bundle".to_string()));
    }

    #[tokio::test]
    async fn test_operations_list() {
        let bundle = BundleBuilder::create("memory:///test_bundle", None)
            .await
            .unwrap();
        assert_eq!(
            bundle.bundle.operations().len(),
            0,
        );

        bundle
            .attach(test_datafile("userdata.parquet"), None)
            .await
            .unwrap();
        assert_eq!(bundle.bundle.operations().len(), 1);

        bundle.drop_column("title").await.unwrap();
        assert_eq!(bundle.bundle.operations().len(), 2);
    }

    #[tokio::test]
    async fn test_version() {
        let bundle = BundleBuilder::create("memory:///test_bundle", None)
            .await
            .unwrap();

        let init_version = bundle.version();

        bundle
            .attach(test_datafile("userdata.parquet"), None)
            .await
            .unwrap();

        assert_ne!(init_version, bundle.version());
    }

    // NOTE: test_clone_independence was removed because BundleBuilder now uses interior
    // mutability (RwLock) and doesn't support cloning. The interior mutability design
    // allows using &self methods instead of &mut self, eliminating the need for cloning.

    #[tokio::test]
    async fn test_multiple_operations_pipeline() {
        let bundle = BundleBuilder::create("memory:///test_bundle", None)
            .await
            .unwrap();
        bundle
            .attach(test_datafile("userdata.parquet"), None)
            .await
            .unwrap();
        bundle.drop_column("title").await.unwrap();
        bundle
            .rename_column("first_name", "given_name")
            .await
            .unwrap();

        assert_eq!(bundle.bundle.operations.read().len(), 3);
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
