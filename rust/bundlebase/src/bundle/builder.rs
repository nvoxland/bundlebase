use crate::bundle::facade::BundleFacade;
use crate::bundle::init::InitCommit;
use crate::bundle::operation::SetNameOp;
use crate::bundle::operation::{AnyOperation, SelectOp};
use crate::bundle::operation::{
    AttachBlockOp, CreateViewOp, DefineFunctionOp, DefinePackOp, DropViewOp, FilterOp, JoinOp,
    RebuildIndexOp, RemoveColumnsOp, RenameColumnOp, RenameViewOp, SetConfigOp, SetDescriptionOp,
};
use crate::bundle::operation::{BundleChange, IndexBlocksOp, Operation};
use crate::bundle::operation::{DefineIndexOp, DropIndexOp, JoinTypeOption};
use crate::bundle::{commit, INIT_FILENAME, META_DIR};
use crate::bundle::{sql, Bundle};
use crate::data::{DataBlock, ObjectId, VersionedBlockId};
use crate::functions::FunctionImpl;
use crate::functions::FunctionSignature;
use crate::index::IndexDefinition;
use crate::io::ObjectStoreDir;
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

/// Format a system time as ISO8601 UTC string (e.g., "2024-01-01T12:34:56Z")
fn to_iso(time: std::time::SystemTime) -> String {
    let datetime: DateTime<chrono::Utc> = time.into();
    datetime.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Bundle status showing uncommitted changes.
///
/// Represents the current state of a BundleBuilder with information about
/// all the operations that have been queued but not yet committed.
#[derive(Debug, Clone)]
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
            write!(
                f,
                "Bundle Status: {} change(s), {} total operation(s)\n",
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
                    write!(f, "\n")?;
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
/// bundle.attach("data.parquet").await?
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
    /// bundle.attach("data.parquet").await?;
    /// ```
    pub async fn create(
        path: &str,
        config: Option<BundleConfig>,
    ) -> Result<BundleBuilder, BundlebaseError> {
        let mut existing = Bundle::empty().await?;
        existing.passed_config = config;
        existing.recompute_config()?;
        existing.data_dir = ObjectStoreDir::from_str(path, existing.config.clone())?;
        Ok(BundleBuilder {
            status: BundleStatus::new(),
            bundle: existing,
            in_progress_change: None,
        })
    }

    pub fn extend(bundle: Arc<Bundle>, data_dir: &str) -> Result<BundleBuilder, BundlebaseError> {
        let mut new_bundle = bundle.deref().clone();
        new_bundle.data_dir = ObjectStoreDir::from_str(data_dir, bundle.config())?;
        if new_bundle.data_dir.url() != bundle.url() {
            new_bundle.last_manifest_version = 0;
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
    /// bundle.attach("data.parquet").await?;
    /// bundle.filter("amount > 100").await?;
    /// bundle.commit("Filter high-value transactions").await?;
    /// ```
    pub async fn commit(&mut self, message: &str) -> Result<(), BundlebaseError> {
        let manifest_dir = self.bundle.data_dir.subdir(META_DIR)?; //todo rename the dir

        if self.bundle.last_manifest_version == 0 {
            let from = self.bundle.from();
            let init_file = manifest_dir.file(INIT_FILENAME)?;
            init_file.write_yaml(&InitCommit::new(from)).await?;
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
        let manifest_file = manifest_dir.file(filename.as_str())?;

        // Write as stream
        let data = bytes::Bytes::from(yaml);
        let stream = futures::stream::iter(vec![Ok::<_, std::io::Error>(data)]);
        manifest_file.write_stream(stream).await?;

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
    /// bundle.attach("data.parquet").await?;
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
    /// bundle.attach("data.parquet").await?;
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
            new.data_dir = ObjectStoreDir::from_url(self.url(), new.config.clone())?;
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
        // Check for nested changes
        match &self.in_progress_change {
            Some(in_progress) => {
                debug!(
                    "Change {} already in progress, not going to separately track {}",
                    in_progress.description, description
                );
            }
            None => {
                let change = BundleChange::new(description);
                self.in_progress_change = Some(change);
            }
        };

        // Execute the closure
        let result = f(self).await;

        // Move change to status on success, discard on error
        match result {
            Ok(_) => {
                if let Some(change) = self.in_progress_change.take() {
                    self.status.changes.push(change);
                }
                Ok(())
            }
            Err(e) => {
                self.in_progress_change = None;
                Err(e)
            }
        }
    }

    /// Attach a data block to the bundle
    pub async fn attach(&mut self, path: &str) -> Result<&mut Self, BundlebaseError> {
        let path = path.to_string();

        self.do_change(&format!("Attach {}", path), |builder| {
            Box::pin(async move {
                if builder.bundle.base_pack.is_none() {
                    builder
                        .apply_operation(DefinePackOp::setup(&ObjectId::generate()).await?.into())
                        .await?;
                    info!(
                        "Created base pack {}",
                        builder.bundle.base_pack.expect("Base pack not set")
                    );
                }

                builder
                    .apply_operation(
                        AttachBlockOp::setup(
                            &builder.bundle.base_pack.expect("Base pack not set"),
                            &path,
                            builder,
                        )
                        .await?
                        .into(),
                    )
                    .await?;

                info!("Attached {} to bundle", path);

                Ok(())
            })
        })
        .await?;

        Ok(self)
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
    /// c.attach("data.csv").await?;
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
    /// # c.attach("data.csv").await?;
    /// let adults = c.select("select * from data where age > 21", vec![]).await?;
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
    /// # c.attach("data.csv").await?;
    /// let adults = c.select("select * from data where age > 21", vec![]).await?;
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

    /// Attach a data block to the joined pack
    pub async fn attach_to_join(
        &mut self,
        join: &str,
        path: &str,
    ) -> Result<&mut Self, BundlebaseError> {
        let pack_join_id = self
            .bundle
            .joins
            .get(join)
            .ok_or(format!("Unknown join '{}'", join))?
            .pack_id()
            .clone();

        let join = join.to_string();
        let path = path.to_string();

        self.do_change(&format!("Attach {} to join '{}'", path, join), |builder| {
            Box::pin(async move {
                builder
                    .apply_operation(
                        AttachBlockOp::setup(&pack_join_id, &path, builder)
                            .await?
                            .into(),
                    )
                    .await?;

                Ok(())
            })
        })
        .await?;

        Ok(self)
    }

    /// Remove a column (mutates self)
    pub async fn remove_column(&mut self, name: &str) -> Result<&mut Self, BundlebaseError> {
        let name = name.to_string();

        self.do_change(&format!("Remove column {}", name), |builder| {
            Box::pin(async move {
                builder
                    .apply_operation(RemoveColumnsOp::setup(vec![name.as_str()]).into())
                    .await?;

                info!("Removed column \"{}\"", name);

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
    pub async fn join(
        &mut self,
        name: &str,
        source: &str,
        expression: &str,
        join_type: JoinTypeOption,
    ) -> Result<&mut Self, BundlebaseError> {
        let name = name.to_string();
        let source = source.to_string();
        let expression = expression.to_string();

        self.do_change(&format!("Join '{}' on {}", name, expression), |builder| {
            Box::pin(async move {
                // Step 1: Create a new pack for the joined data
                let join_pack_id = ObjectId::generate();
                builder
                    .apply_operation(DefinePackOp::setup(&join_pack_id).await?.into())
                    .await?;

                // Step 2: Attach the source data to the join pack
                builder
                    .apply_operation(
                        AttachBlockOp::setup(&join_pack_id, &source, builder)
                            .await?
                            .into(),
                    )
                    .await?;

                // Step 3: Create JoinOp that references the pack
                builder
                    .apply_operation(
                        JoinOp::setup(&name, join_pack_id, &expression, join_type, builder)
                            .await?
                            .into(),
                    )
                    .await?;

                info!("Joined: {} as \"{}\"", source, name);

                Ok(())
            })
        })
        .await?;

        Ok(self)
    }

    /// Define a custom function (mutates self)
    pub async fn define_function(
        &mut self,
        signature: FunctionSignature,
    ) -> Result<&mut Self, BundlebaseError> {
        let name = signature.name().to_string();

        self.do_change(&format!("Define function {}", name), |builder| {
            Box::pin(async move {
                builder
                    .apply_operation(DefineFunctionOp::setup(signature).into())
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
                    .apply_operation(DefineIndexOp::setup(&column).await?.into())
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
                        Some(idx) => idx.id().clone(),
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

                    // Pass data_packs to expand pack tables into block tables
                    let sources = match sql::column_sources_from_df(
                        logical_col.as_str(),
                        &df,
                        Some(&builder.bundle.data_packs),
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
                            .ok_or_else(|| format!("Block {} not found in data_packs", block_id))?;
                        debug!(
                            "Physical source: block {} version {}",
                            &block_id, &block_version
                        );

                        // Check if index already exists at this version
                        let versioned_block =
                            VersionedBlockId::new(block_id.clone(), block_version.clone());
                        let needs_index = builder
                            .bundle()
                            .get_index(&source_col, &versioned_block)
                            .is_none();
                        debug!("Needs index? {}", needs_index);

                        if needs_index {
                            blocks_to_index
                                .entry((index_id.clone(), source_col.clone()))
                                .or_insert_with(Vec::new)
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
        for (_, pack) in &self.bundle.data_packs.read().clone() {
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
        if self.bundle.base_pack.is_some() {
            analyzer.register_table("__base_0".to_string(), "base".to_string());
        }

        // Register joined packs
        for (join_name, _join) in &self.bundle.joins {
            analyzer.register_table(join_name.clone(), join_name.clone());
        }

        // Analyze the plan
        analyzer.analyze(plan).map_err(|e| {
            Box::new(std::io::Error::new(std::io::ErrorKind::Other, e)) as BundlebaseError
        })?;

        // Query for the specific column
        Ok(analyzer.get_source(logical_name))
    }

    pub fn data_dir(&self) -> &ObjectStoreDir {
        &self.bundle.data_dir
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
            .attach(test_datafile("userdata.parquet"))
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
    async fn test_schema_after_remove_column() {
        let mut bundle = BundleBuilder::create("memory:///test_bundle", None)
            .await
            .unwrap();
        bundle
            .attach(test_datafile("userdata.parquet"))
            .await
            .unwrap();

        let schema_before = &bundle.bundle.schema().await.unwrap();
        assert_eq!(schema_before.fields().len(), 13);

        bundle.remove_column("title").await.unwrap();
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
            .attach(test_datafile("userdata.parquet"))
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
            "Empty bundle has no operations"
        );

        let bundle = bundle
            .attach(test_datafile("userdata.parquet"))
            .await
            .unwrap();
        assert_eq!(bundle.bundle.operations().len(), 2);

        bundle.remove_column("title").await.unwrap();
        assert_eq!(bundle.bundle.operations().len(), 3,);
    }

    #[tokio::test]
    async fn test_version() {
        let mut bundle = BundleBuilder::create("memory:///test_bundle", None)
            .await
            .unwrap();

        assert_eq!("empty", bundle.version());

        bundle
            .attach(test_datafile("userdata.parquet"))
            .await
            .unwrap();

        assert_ne!("empty", bundle.version());
    }

    #[tokio::test]
    async fn test_clone_independence() {
        let mut bundle = BundleBuilder::create("memory:///test_bundle", None)
            .await
            .unwrap();
        bundle
            .attach(test_datafile("userdata.parquet"))
            .await
            .unwrap();

        let v1 = bundle.version();

        // Clone and add operation to clone
        let mut bundle_clone = bundle.clone();
        bundle_clone.remove_column("title").await.unwrap();
        let v2 = bundle_clone.version();

        // Original should be unchanged
        assert_eq!(bundle.bundle.operations().len(), 2);
        assert_eq!(bundle_clone.bundle.operations().len(), 3);
        assert_ne!(
            v1, v2,
            "Different operations should have different versions"
        );

        // Test that data_packs are independent
        let orig_packs_count = bundle.bundle.data_packs.read().len();
        let clone_packs_count = bundle_clone.bundle.data_packs.read().len();
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
            .attach(test_datafile("userdata.parquet"))
            .await
            .unwrap();
        bundle.remove_column("title").await.unwrap();
        let bundle = bundle
            .rename_column("first_name", "given_name")
            .await
            .unwrap();

        assert_eq!(bundle.bundle.operations.len(), 4);
    }
}
