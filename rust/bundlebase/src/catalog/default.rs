use crate::bundle::BundleFacade;
use async_trait::async_trait;
use datafusion::catalog::{SchemaProvider, TableProvider};
use std::sync::Arc;

/// Alias dataframe is registered in the ctx under. User can select from this
pub static BUNDLE_TABLE: &str = "bundle";

/// SchemaProvider that exposes the bundle's cached dataframe as a "bundle" table.
/// Tables query data dynamically from the BundleFacade on each access,
/// ensuring they always reflect the current state.
pub struct DefaultSchemaProvider {
    bundle: Arc<dyn BundleFacade>,
}

impl std::fmt::Debug for DefaultSchemaProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DefaultSchemaProvider").finish()
    }
}

impl DefaultSchemaProvider {
    /// Create a new DefaultSchemaProvider with the given BundleFacade.
    pub fn new(bundle: Arc<dyn BundleFacade>) -> Self {
        Self { bundle }
    }
}

#[async_trait]
impl SchemaProvider for DefaultSchemaProvider {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn table_names(&self) -> Vec<String> {
        vec![BUNDLE_TABLE.to_string()]
    }

    async fn table(&self, name: &str) -> datafusion::error::Result<Option<Arc<dyn TableProvider>>> {
        if name == BUNDLE_TABLE {

            let df = self
                .bundle
                .dataframe()
                .await
                .map_err(|e| datafusion::error::DataFusionError::External(e.into()))?;

            Ok(Some((*df).clone().into_view()))
        } else {
            Ok(None)
        }
    }

    fn table_exist(&self, name: &str) -> bool {
        name == BUNDLE_TABLE
    }
}
