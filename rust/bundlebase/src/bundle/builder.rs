use crate::bundle::facade::BundleFacade;
use crate::bundle::init::InitCommit;
use crate::bundle::operation::SetNameOp;
use crate::bundle::operation::{AnyOperation, CreateSourceOp, SelectOp, SourceInfo};
use crate::bundle::operation::{
    AttachBlockOp, CreateFunctionOp, CreateJoinOp, CreateViewOp, DetachBlockOp, DropColumnOp,
    DropJoinOp, DropViewOp, FilterOp, RebuildIndexOp, RenameColumnOp, RenameJoinOp,
    RenameViewOp, ReplaceBlockOp, SetConfigOp, SetDescriptionOp,
};
use crate::bundle::operation::{BundleChange, IndexBlocksOp, Operation};
use crate::bundle::operation::{CreateIndexOp, DropIndexOp};
use crate::bundle::{commit, Pack, INIT_FILENAME, META_DIR};
use crate::bundle::{sql, Bundle, Source};
use super::DataBlock;
use crate::data::{ObjectId, VersionedBlockId};
use crate::source::{FetchAction, FetchResults};
use crate::functions::FunctionImpl;
use crate::functions::FunctionSignature;
use crate::index::IndexDefinition;
use crate::io::{writable_dir_from_str, writable_dir_from_url, write_yaml, IOReadWriteDir};
use crate::BundleConfig;
use crate::BundlebaseError;
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use chrono::DateTime;
use datafusion::prelude::DataFrame;
use datafusion::scalar::ScalarValue;
use log::{debug, info};
use sha2::{Digest, Sha256};
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

    fn clear(&mut self) {
        self.changes.clear();
    }

    pub fn pop(&mut self) {
        self.changes.pop();
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

/// A modifiable Bundle.
///
/// `BundleBuilder` represents a bundle during the development/transformation phase.
/// It tracks both operations that have been previously committed (via the `existing` base) and
/// new operations added since the working copy was created or extended.
///
/// # Key Characteristics
/// - **Mutation-in-Place**: Methods take `&mut self` and add operations to the chain
/// - **Builder Pattern**: Methods return `&mut Self` for convenient chaining
/// - **Commit**: Call `commit()` to persist all operations to disk
///
/// # Example
/// let bundle = BundleBuilder::create("memory://work", None).await?;
/// bundle.attach("data.parquet", None).await?
///     .filter("amount > 100").await?
///     .commit("Filter high-value transactions").await?;
/// ```
pub struct BundleBuilder {
    pub bundle: Bundle,
    status: BundleStatus,
    in_progress_change: Option<BundleChange>,
}

impl Clone for BundleBuilder {
    fn clone(&self) -> Self {
        Self {
            bundle: self.bundle.clone(),
            status: self.status.clone(),
            in_progress_change: self.in_progress_change.clone(),
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
    /// let bundle = BundleBuilder::create("memory://work", None).await?;
    /// bundle.attach("data.parquet", None).await?;
    /// ```
    pub async fn create(
        path: &str,
        config: Option<BundleConfig>,
    ) -> Result<BundleBuilder, BundlebaseError> {
        let mut existing = Bundle::empty().await?;
        existing.passed_config = config;
        existing.recompute_config()?;
        existing.data_dir = writable_dir_from_str(path, existing.config.clone())?;

        // Check if a bundle already exists at this location
        let meta_dir = existing.data_dir.writable_subdir(META_DIR)?;
        let init_file = meta_dir.file(INIT_FILENAME)?;
        if init_file.exists().await? {
            return Err(format!(
                "A bundle already exists at '{}'. Use open() to access an existing bundle.",
                path
            )
            .into());
        }

        let builder = BundleBuilder {
            status: BundleStatus::new(),
            bundle: existing,
            in_progress_change: None,
        };

        // Automatically create the base pack with a well-known ID
        builder.bundle.add_pack(ObjectId::BASE_PACK, Arc::new(Pack::new_base()));

        Ok(builder)
    }

    pub fn extend(bundle: Arc<Bundle>, data_dir: Option<&str>) -> Result<BundleBuilder, BundlebaseError> {
        let mut new_bundle = bundle.deref().clone();

        // If data_dir is provided and not empty, use it; otherwise keep the current bundle's data_dir
        if let Some(dir) = data_dir {
            if !dir.is_empty() {
                new_bundle.data_dir = writable_dir_from_str(dir, bundle.config())?;
                if new_bundle.data_dir.url() != bundle.url() {
                    new_bundle.last_manifest_version = 0;
                }
            }
        }

        Ok(BundleBuilder {
            bundle: new_bundle,
            status: BundleStatus::new(),
            in_progress_change: None,
        })
    }

    /// The bundle being built
    pub fn bundle(&self) -> &Bundle {
        &self.bundle
    }

    /// Returns the bundle status showing uncommitted changes.
    pub fn status(&self) -> &BundleStatus {
        &self.status
    }

    /// Commits all operations in the bundle to persistent storage.
    ///
    /// # Arguments
    /// * `message` - Human-readable description of the changes (e.g., "Filter to Q4 data")
    ///
    /// # Example
    /// bundle.attach("data.parquet", None).await?;
    /// bundle.filter("amount > 100").await?;
    /// bundle.commit("Filter high-value transactions").await?;
    /// ```
    pub async fn commit(&mut self, message: &str) -> Result<(), BundlebaseError> {
        let manifest_dir = self.bundle.data_dir.writable_subdir(META_DIR)?;

        if self.bundle.last_manifest_version == 0 {
            let from = self.bundle.from();
            let init_file = manifest_dir.writable_file(INIT_FILENAME)?;
            write_yaml(init_file.as_ref(), &InitCommit::new(from)).await?;
        };

        // Calculate next version number
        let next_version = self.bundle.last_manifest_version + 1;

        // Get current timestamp in UTC ISO format
        let now = std::time::SystemTime::now();
        let timestamp = to_iso(now);

        // Get author from environment or use default
        let author = std::env::var("BUNDLEBASE_AUTHOR")
            .unwrap_or_else(|_| std::env::var("USER").unwrap_or_else(|_| "unknown".to_string()));

        let changes = self.status.changes().clone();

        let commit_struct = commit::BundleCommit {
            url: None, //no need to set, we're just writing it and then will re-read it back
            data_dir: None,
            message: message.to_string(),
            author,
            timestamp,
            changes,
        };

        // Serialize directly using serde_yaml
        let yaml = serde_yaml::to_string(&commit_struct)?;

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
        let config = self.bundle.passed_config.clone();
        self.bundle = Bundle::open(self.url().as_str(), config).await?;
        // Clear status since the operations have been persisted
        self.status.clear();

        info!("Committed version {}", self.bundle.version());

        Ok(())
    }

    /// Resets all uncommitted operations, reverting to the last committed state.
    ///
    /// This method clears all pending operations and reloads the bundle from
    /// the last committed version. Any changes made since the last commit are discarded.
    ///
    /// # Example
    /// bundle.attach("data.parquet", None).await?;
    /// bundle.filter("amount > 100").await?;
    /// bundle.reset().await?;  // Discards attach and filter operations
    /// ```
    pub async fn reset(&mut self) -> Result<&mut Self, BundlebaseError> {
        if self.status.is_empty() {
            return Err("No uncommitted changes".into());
        }

        // Clear all uncommitted changes
        self.status.clear();

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
    /// bundle.attach("data.parquet", None).await?;
    /// bundle.filter("amount > 100").await?;
    /// bundle.undo().await?; // Discards only the filter change
    /// // Bundle now has only the attach change pending
    /// ```
    pub async fn undo(&mut self) -> Result<&mut Self, BundlebaseError> {
        if self.status.is_empty() {
            return Err("No uncommitted changes to undo".into());
        }

        // Remove the last change
        self.status.pop();

        self.reload_bundle().await?;

        // Reapply all remaining operations
        for change in &self.status.changes {
            for op in &change.operations {
                self.bundle.apply_operation(op.clone()).await?;
            }
        }

        info!("Last operation undone");

        Ok(self)
    }

    async fn reload_bundle(&mut self) -> Result<(), BundlebaseError> {
        // Reload the bundle from the last committed state
        let empty = self.bundle.commits.is_empty();
        self.bundle = if empty {
            let mut new = Bundle::empty().await?;
            new.passed_config = self.bundle.passed_config.clone();
            new.recompute_config()?;
            new.data_dir = writable_dir_from_url(self.url(), new.config.clone())?;
            new
        } else {
            // Preserve explicit_config when reopening
            let config = self.bundle.passed_config.clone();
            Bundle::open(self.url().as_str(), config).await?
        };
        Ok(())
    }

    async fn apply_operation(&mut self, op: AnyOperation) -> Result<(), BundlebaseError> {
        if self.bundle.is_view() && !op.allowed_on_view() {
            return Err(format!(
                "Operation '{}' is not allowed on a view",
                op.describe()
            )
            .into());
        }

        self.bundle.apply_operation(op.clone()).await?;

        self.in_progress_change
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
    async fn do_change<F>(&mut self, description: &str, f: F) -> Result<(), BundlebaseError>
    where
        F: for<'a> FnOnce(&'a mut Self) -> BoxFuture<'a, Result<(), BundlebaseError>>,
    {
        // Check for nested changes - track whether we created this change
        let is_nested = match &self.in_progress_change {
            Some(in_progress) => {
                debug!(
                    "Change {} already in progress, not going to separately track {}",
                    in_progress.description, description
                );
                true
            }
            None => {
                let change = BundleChange::new(description);
                self.in_progress_change = Some(change);
                false
            }
        };

        // Execute the closure
        let result = f(self).await;

        // Only finalize the change if we created it (not nested)
        match result {
            Ok(_) => {
                if !is_nested {
                    if let Some(change) = self.in_progress_change.take() {
                        self.status.changes.push(change);
                    }
                }
                Ok(())
            }
            Err(e) => {
                if !is_nested {
                    self.in_progress_change = None;
                }
                Err(e)
            }
        }
    }

    /// Attach a data block to the bundle.
    ///
    /// # Arguments
    /// * `path` - The location/URL of the data to attach
    /// * `pack` - The pack to attach to. Use `None` or `"base"` for the base pack,
    ///            or a join name to attach to that join's pack.
    pub async fn attach(
        &mut self,
        path: &str,
        pack: Option<&str>,
    ) -> Result<&mut Self, BundlebaseError> {
        let pack_id = match pack {
            None | Some("base") => ObjectId::BASE_PACK,
            Some(join_name) => *self
                .bundle
                .pack_by_name(join_name)
                .ok_or(format!("Unknown join '{}'", join_name))?
                .id(),
        };

        let path = path.to_string();
        let pack_name = pack.unwrap_or("base").to_string();

        self.do_change(&format!("Attach {} to {}", path, pack_name), |builder| {
            Box::pin(async move {
                builder
                    .apply_operation(
                        AttachBlockOp::setup(&pack_id, &path, builder).await?.into(),
                    )
                    .await?;

                info!("Attached {} to {}", path, pack_name);

                Ok(())
            })
        })
        .await?;

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
    /// bundle.detach_block("s3://bucket/data.parquet").await?;
    /// ```
    pub async fn detach_block(&mut self, location: &str) -> Result<&mut Self, BundlebaseError> {
        let location = location.to_string();

        self.do_change(&format!("Detach block at {}", location), |builder| {
            Box::pin(async move {
                let op = DetachBlockOp::setup(&location, &builder.bundle).await?;
                builder.apply_operation(op.into()).await?;

                info!("Detached block from {}", location);

                Ok(())
            })
        })
        .await?;

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
    /// bundle.replace_block(
    ///     "s3://old-bucket/data.parquet",
    ///     "s3://new-bucket/data.parquet"
    /// ).await?;
    /// ```
    pub async fn replace_block(
        &mut self,
        old_location: &str,
        new_location: &str,
    ) -> Result<&mut Self, BundlebaseError> {
        let old_location = old_location.to_string();
        let new_location = new_location.to_string();

        self.do_change(
            &format!("Replace block {} -> {}", old_location, new_location),
            |builder| {
                Box::pin(async move {
                    let op =
                        ReplaceBlockOp::setup(&old_location, &new_location, builder).await?;
                    builder.apply_operation(op.into()).await?;

                    info!("Replaced block {} -> {}", old_location, new_location);

                    Ok(())
                })
            },
        )
        .await?;

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
    /// ```no_run
    /// # use bundlebase::{BundleBuilder, BundlebaseError};
    /// # use std::collections::HashMap;
    /// # async fn example() -> Result<(), BundlebaseError> {
    /// let mut bundle = BundleBuilder::create("memory:///work", None).await?;
    /// let mut args = HashMap::new();
    /// args.insert("url".to_string(), "s3://bucket/data/".to_string());
    /// args.insert("patterns".to_string(), "**/*.parquet".to_string());
    /// bundle.create_source("remote_dir", args, None).await?;
    /// bundle.fetch(None).await?;  // Fetch from base pack sources
    /// bundle.commit("Initial data from source").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn create_source(
        &mut self,
        function: &str,
        args: HashMap<String, String>,
        pack: Option<&str>,
    ) -> Result<&mut Self, BundlebaseError> {
        let pack = pack.map(|s| s.to_string());
        let function = function.to_string();
        let url = args.get("url").cloned().unwrap_or_else(|| "<no url>".to_string());
        let pack_name = pack.clone().unwrap_or_else(|| "base".to_string());

        self.do_change(
            &format!("Create source for {} at {}", pack_name, url),
            |builder| {
                let pack_name = pack_name.clone();
                Box::pin(async move {
                    let pack_id = match pack.as_deref() {
                        None | Some("base") => ObjectId::BASE_PACK,
                        Some(join_name) => *builder
                            .bundle
                            .pack_by_name(join_name)
                            .ok_or(format!("Unknown join '{}'", join_name))?
                            .id(),
                    };

                    let source_id = ObjectId::generate();
                    let op = CreateSourceOp::setup(source_id, pack_id, function, args);

                    builder.apply_operation(op.into()).await?;

                    // Automatically fetch from the newly created source
                    // This runs inside the same change context
                    let source = builder
                        .bundle
                        .get_source(&source_id)
                        .ok_or_else(|| format!("Source '{}' not found after creation", source_id))?;
                    let _ = builder.fetch_source(&source, &pack_name).await?;

                    Ok(())
                })
            },
        )
        .await?;

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
    /// ```no_run
    /// # use bundlebase::{BundleBuilder, BundlebaseError};
    /// # use std::collections::HashMap;
    /// # async fn example() -> Result<(), BundlebaseError> {
    /// let mut bundle = BundleBuilder::create("memory:///work", None).await?;
    /// let mut args = HashMap::new();
    /// args.insert("url".to_string(), "s3://bucket/data/".to_string());
    /// args.insert("patterns".to_string(), "**/*.parquet".to_string());
    /// bundle.create_source("remote_dir", args, None).await?;
    /// let results = bundle.fetch(None).await?;  // Fetch from base pack sources
    /// for result in &results {
    ///     println!("Source {}: {} added", result.source_function, result.added.len());
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn fetch(&mut self, pack: Option<&str>) -> Result<Vec<FetchResults>, BundlebaseError> {
        let pack_name = pack.unwrap_or("base").to_string();
        let pack_id = match pack {
            None | Some("base") => ObjectId::BASE_PACK,
            Some(join_name) => *self
                .bundle
                .pack_by_name(join_name)
                .ok_or(format!("Unknown join '{}'", join_name))?
                .id(),
        };

        let sources = self.bundle.get_sources_for_pack(&pack_id);
        if sources.is_empty() {
            return Err(format!("No sources defined for pack '{}'", pack.unwrap_or("base")).into());
        }

        let mut results = Vec::new();
        for source in sources {
            let result = self.fetch_source(&source, &pack_name).await?;
            results.push(result);
        }

        Ok(results)
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
    /// ```no_run
    /// # use bundlebase::{BundleBuilder, BundlebaseError};
    /// # use std::collections::HashMap;
    /// # async fn example() -> Result<(), BundlebaseError> {
    /// let mut bundle = BundleBuilder::create("memory:///work", None).await?;
    /// // Create multiple sources...
    /// let results = bundle.fetch_all().await?;
    /// for result in &results {
    ///     println!("Source {}: {} added, {} replaced, {} removed",
    ///         result.source_function,
    ///         result.added.len(),
    ///         result.replaced.len(),
    ///         result.removed.len());
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn fetch_all(&mut self) -> Result<Vec<FetchResults>, BundlebaseError> {
        let mut results = Vec::new();

        // Collect sources with their pack names to avoid borrow issues
        let sources_with_packs: Vec<_> = self
            .bundle
            .sources()
            .values()
            .map(|source| {
                let pack_name = self.bundle.pack_name(source.pack()).unwrap_or("base".to_string());
                (source.clone(), pack_name)
            })
            .collect();

        for (source, pack_name) in sources_with_packs {
            let result = self.fetch_source(&source, &pack_name).await?;
            results.push(result);
        }

        Ok(results)
    }

    /// Internal helper to fetch from a single source.
    async fn fetch_source(
        &mut self,
        source: &Arc<Source>,
        pack_name: &str,
    ) -> Result<FetchResults, BundlebaseError> {
        let registry = self.bundle.source_function_registry();
        let pack_id = *source.pack();
        let source_id = *source.id();
        let source_function = source.function().to_string();
        let source_url = source.args().get("url").cloned().unwrap_or_default();

        // Get fetch actions from the source function
        let actions = source
            .fetch(
                self.data_dir(),
                self.bundle.config(),
                &registry,
            )
            .await?;

        // Process actions and collect them for the result
        let mut processed_actions = Vec::new();

        for action in actions {
            match &action {
                FetchAction::Add(data) => {
                    let attach_location = data.attach_location.clone();
                    let source_location = data.source_location.clone();
                    let source_url_for_op = data.source_url.clone();
                    let hash = data.hash.clone();

                    self.do_change(
                        &format!("Fetch: attach {}", source_location),
                        |builder| {
                            let attach_location = attach_location.clone();
                            let source_location = source_location.clone();
                            let source_url_for_op = source_url_for_op.clone();
                            let hash = hash.clone();

                            Box::pin(async move {
                                // Use setup_for_source to read version from source_url
                                let mut op = AttachBlockOp::setup_for_source(
                                    &pack_id,
                                    &attach_location,
                                    &source_url_for_op,
                                    &hash,
                                    builder,
                                )
                                .await?;
                                // Create SourceInfo with the source version from the operation
                                op.source_info = Some(SourceInfo {
                                    id: source_id,
                                    location: source_location,
                                    version: op.version.clone(),
                                });
                                builder.apply_operation(op.into()).await?;
                                Ok(())
                            })
                        },
                    )
                    .await?;
                }
                FetchAction::Replace {
                    old_source_location,
                    data,
                } => {
                    // Find the old block's location and detach it
                    let old_location = self.find_block_location_by_source(
                        &source_id,
                        old_source_location,
                    )?;
                    self.detach_block(&old_location).await?;

                    // Attach the new block
                    let attach_location = data.attach_location.clone();
                    let source_location = data.source_location.clone();
                    let source_url_for_op = data.source_url.clone();
                    let hash = data.hash.clone();

                    self.do_change(
                        &format!("Fetch: replace {}", source_location),
                        |builder| {
                            let attach_location = attach_location.clone();
                            let source_location = source_location.clone();
                            let source_url_for_op = source_url_for_op.clone();
                            let hash = hash.clone();

                            Box::pin(async move {
                                let mut op = AttachBlockOp::setup_for_source(
                                    &pack_id,
                                    &attach_location,
                                    &source_url_for_op,
                                    &hash,
                                    builder,
                                )
                                .await?;
                                // Create SourceInfo with the source version from the operation
                                op.source_info = Some(SourceInfo {
                                    id: source_id,
                                    location: source_location,
                                    version: op.version.clone(),
                                });
                                builder.apply_operation(op.into()).await?;
                                Ok(())
                            })
                        },
                    )
                    .await?;
                }
                FetchAction::Remove { source_location } => {
                    // Find the block's location and detach it
                    let location =
                        self.find_block_location_by_source(&source_id, source_location)?;
                    self.detach_block(&location).await?;
                }
            }
            processed_actions.push(action);
        }

        Ok(FetchResults::from_actions(
            source_function,
            source_url,
            pack_name.to_string(),
            processed_actions,
        ))
    }

    /// Find the current location of a block that was attached from a source with the given source_location.
    ///
    /// This searches through both AttachBlockOp and ReplaceBlockOp operations to find the
    /// current location of a block. If the block was replaced, returns the new location.
    fn find_block_location_by_source(
        &self,
        source_id: &ObjectId,
        source_location: &str,
    ) -> Result<String, BundlebaseError> {
        // First, check ReplaceBlockOp operations (in reverse order to get most recent)
        // to see if the block was replaced and has updated source_info
        for op in self.bundle.operations.iter().rev() {
            if let AnyOperation::ReplaceBlock(replace) = op {
                if let Some(ref info) = replace.source_info {
                    if &info.id == source_id && info.location == source_location {
                        return Ok(replace.new_location.clone());
                    }
                }
            }
        }

        // If not found in ReplaceBlockOp, check AttachBlockOp
        self.bundle
            .operations
            .iter()
            .find_map(|op| {
                if let AnyOperation::AttachBlock(attach) = op {
                    if let Some(ref info) = attach.source_info {
                        if &info.id == source_id && info.location == source_location {
                            return Some(attach.location.clone());
                        }
                    }
                }
                None
            })
            .ok_or_else(|| {
                format!(
                    "No block found for source_location '{}'",
                    source_location
                )
                .into()
            })
    }

    /// Attach a view from another BundleBuilder
    ///
    /// Creates a named view that captures all uncommitted operations from the source BundleBuilder.
    /// The view is stored in a subdirectory under view_{id}/ and automatically inherits
    /// changes from the parent bundle through the FROM mechanism.
    ///
    /// # Arguments
    /// * `name` - Name of the view
    /// * `source` - BundleBuilder containing the operations to capture (typically from a select())
    ///
    /// # Example
    /// ```no_run
    /// # use bundlebase::{BundleBuilder, BundlebaseError, BundleFacade};
    /// # async fn example() -> Result<(), BundlebaseError> {
    /// let mut c = BundleBuilder::create("memory:///container", None).await?;
    /// c.attach("data.csv", None).await?;
    /// c.commit("Initial").await?;
    ///
    /// let adults = c.select("select * where age > 21", vec![]).await?;
    /// c.create_view("adults", &adults).await?;
    /// c.commit("Add adults view").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn create_view(
        &mut self,
        name: &str,
        source: &BundleBuilder,
    ) -> Result<&mut Self, BundlebaseError> {
        let name = name.to_string();

        // Check if source has uncommitted operations that will be captured for the view
        let source_ops_count = source.status().operations().len();
        let changes_before = self.status.changes.len();

        // Detect if source and self share the same underlying bundle by comparing bundle IDs
        // This is important for the Python case where source and self share the same Arc<Mutex<BundleBuilder>>
        let source_is_self = self.bundle.id() == source.bundle.id();

        // Clone source to avoid lifetime issues in async move
        let source_clone = source.clone();

        self.do_change(&format!("Create view '{}'", name), |builder| {
            Box::pin(async move {
                let op = CreateViewOp::setup(&name, &source_clone, builder).await?;
                builder.apply_operation(op.into()).await?;
                info!("Attached view '{}'", name);
                Ok(())
            })
        })
        .await?;

        // After creating view, if source had uncommitted operations and source is the same
        // as self, we need to remove those operations to prevent double-commit.
        if source_is_self && source_ops_count > 0 && changes_before >= source_ops_count {
            // Source and self share the same bundle - the source operations are in self's status
            // Remove the captured operations (keep only changes before source ops + CreateViewOp)
            let create_view_change = self.status.changes.pop(); // Remove CreateViewOp
            let keep_count = changes_before - source_ops_count; // Changes before source operations
            self.status.changes.truncate(keep_count); // Remove source ops
            if let Some(create_view_change) = create_view_change {
                self.status.changes.push(create_view_change); // Add back CreateViewOp
            }

            debug!(
                "Removed {} changes that were captured for view (prevents double-commit)",
                source_ops_count
            );
        }

        Ok(self)
    }

    /// Rename an existing view
    ///
    /// # Arguments
    /// * `old_name` - The current name of the view
    /// * `new_name` - The new name for the view
    ///
    /// # Example
    /// ```no_run
    /// # use bundlebase::{BundleBuilder, BundlebaseError, BundleFacade};
    /// # async fn example() -> Result<(), BundlebaseError> {
    /// # let mut c = BundleBuilder::create("memory:///example", None).await?;
    /// # c.attach("data.csv", None).await?;
    /// let adults = c.select("select * from bundle where age > 21", vec![]).await?;
    /// c.create_view("adults", &adults).await?;
    /// c.rename_view("adults", "adults_view").await?;
    /// c.commit("Renamed view").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn rename_view(
        &mut self,
        old_name: &str,
        new_name: &str,
    ) -> Result<&mut Self, BundlebaseError> {
        let old_name = old_name.to_string();
        let new_name = new_name.to_string();

        self.do_change(
            &format!("Rename view '{}' to '{}'", old_name, new_name),
            |builder| {
                Box::pin(async move {
                    // Call setup() with bundle reference to look up view_id
                    let op =
                        RenameViewOp::setup(&old_name, &new_name, &builder.bundle).await?;
                    builder.apply_operation(op.into()).await?;
                    Ok(())
                })
            },
        )
        .await?;

        Ok(self)
    }

    /// Drop an existing view
    ///
    /// # Arguments
    /// * `view_name` - The name of the view to drop
    ///
    /// # Example
    /// ```no_run
    /// # use bundlebase::{BundleBuilder, BundlebaseError, BundleFacade};
    /// # async fn example() -> Result<(), BundlebaseError> {
    /// # let mut c = BundleBuilder::create("memory:///example", None).await?;
    /// # c.attach("data.csv", None).await?;
    /// let adults = c.select("select * from bundle where age > 21", vec![]).await?;
    /// c.create_view("adults", &adults).await?;
    /// c.drop_view("adults").await?;
    /// c.commit("Dropped view").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn drop_view(
        &mut self,
        view_name: &str,
    ) -> Result<&mut Self, BundlebaseError> {
        let view_name = view_name.to_string();

        self.do_change(
            &format!("Drop view '{}'", view_name),
            |builder| {
                Box::pin(async move {
                    // Call setup() with bundle reference to look up view_id
                    let op = DropViewOp::setup(&view_name, &builder.bundle).await?;
                    builder.apply_operation(op.into()).await?;
                    Ok(())
                })
            },
        )
        .await?;

        Ok(self)
    }

    /// Drop an existing join
    ///
    /// # Arguments
    /// * `join_name` - The name of the join to drop
    ///
    /// # Example
    /// ```no_run
    /// # use bundlebase::{BundleBuilder, BundlebaseError, BundleFacade, JoinTypeOption};
    /// # async fn example() -> Result<(), BundlebaseError> {
    /// # let mut c = BundleBuilder::create("memory:///example", None).await?;
    /// # c.attach("data.csv", None).await?;
    /// c.join("customers", "base.customer_id = customers.id", Some("customers.parquet"), JoinTypeOption::Left).await?;
    /// c.drop_join("customers").await?;
    /// c.commit("Dropped join").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn drop_join(&mut self, join_name: &str) -> Result<&mut Self, BundlebaseError> {
        let join_name = join_name.to_string();

        self.do_change(&format!("Drop join '{}'", join_name), |builder| {
            Box::pin(async move {
                let op = DropJoinOp::setup(&join_name, &builder.bundle).await?;
                builder.apply_operation(op.into()).await?;
                Ok(())
            })
        })
        .await?;

        Ok(self)
    }

    /// Rename an existing join
    ///
    /// # Arguments
    /// * `old_name` - The current name of the join
    /// * `new_name` - The new name for the join
    ///
    /// # Example
    /// ```no_run
    /// # use bundlebase::{BundleBuilder, BundlebaseError, BundleFacade, JoinTypeOption};
    /// # async fn example() -> Result<(), BundlebaseError> {
    /// # let mut c = BundleBuilder::create("memory:///example", None).await?;
    /// # c.attach("data.csv", None).await?;
    /// c.join("customers", "base.customer_id = customers.id", Some("customers.parquet"), JoinTypeOption::Left).await?;
    /// c.rename_join("customers", "clients").await?;
    /// c.commit("Renamed join").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn rename_join(
        &mut self,
        old_name: &str,
        new_name: &str,
    ) -> Result<&mut Self, BundlebaseError> {
        let old_name = old_name.to_string();
        let new_name = new_name.to_string();

        self.do_change(
            &format!("Rename join '{}' to '{}'", old_name, new_name),
            |builder| {
                Box::pin(async move {
                    let op =
                        RenameJoinOp::setup(&old_name, &new_name, &builder.bundle).await?;
                    builder.apply_operation(op.into()).await?;
                    Ok(())
                })
            },
        )
        .await?;

        Ok(self)
    }

    /// Drop a column (mutates self)
    pub async fn drop_column(&mut self, name: &str) -> Result<&mut Self, BundlebaseError> {
        let name = name.to_string();

        self.do_change(&format!("Drop column {}", name), |builder| {
            Box::pin(async move {
                builder
                    .apply_operation(DropColumnOp::setup(vec![name.as_str()]).into())
                    .await?;

                info!("Dropped column \"{}\"", name);

                Ok(())
            })
        })
        .await?;

        Ok(self)
    }

    /// Rename a column (mutates self)
    pub async fn rename_column(
        &mut self,
        old_name: &str,
        new_name: &str,
    ) -> Result<&mut Self, BundlebaseError> {
        debug!("Staring rename column {} to {}", old_name, new_name);

        let old_name = old_name.to_string();
        let new_name = new_name.to_string();

        self.do_change(
            &format!("Rename column '{}' to '{}'", old_name, new_name),
            |builder| {
                Box::pin(async move {
                    builder
                        .apply_operation(RenameColumnOp::setup(&old_name, &new_name).into())
                        .await?;
                    info!("Renamed \"{}\" to \"{}\"", old_name, new_name);
                    Ok(())
                })
            },
        )
        .await?;

        Ok(self)
    }

    /// Filter rows with a WHERE clause (mutates self)
    /// Parameters can be referenced as $1, $2, etc. in the WHERE clause.
    pub async fn filter(
        &mut self,
        where_clause: &str,
        params: Vec<ScalarValue>,
    ) -> Result<&mut Self, BundlebaseError> {
        let where_clause = where_clause.to_string();

        self.do_change(&format!("Filter: {}", where_clause), |builder| {
            Box::pin(async move {
                builder
                    .apply_operation(FilterOp::setup(&where_clause, params).await?.into())
                    .await?;
                info!("Filtered by {}", where_clause);
                Ok(())
            })
        })
        .await?;

        Ok(self)
    }

    /// Join with another data source (mutates self)
    ///
    /// If `location` is None, the join point is created without any initial data.
    /// Data can be attached later using `attach()` or `create_source()` with the `pack` parameter.
    pub async fn join(
        &mut self,
        name: &str,
        expression: &str,
        location: Option<&str>,
        join_type: JoinTypeOption,
    ) -> Result<&mut Self, BundlebaseError> {
        let name = name.to_string();
        let location = location.map(|s| s.to_string());
        let expression = expression.to_string();

        self.do_change(&format!("Join '{}' on {}", name, expression), |builder| {
            Box::pin(async move {
                // Step 1: Create a new pack with join metadata
                let join_pack_id = ObjectId::generate();
                builder
                    .apply_operation(
                        CreateJoinOp::setup(&join_pack_id, &name, &expression, join_type)
                            .await?
                            .into(),
                    )
                    .await?;

                // Step 2: Attach the location data to the join pack (if provided)
                if let Some(loc) = &location {
                    builder
                        .apply_operation(
                            AttachBlockOp::setup(&join_pack_id, loc, builder)
                                .await?
                                .into(),
                        )
                        .await?;
                }

                match &location {
                    Some(loc) => info!("Joined: {} as \"{}\"", loc, name),
                    None => info!("Created join point \"{}\" (no initial data)", name),
                }

                Ok(())
            })
        })
        .await?;

        Ok(self)
    }

    /// Create a custom function (mutates self)
    pub async fn create_function(
        &mut self,
        signature: FunctionSignature,
    ) -> Result<&mut Self, BundlebaseError> {
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

    /// Set the implementation for a function (mutates self)
    pub async fn set_impl(
        &mut self,
        name: &str,
        def: Arc<dyn FunctionImpl>,
    ) -> Result<&mut Self, BundlebaseError> {
        self.bundle.function_registry.write().set_impl(name, def)?;
        Ok(self)
    }

    /// Set the bundle's name (mutates self)
    pub async fn set_name(&mut self, name: &str) -> Result<&mut Self, BundlebaseError> {
        let name = name.to_string();

        self.do_change(&format!("Set name to {}", name), |builder| {
            Box::pin(async move {
                builder
                    .apply_operation(SetNameOp::setup(&name).into())
                    .await?;
                Ok(())
            })
        })
        .await?;

        Ok(self)
    }

    /// Set the bundle's description (mutates self)
    pub async fn set_description(
        &mut self,
        description: &str,
    ) -> Result<&mut Self, BundlebaseError> {
        let description = description.to_string();

        self.do_change(&format!("Set description to {}", description), |builder| {
            Box::pin(async move {
                builder
                    .apply_operation(SetDescriptionOp::setup(&description).into())
                    .await?;
                Ok(())
            })
        })
        .await?;

        Ok(self)
    }

    /// Set a configuration value (mutates self)
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
        &mut self,
        key: &str,
        value: &str,
        url_prefix: Option<&str>,
    ) -> Result<&mut Self, BundlebaseError> {
        let key = key.to_string();
        let value = value.to_string();
        let url_prefix_owned = url_prefix.map(|s| s.to_string());

        let description = match &url_prefix_owned {
            Some(prefix) => format!("Set config [{}]: {}", prefix, key),
            None => format!("Set config: {}", key),
        };

        self.do_change(&description, |builder| {
            Box::pin(async move {
                builder
                    .apply_operation(
                        SetConfigOp::setup(&key, &value, url_prefix_owned.as_deref()).into(),
                    )
                    .await?;
                Ok(())
            })
        })
        .await?;

        Ok(self)
    }

    /// Create an index on a column
    pub async fn index(&mut self, column: &str) -> Result<&mut Self, BundlebaseError> {
        let column = column.to_string();

        self.do_change(&format!("Index column {}", column), |builder| {
            Box::pin(async move {
                builder
                    .apply_operation(CreateIndexOp::setup(&column).await?.into())
                    .await?;

                builder.reindex().await?;

                info!("Created index on: \"{}\"", column);

                Ok(())
            })
        })
        .await?;

        Ok(self)
    }

    /// Drop an index on a column
    pub async fn drop_index(&mut self, column: &str) -> Result<&mut Self, BundlebaseError> {
        let column = column.to_string();

        self.do_change(&format!("Drop index on column {}", column), |builder| {
            Box::pin(async move {
                // Find the index ID for the given column
                let index_id = {
                    let indexes = builder.bundle.indexes().read();
                    let index = indexes.iter().find(|idx| idx.column() == column.as_str());

                    match index {
                        Some(idx) => *idx.id(),
                        None => {
                            return Err(format!("No index found for column '{}'", column).into());
                        }
                    }
                };

                builder
                    .apply_operation(DropIndexOp::setup(&index_id).await?.into())
                    .await?;

                info!("Dropped index on: \"{}\"", column);

                Ok(())
            })
        })
        .await?;

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
    /// - `Ok(&mut Self)` - Successfully processed all indexes
    /// - `Err(BundlebaseError)` - If a critical operation fails (e.g., block not found during setup)
    ///
    /// # Note
    /// This is typically called automatically by `index()` method after defining a new index.
    /// Manual calls are useful when recovering from partial index creation failures.
    pub async fn reindex(&mut self) -> Result<&mut Self, BundlebaseError> {
        debug!("Starting reindex");

        self.do_change("Reindex", |builder| {
            Box::pin(async move {
                // Group blocks by (index_id, column_name) for batching
                let mut blocks_to_index: HashMap<(ObjectId, String), Vec<(ObjectId, String)>> =
                    HashMap::new();

                // Ensure dataframe is set up for queries
                let df = builder.dataframe().await?;

                // Collect index definitions before the loop to avoid holding the lock across awaits
                let index_defs: Vec<Arc<IndexDefinition>> =
                    builder.bundle.indexes.read().iter().cloned().collect();

                for index_def in &index_defs {
                    let logical_col = index_def.column().to_string();
                    let index_id = index_def.id();
                    debug!("Checking index on {}", &logical_col);

                    // Pass packs to expand pack tables into block tables
                    let sources = match sql::column_sources_from_df(
                        logical_col.as_str(),
                        &df,
                        Some(builder.bundle.packs()),
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
                        let block_version = builder
                            .find_block_version(&block_id)
                            .ok_or_else(|| format!("Block {} not found in packs", block_id))?;
                        debug!(
                            "Physical source: block {} version {}",
                            &block_id, &block_version
                        );

                        // Check if index already exists at this version
                        let versioned_block =
                            VersionedBlockId::new(block_id, block_version.clone());
                        let needs_index = builder
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

                        builder
                            .apply_operation(
                                IndexBlocksOp::setup(&index_id, &column, blocks, &builder.bundle)
                                    .await?
                                    .into(),
                            )
                            .await?;
                    }
                }

                info!("Reindexed all columns");

                Ok(())
            })
        })
        .await?;

        Ok(self)
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

    /// Rebuild an index on a column (mutates self)
    pub async fn rebuild_index(&mut self, column: &str) -> Result<&mut Self, BundlebaseError> {
        let column = column.to_string();

        self.do_change(&format!("Rebuild index on column {}", column), |builder| {
            Box::pin(async move {
                builder
                    .apply_operation(RebuildIndexOp::setup(column).await?.into())
                    .await?;
                Ok(())
            })
        })
        .await?;

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

    pub fn data_dir(&self) -> &dyn IOReadWriteDir {
        self.bundle.data_dir()
    }
}

#[async_trait]
impl BundleFacade for BundleBuilder {
    fn id(&self) -> &str {
        self.bundle.id()
    }

    fn name(&self) -> Option<&str> {
        self.bundle.name()
    }

    fn description(&self) -> Option<&str> {
        self.bundle.description()
    }

    fn url(&self) -> &Url {
        self.bundle.url()
    }

    fn from(&self) -> Option<&Url> {
        self.bundle.from()
    }

    fn version(&self) -> String {
        self.bundle.version()
    }

    fn history(&self) -> Vec<commit::BundleCommit> {
        self.bundle.history()
    }

    fn operations(&self) -> Vec<AnyOperation> {
        let mut ops = self.bundle.operations.clone();
        ops.append(&mut self.status.operations().clone());

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

    async fn select(&self, sql: &str, params: Vec<ScalarValue>) -> Result<Self, BundlebaseError> {
        let mut bundle = self.clone();
        let sql = sql.to_string();
        let sql = if !sql.to_lowercase().starts_with("select ") {
            format!("SELECT {}", sql)
        } else {
            sql
        };

        bundle
            .do_change(&format!("Query: {}", sql), |builder| {
                Box::pin(async move {
                    builder
                        .apply_operation(SelectOp::setup(sql, params).await?.into())
                        .await?;
                    info!("Created query");
                    Ok(())
                })
            })
            .await?;

        Ok(bundle)
    }

    fn views(&self) -> HashMap<ObjectId, String> {
        self.bundle.views()
    }

    async fn view(&self, identifier: &str) -> Result<Bundle, BundlebaseError> {
        self.bundle.view(identifier).await
    }

    async fn export_tar(&self, tar_path: &str) -> Result<String, BundlebaseError> {
        // Check for uncommitted changes
        if !self.status().is_empty() {
            return Err("Cannot export tar with uncommitted changes. Please commit first.".into());
        }

        // Delegate to the Bundle's implementation via BundleFacade
        self.bundle.export_tar(tar_path).await
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
        assert!(
            schema.fields().is_empty(),
            "Empty bundle should have empty schema"
        );
    }

    #[tokio::test]
    async fn test_schema_after_attach() {
        let mut bundle = BundleBuilder::create("memory:///test_bundle", None)
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
        let mut bundle = BundleBuilder::create("memory:///test_bundle", None)
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
        let mut bundle = BundleBuilder::create("memory:///test_bundle", None)
            .await
            .unwrap();
        assert_eq!(bundle.bundle.name, None, "Empty bundle should have no name");

        let bundle = bundle.set_name("My Bundle").await.unwrap();
        let name = bundle.bundle.name.as_ref().unwrap();
        assert_eq!(name, "My Bundle");
    }

    #[tokio::test]
    async fn test_set_and_get_description() {
        let mut bundle = BundleBuilder::create("memory:///test_bundle", None)
            .await
            .unwrap();
        assert_eq!(bundle.bundle.description, None);

        bundle
            .set_description("This is a test bundle")
            .await
            .unwrap();
        assert_eq!(
            bundle.bundle.description.unwrap_or("NOT SET".to_string()),
            "This is a test bundle"
        );
    }

    #[tokio::test]
    async fn test_name_doesnt_affect_version() {
        let mut bundle = BundleBuilder::create("memory:///test_bundle", None)
            .await
            .unwrap();
        bundle
            .attach(test_datafile("userdata.parquet"), None)
            .await
            .unwrap();

        let v_no_name = bundle.bundle.version();

        let bundle_with_name = bundle.set_name("Named Bundle").await.unwrap();
        let v_with_name = bundle_with_name.bundle.version();

        // Metadata operations now affect the version hash since they're proper operations
        assert_ne!(
            v_no_name, v_with_name,
            "Name should be tracked as an operation and change version"
        );
        // Verify the name was actually set
        assert_eq!(bundle_with_name.bundle.name(), Some("Named Bundle"));
    }

    #[tokio::test]
    async fn test_operations_list() {
        let mut bundle = BundleBuilder::create("memory:///test_bundle", None)
            .await
            .unwrap();
        assert_eq!(
            bundle.bundle.operations().len(),
            0,
        );

        let bundle = bundle
            .attach(test_datafile("userdata.parquet"), None)
            .await
            .unwrap();
        assert_eq!(bundle.bundle.operations().len(), 1);

        bundle.drop_column("title").await.unwrap();
        assert_eq!(bundle.bundle.operations().len(), 2);
    }

    #[tokio::test]
    async fn test_version() {
        let mut bundle = BundleBuilder::create("memory:///test_bundle", None)
            .await
            .unwrap();

        let init_version = bundle.version();

        bundle
            .attach(test_datafile("userdata.parquet"), None)
            .await
            .unwrap();

        assert_ne!(init_version, bundle.version());
    }

    #[tokio::test]
    async fn test_clone_independence() {
        let mut bundle = BundleBuilder::create("memory:///test_bundle", None)
            .await
            .unwrap();
        bundle
            .attach(test_datafile("userdata.parquet"), None)
            .await
            .unwrap();

        let v1 = bundle.version();

        // Clone and add operation to clone
        let mut bundle_clone = bundle.clone();
        bundle_clone.drop_column("title").await.unwrap();
        let v2 = bundle_clone.version();

        // Original should be unchanged
        assert_eq!(bundle.bundle.operations().len(), 1);
        assert_eq!(bundle_clone.bundle.operations().len(), 2);
        assert_ne!(
            v1, v2,
            "Different operations should have different versions"
        );

        // Test that packs are independent
        let orig_packs_count = bundle.bundle.packs().read().len();
        let clone_packs_count = bundle_clone.bundle.packs().read().len();
        assert_eq!(orig_packs_count, clone_packs_count);

        // Test that indexes are independent
        let orig_indexes_count = bundle.bundle.indexes.read().len();
        let clone_indexes_count = bundle_clone.bundle.indexes.read().len();
        assert_eq!(orig_indexes_count, clone_indexes_count);

        // Now add an index to the clone
        bundle_clone.index("id").await.unwrap();

        // Original should still have 0 indexes, clone should have 1
        let orig_indexes_after = bundle.bundle.indexes.read().len();
        let clone_indexes_after = bundle_clone.bundle.indexes.read().len();
        assert_eq!(
            0, orig_indexes_after,
            "Original should have 0 indexes after clone modifies"
        );
        assert_eq!(1, clone_indexes_after, "Clone should have 1 index");
    }

    #[tokio::test]
    async fn test_multiple_operations_pipeline() {
        let mut bundle = BundleBuilder::create("memory:///test_bundle", None)
            .await
            .unwrap();
        bundle
            .attach(test_datafile("userdata.parquet"), None)
            .await
            .unwrap();
        bundle.drop_column("title").await.unwrap();
        let bundle = bundle
            .rename_column("first_name", "given_name")
            .await
            .unwrap();

        assert_eq!(bundle.bundle.operations.len(), 3);
    }

    #[tokio::test]
    async fn test_create_fails_if_bundle_exists() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let path = tmp_dir.path().to_str().unwrap();

        // Create and commit a bundle
        let mut bundle = BundleBuilder::create(path, None).await.unwrap();
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
