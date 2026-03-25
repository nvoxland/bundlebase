use super::column_metadata::ColumnNames;
use super::operation::BundleChange;
use crate::bundle::BundleCommit;
use crate::bundle::BundleStatus;
use crate::bundle::Pack;
use crate::index::IndexDefinition;
use crate::io::{IOReadWriteDir, ObjectId};
use crate::object_id::ColumnId;
use crate::bundle_config::Scope;
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
    /// Returns self as `&dyn Any` for downcasting to concrete types.
    fn as_any(&self) -> &dyn std::any::Any;

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

    /// Returns the fully-resolved column ID -> name map after all operations.
    fn column_names(&self) -> ColumnNames;

    /// Resolve a column name to its ColumnId using the current operations.
    fn column_id(&self, name: &str) -> Option<ColumnId> {
        self.column_names()
            .iter()
            .find(|(_, n)| n.as_str() == name)
            .map(|(id, _)| *id)
    }

    /// Resolve a ColumnId to its current column name using the current operations.
    fn column_name(&self, id: &ColumnId) -> Option<String> {
        self.column_names().get(id).cloned()
    }

    async fn schema(&self) -> Result<SchemaRef, BundlebaseError>;

    /// Computes the number of rows in the bundle
    async fn num_rows(&self) -> Result<usize, BundlebaseError>;

    /// Builds and returns the final DataFrame
    async fn dataframe(&self) -> Result<Arc<DataFrame>, BundlebaseError>;

    /// Extends this bundle to create a new BundleBuilder.
    async fn extend(
        &self,
        data_dir: Option<&str>,
    ) -> Result<Arc<BundleBuilder>, BundlebaseError>;

    /// Executes a SQL query and returns streaming results directly.
    async fn query(
        &self,
        sql: &str,
        params: Vec<ScalarValue>,
        hard_limit: Option<usize>,
    ) -> Result<SendableRecordBatchStream, BundlebaseError>;

    /// Returns a map of view IDs to view names for all views in this container
    fn views(&self) -> HashMap<ObjectId, String>;

    /// Open a view by name or ID, returning a read-only Bundle
    async fn view(&self, identifier: &str) -> Result<Arc<Bundle>, BundlebaseError>;

    /// Exports the bundle's data directory to an uncompressed tar archive.
    async fn export_tar(&self, tar_path: &str) -> Result<String, BundlebaseError>;

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

    /// Remove runtime-only connector for a defined source.
    async fn drop_temp_connector(
        &self,
        name: &str,
        platform: Option<&crate::platform::Platform>,
    ) -> Result<usize, BundlebaseError>;

    /// Remove runtime-only function entries.
    async fn drop_temp_function(
        &self,
        name: &str,
        platform: Option<&crate::platform::Platform>,
    ) -> Result<usize, BundlebaseError>;

    /// Rename runtime-only connector entries.
    async fn rename_temp_connector(
        &self,
        old_name: &str,
        new_name: &str,
    ) -> Result<(), BundlebaseError>;

    /// Rename runtime-only function entries.
    async fn rename_temp_function(
        &self,
        old_name: &str,
        new_name: &str,
    ) -> Result<(), BundlebaseError>;

    /// Set a runtime config value (session-only, highest priority).
    async fn set_config(
        &self,
        scope: &Scope,
        key: &str,
        value: &str,
    ) -> Result<(), BundlebaseError>;

    /// Returns the connector registry.
    fn connector_registry(&self) -> Arc<parking_lot::RwLock<crate::source::ConnectorRegistry>>;

    /// Returns the function registry.
    fn function_registry(&self) -> Arc<parking_lot::RwLock<crate::bundle::function_entry::FunctionRegistry>>;

    /// Returns the DataFusion session context
    fn ctx(&self) -> Arc<SessionContext>;
}
