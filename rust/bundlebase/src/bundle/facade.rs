use crate::bundle::operation::BundleChange;
use crate::bundle::BundleCommit;
use crate::bundle::BundleStatus;
use crate::bundle::Pack;
use crate::index::IndexDefinition;
use crate::io::{IOReadWriteDir, ObjectId};
use crate::{AnyOperation, Bundle, BundleBuilder, BundleConfig, BundlebaseError};
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use datafusion::common::ScalarValue;
use datafusion::dataframe::DataFrame;
use datafusion::execution::SendableRecordBatchStream;
use datafusion::prelude::SessionContext;
use std::collections::HashMap;
use std::sync::Arc;
use url::Url;

#[async_trait]
pub trait BundleFacade: Send + Sync {
    /// The id of the bundle
    fn id(&self) -> String;

    /// Retrieve the bundle name, if set.
    fn name(&self) -> Option<String>;

    /// Retrieve the bundle description, if set.
    fn description(&self) -> Option<String>;

    /// Retrieve the URL of the base bundle this was loaded from, if any.
    fn url(&self) -> Url;

    /// The base bundle this was extended from
    fn from(&self) -> Option<Url>;

    /// Unique version for this bundle
    fn version(&self) -> String;

    /// Returns the commit history for this bundle, including any base bundles
    fn history(&self) -> Vec<BundleCommit>;

    /// All operations applied to this bundle
    fn operations(&self) -> Vec<AnyOperation>;

    async fn schema(&self) -> Result<SchemaRef, BundlebaseError>;

    /// Computes the number of rows in the bundle
    async fn num_rows(&self) -> Result<usize, BundlebaseError>;

    /// Builds and returns the final DataFrame
    async fn dataframe(&self) -> Result<Arc<DataFrame>, BundlebaseError>;

    // todo: don't extend bundles when uncommitted changes
    /// Extends this bundle to create a new BundleBuilder.
    ///
    /// This is the primary way to create a new BundleBuilder from an existing bundle.
    /// The new builder can optionally have a different data directory.
    ///
    /// # Arguments
    /// * `data_dir` - Optional new data directory. If None, uses the current bundle's data_dir.
    ///
    /// # Returns
    /// A new BundleBuilder extending from this bundle.
    ///
    /// # Example
    /// ```ignore
    /// // Extend with a new data directory
    /// let builder = bundle.extend(Some("s3://bucket/new"))?;
    ///
    /// // Extend and then filter
    /// let builder = bundle.extend(None)?;
    /// builder.filter("active = true", vec![]).await?;
    /// ```
    fn extend(
        &self,
        data_dir: Option<&str>,
    ) -> Result<Arc<BundleBuilder>, BundlebaseError>;

    /// Executes a SQL query and returns streaming results directly.
    ///
    /// Unlike `extend()` with SQL, this does NOT create a new BundleBuilder.
    /// It directly executes the query and streams the results. Use this when
    /// you want to read data from the bundle without creating a new builder.
    ///
    /// # Arguments
    /// * `sql` - SQL query string (e.g., "SELECT * FROM bundle WHERE id > 10")
    /// * `params` - Query parameters for parameterized queries ($1, $2, etc.)
    ///
    /// # Returns
    /// A streaming result set that can be consumed incrementally.
    ///
    /// # Example
    /// ```ignore
    /// let stream = bundle.query("SELECT COUNT(*) FROM bundle", vec![]).await?;
    /// while let Some(batch) = stream.next().await {
    ///     // Process batch
    /// }
    /// ```
    async fn query(
        &self,
        sql: &str,
        params: Vec<ScalarValue>,
    ) -> Result<SendableRecordBatchStream, BundlebaseError>;

    /// Returns a map of view IDs to view names for all views in this container
    fn views(&self) -> HashMap<ObjectId, String>;

    /// Open a view by name or ID, returning a read-only Bundle
    ///
    /// Looks up the view by name or ID and opens it as a Bundle. The view automatically
    /// inherits all changes from its parent bundle through the FROM mechanism.
    ///
    /// # Arguments
    /// * `identifier` - Name or ID of the view to open
    ///
    /// # Returns
    /// A read-only Bundle representing the view
    ///
    /// # Errors
    /// Returns an error if the view doesn't exist or if the identifier is ambiguous
    ///
    /// # Example
    /// ```no_run
    /// # use bundlebase::{Bundle, BundleBuilder, BundlebaseError, BundleFacade};
    /// # async fn example(c: &BundleBuilder) -> Result<(), BundlebaseError> {
    /// // Open by name
    /// let view = c.view("adults").await?;
    ///
    /// // Or open by ID
    /// let view = c.view("abc123def456").await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn view(&self, identifier: &str) -> Result<Arc<Bundle>, BundlebaseError>;

    /// Exports the bundle's data directory to an uncompressed tar archive.
    ///
    /// Creates a tar file containing all bundle data including:
    /// - `_bundlebase/` directory with all commit manifests
    /// - All data files (parquet, CSV, etc.)
    /// - All index files
    /// - All layout files
    ///
    /// The resulting tar file can be opened as a bundle and supports
    /// further commits via append-only mode since bundlebase never modifies
    /// existing files.
    ///
    /// # Arguments
    /// * `tar_path` - Path where the tar file should be created
    ///
    /// # Returns
    /// Success message with the tar file path
    ///
    /// # Errors
    /// Returns an error if the tar file cannot be created or if there are
    /// uncommitted changes (for BundleBuilder instances).
    ///
    /// # Example
    /// ```ignore
    /// bundle.export_tar("archive.tar").await?;
    /// let archived = Bundle::open("archive.tar", None).await?;
    /// ```
    async fn export_tar(&self, tar_path: &str) -> Result<String, BundlebaseError>;

    /// Returns the query execution plan as a formatted string
    async fn explain(&self) -> Result<String, BundlebaseError>;

    /// Returns uncommitted changes (empty for Bundle, populated for BundleBuilder)
    fn status_changes(&self) -> Vec<BundleChange>;

    /// Returns the current bundle status
    fn status(&self) -> BundleStatus;

    /// Returns index definitions
    fn indexes(&self) -> Vec<Arc<IndexDefinition>>;

    /// Returns packs (id -> pack)
    fn packs(&self) -> HashMap<ObjectId, Arc<Pack>>;

    /// Returns views by name (name -> id mapping)
    fn views_by_name(&self) -> HashMap<String, ObjectId>;

    /// Returns the data directory for this bundle
    fn data_dir(&self) -> Arc<dyn IOReadWriteDir>;

    /// Returns the bundle configuration
    fn config(&self) -> Arc<BundleConfig>;

    /// Returns the DataFusion session context
    fn ctx(&self) -> Arc<SessionContext>;
}
