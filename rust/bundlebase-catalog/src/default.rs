use async_trait::async_trait;
use bundlebase::bundle::BundleFacade;
use bundlebase::catalog::BundleViewTable;
use datafusion::catalog::{SchemaProvider, TableProvider};
use std::sync::{Arc, Weak};

/// Alias dataframe is registered in the ctx under. User can select from this
pub use bundlebase::catalog::BUNDLE_TABLE;

/// SchemaProvider that exposes the bundle's cached dataframe as a "bundle" table.
/// Tables query data dynamically from the BundleFacade on each access,
/// ensuring they always reflect the current state.
///
/// Holds a `Weak` reference to avoid an Arc reference cycle:
/// BundleBuilder -> Bundle -> SessionContext -> SchemaProviders -> BundleBuilder.
pub struct DefaultSchemaProvider {
    bundle: Weak<dyn BundleFacade>,
}

impl std::fmt::Debug for DefaultSchemaProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DefaultSchemaProvider").finish()
    }
}

impl DefaultSchemaProvider {
    /// Create a new DefaultSchemaProvider with the given BundleFacade.
    pub fn new(bundle: Weak<dyn BundleFacade>) -> Self {
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
            let facade = self.bundle.upgrade().ok_or_else(|| {
                datafusion::error::DataFusionError::Internal(
                    "Bundle has been dropped (while resolving 'bundle' table)".to_string(),
                )
            })?;

            let df = facade
                .dataframe()
                .await
                .map_err(|e| datafusion::error::DataFusionError::External(e.into()))?;

            Ok(Some(Arc::new(BundleViewTable::new((*df).clone()))))
        } else {
            Ok(None)
        }
    }

    fn table_exist(&self, name: &str) -> bool {
        name == BUNDLE_TABLE
    }
}
