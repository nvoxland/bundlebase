mod blocks_table;
mod details_table;
mod history_table;
mod indexes_table;
mod packs_table;
mod status_table;
mod views_table;

use crate::bundle::{BundleCommit, BundleStatus, Pack};
use crate::data::ObjectId;

/// Table name for bundle commit history
pub static HISTORY_TABLE: &str = "history";
/// Table name for bundle uncommitted status
pub static STATUS_TABLE: &str = "status";
/// Table name for bundle details
pub static DETAILS_TABLE: &str = "details";
/// Table name for bundle views
pub static VIEWS_TABLE: &str = "views";
/// Table name for bundle indexes
pub static INDEXES_TABLE: &str = "indexes";
/// Table name for bundle packs
pub static PACKS_TABLE: &str = "packs";
/// Table name for bundle blocks
pub static BLOCKS_TABLE: &str = "blocks";
use crate::index::IndexDefinition;
use async_trait::async_trait;
use blocks_table::BundleBlocksTable;
use datafusion::catalog::{SchemaProvider, TableProvider};
use details_table::BundleDetailsTable;
use history_table::BundleHistoryTable;
use indexes_table::BundleIndexesTable;
use packs_table::BundlePacksTable;
use parking_lot::RwLock;
use status_table::BundleStatusTable;
use std::collections::HashMap;
use std::sync::Arc;
use url::Url;
use views_table::BundleViewsTable;

/// Configuration for BundleInfoSchemaProvider, grouping related parameters.
///
/// This struct organizes the parameters needed to create a BundleInfoSchemaProvider
/// into logical groupings to improve API ergonomics.
#[derive(Debug)]
pub struct BundleInfoConfig { //todo: remove
    /// Bundle identifier
    pub id: Arc<RwLock<String>>,
    /// Bundle URL (immutable)
    pub url: Url,
    /// Parent bundle URL, if extended from another bundle (immutable)
    pub from: Option<Url>,
    /// Bundle name (mutable via operations)
    pub name: Arc<RwLock<Option<String>>>,
    /// Bundle description (mutable via operations)
    pub description: Arc<RwLock<Option<String>>>,
    /// Bundle version hash (recomputed on each operation)
    pub version: Arc<RwLock<String>>,
    /// Commit history
    pub commits: Arc<RwLock<Vec<BundleCommit>>>,
    /// Views by name
    pub views: Arc<RwLock<HashMap<String, ObjectId>>>,
    /// Index definitions
    pub indexes: Arc<RwLock<Vec<Arc<IndexDefinition>>>>,
    /// Packs by ID
    pub packs: Arc<RwLock<HashMap<ObjectId, Arc<Pack>>>>,
}

/// SchemaProvider that exposes bundle metadata tables in the "bundle_info" schema.
/// Provides:
/// - `history`: Commit history for the bundle
/// - `status`: Uncommitted changes (always empty - use BundleFacade::status() instead)
/// - `details`: Bundle metadata (id, name, description, url, from, version)
/// - `views`: List of views in the bundle
/// - `indexes`: List of indexes in the bundle
/// - `packs`: List of packs in the bundle
/// - `blocks`: List of blocks in the bundle
#[derive(Debug)]
pub struct BundleInfoSchemaProvider {
    commits: Arc<RwLock<Vec<BundleCommit>>>,
    id: Arc<RwLock<String>>,
    name: Arc<RwLock<Option<String>>>,
    description: Arc<RwLock<Option<String>>>,
    url: Url,
    from: Option<Url>,
    version: Arc<RwLock<String>>,
    views: Arc<RwLock<HashMap<String, ObjectId>>>,
    indexes: Arc<RwLock<Vec<Arc<IndexDefinition>>>>,
    packs: Arc<RwLock<HashMap<ObjectId, Arc<Pack>>>>,
}

//todo: Only pass BundleFacade
impl BundleInfoSchemaProvider {
    /// Create a new BundleInfoSchemaProvider with individual parameters.
    ///
    /// For a more ergonomic API, consider using `from_config()` instead.
    pub fn new(
        commits: Arc<RwLock<Vec<BundleCommit>>>,
        id: Arc<RwLock<String>>,
        name: Arc<RwLock<Option<String>>>,
        description: Arc<RwLock<Option<String>>>,
        url: Url,
        from: Option<Url>,
        version: Arc<RwLock<String>>,
        views: Arc<RwLock<HashMap<String, ObjectId>>>,
        indexes: Arc<RwLock<Vec<Arc<IndexDefinition>>>>,
        packs: Arc<RwLock<HashMap<ObjectId, Arc<Pack>>>>,
    ) -> Self {
        Self {
            commits,
            id,
            name,
            description,
            url,
            from,
            version,
            views,
            indexes,
            packs,
        }
    }

    /// Create a new BundleInfoSchemaProvider from a config struct.
    ///
    /// This provides a more ergonomic API than `new()` when all parameters
    /// are available together.
    pub fn from_config(config: BundleInfoConfig) -> Self {
        Self {
            commits: config.commits,
            id: config.id,
            name: config.name,
            description: config.description,
            url: config.url,
            from: config.from,
            version: config.version,
            views: config.views,
            indexes: config.indexes,
            packs: config.packs,
        }
    }
}

#[async_trait]
impl SchemaProvider for BundleInfoSchemaProvider {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn table_names(&self) -> Vec<String> {
        vec![
            HISTORY_TABLE.to_string(),
            STATUS_TABLE.to_string(),
            DETAILS_TABLE.to_string(),
            VIEWS_TABLE.to_string(),
            INDEXES_TABLE.to_string(),
            PACKS_TABLE.to_string(),
            BLOCKS_TABLE.to_string(),
        ]
    }

    //todo: these need to take BundleFacade
    //todo: these need to be singletons, maybe. But also need to get the active bundlefacade
    async fn table(&self, name: &str) -> datafusion::error::Result<Option<Arc<dyn TableProvider>>> {
        if name == HISTORY_TABLE {
            let commits = self.commits.read().clone();
            let table = BundleHistoryTable::new(commits)?;
            Ok(Some(Arc::new(table)))
        } else if name == STATUS_TABLE {
            // Status is always empty via SQL - use BundleFacade::status() for live status
            let table = BundleStatusTable::new(BundleStatus::new())?;
            Ok(Some(Arc::new(table)))
        } else if name == DETAILS_TABLE {
            let table = BundleDetailsTable::new(
                &self.id.read(),
                self.name.read().as_deref(),
                self.description.read().as_deref(),
                self.url.as_str(),
                self.from.as_ref().map(|u| u.as_str()),
                &self.version.read(),
            )?;
            Ok(Some(Arc::new(table)))
        } else if name == VIEWS_TABLE {
            let views = self.views.read().clone();
            let table = BundleViewsTable::new(views)?;
            Ok(Some(Arc::new(table)))
        } else if name == INDEXES_TABLE {
            let indexes = self.indexes.read().clone();
            let table = BundleIndexesTable::new(indexes)?;
            Ok(Some(Arc::new(table)))
        } else if name == PACKS_TABLE {
            let packs = self.packs.read().clone();
            let table = BundlePacksTable::new(packs)?;
            Ok(Some(Arc::new(table)))
        } else if name == BLOCKS_TABLE {
            let packs = self.packs.read().clone();
            let table = BundleBlocksTable::new(packs)?;
            Ok(Some(Arc::new(table)))
        } else {
            Ok(None)
        }
    }

    fn table_exist(&self, name: &str) -> bool {
        name == HISTORY_TABLE
            || name == STATUS_TABLE
            || name == DETAILS_TABLE
            || name == VIEWS_TABLE
            || name == INDEXES_TABLE
            || name == PACKS_TABLE
            || name == BLOCKS_TABLE
    }
}
