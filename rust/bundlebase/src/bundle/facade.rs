use crate::bundle::BundleCommit;
use crate::{BundlebaseError, BundleBuilder};
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use datafusion::common::ScalarValue;
use datafusion::dataframe::DataFrame;
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

    async fn schema(&self) -> Result<SchemaRef, BundlebaseError>;

    /// Computes the number of rows in the bundle
    async fn num_rows(&self) -> Result<usize, BundlebaseError>;

    /// Builds and returns the final DataFrame
    async fn dataframe(&self) -> Result<Arc<DataFrame>, BundlebaseError>;

    /// Executes a SQL query against the bundle data.
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
    async fn query(
        &self,
        sql: &str,
        params: Vec<ScalarValue>,
    ) -> Result<BundleBuilder, BundlebaseError>;
}
