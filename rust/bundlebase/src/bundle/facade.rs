use crate::bundle::BundleCommit;
use crate::io::ObjectId;
use crate::{AnyOperation, Bundle, BundleBuilder, BundlebaseError};
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use datafusion::common::ScalarValue;
use datafusion::dataframe::DataFrame;
use std::collections::HashMap;
use std::sync::Arc;
use url::Url;

#[async_trait]
pub trait BundleFacade {
    /// The id of the bundle
    fn id(&self) -> &str;

    /// Retrieve the bundle name, if set.
    fn name(&self) -> Option<&str>;

    /// Retrieve the bundle description, if set.
    fn description(&self) -> Option<&str>;

    /// Retrieve the URL of the base bundle this was loaded from, if any.
    fn url(&self) -> &Url;

    /// The base bundle this was extended from
    fn from(&self) -> Option<&Url>;

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

    /// Executes a SQL query against the bundle data. "SELECT" keyword in SQL is optional.
    ///
    /// Returns a new `BundleBuilder` with the query applied as an operation.
    /// Parameters can be used for parameterized queries.
    ///
    /// # Arguments
    /// * `sql` - SQL query string (e.g., "SELECT * FROM table WHERE id = ?")
    /// * `params` - Optional query parameters for parameterized queries
    ///
    /// # Returns
    /// A new bundle with the query operation added to its operation chain.
    ///
    /// # Errors
    /// Returns error if the query is invalid or references non-existent columns.
    async fn select(
        &self,
        sql: &str,
        params: Vec<ScalarValue>,
    ) -> Result<BundleBuilder, BundlebaseError>;

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
    async fn view(&self, identifier: &str) -> Result<Bundle, BundlebaseError>;

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
}
