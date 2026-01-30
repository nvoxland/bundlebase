mod blocks_table;
mod details_table;
mod history_table;
mod indexes_table;
mod packs_table;
mod status_table;
mod views_table;

use super::tables;
use crate::bundle::BundleFacade;

use async_trait::async_trait;
use blocks_table::BundleBlocksTable;
use datafusion::catalog::{SchemaProvider, TableProvider};
use details_table::BundleDetailsTable;
use history_table::BundleHistoryTable;
use indexes_table::BundleIndexesTable;
use packs_table::BundlePacksTable;
use status_table::BundleStatusTable;
use std::collections::HashMap;
use std::sync::Arc;
use views_table::BundleViewsTable;

/// SchemaProvider that exposes bundle metadata tables in the "bundle_info" schema.
///
/// Tables query data dynamically from the BundleFacade on each access,
/// ensuring they always reflect the current state.
pub struct BundleInfoSchemaProvider {
    tables: HashMap<&'static str, Arc<dyn TableProvider>>,
}

impl std::fmt::Debug for BundleInfoSchemaProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BundleInfoSchemaProvider").finish()
    }
}

impl BundleInfoSchemaProvider {
    /// Create a new BundleInfoSchemaProvider with the given BundleFacade.
    pub fn new(bundle: Arc<dyn BundleFacade>) -> Self {
        let mut tables: HashMap<&'static str, Arc<dyn TableProvider>> = HashMap::new();
        tables.insert(tables::HISTORY, Arc::new(BundleHistoryTable::new(bundle.clone())));
        tables.insert(tables::STATUS, Arc::new(BundleStatusTable::new(bundle.clone())));
        tables.insert(tables::DETAILS, Arc::new(BundleDetailsTable::new(bundle.clone())));
        tables.insert(tables::VIEWS, Arc::new(BundleViewsTable::new(bundle.clone())));
        tables.insert(tables::INDEXES, Arc::new(BundleIndexesTable::new(bundle.clone())));
        tables.insert(tables::PACKS, Arc::new(BundlePacksTable::new(bundle.clone())));
        tables.insert(tables::BLOCKS, Arc::new(BundleBlocksTable::new(bundle)));
        Self { tables }
    }
}

#[async_trait]
impl SchemaProvider for BundleInfoSchemaProvider {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn table_names(&self) -> Vec<String> {
        self.tables.keys().map(|k| k.to_string()).collect()
    }

    async fn table(&self, name: &str) -> datafusion::error::Result<Option<Arc<dyn TableProvider>>> {
        Ok(self.tables.get(name).cloned())
    }

    fn table_exist(&self, name: &str) -> bool {
        self.tables.contains_key(name)
    }
}
