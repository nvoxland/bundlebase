mod block_table;

use crate::bundle::{BundleFacade, DataBlock};
use crate::io::ObjectId;
use async_trait::async_trait;
use block_table::BlockTable;
use datafusion::catalog::{SchemaProvider, TableProvider};
use datafusion::error::Result;
use std::any::Any;
use std::sync::Arc;

/// SchemaProvider that exposes individual DataBlock tables.
///
/// Each block in each pack is exposed as a table with name `__block_{id}`.
/// This provider dynamically discovers blocks by scanning through all data packs.
/// Tables query data dynamically from the BundleFacade on each access,
/// ensuring they always reflect the current state.
pub struct BlockSchemaProvider {
    bundle: Arc<dyn BundleFacade>,
}

impl std::fmt::Debug for BlockSchemaProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlockSchemaProvider").finish()
    }
}

impl BlockSchemaProvider {
    /// Create a new BlockSchemaProvider with the given BundleFacade.
    pub fn new(facade: Arc<dyn BundleFacade>) -> Self {
        Self { bundle: facade }
    }

    /// Get the BundleFacade reference.
    fn bundle(&self) -> &Arc<dyn BundleFacade> {
        &self.bundle
    }

    /// Extract block ID from table name (e.g., "__block_abc123" -> "abc123")
    fn parse_id(name: &str) -> Option<ObjectId> {
        name.strip_prefix("__block_")
            .and_then(|id| id.try_into().ok())
    }

    /// Find a block by ID across all packs
    fn find_block(&self, block_id: &ObjectId) -> Option<Arc<DataBlock>> {
        let packs = self.bundle().packs();
        for pack in packs.values() {
            let blocks = pack.blocks();
            for block in blocks {
                if block.id() == block_id {
                    return Some(block);
                }
            }
        }
        None
    }
}


#[async_trait]
impl SchemaProvider for BlockSchemaProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn table_names(&self) -> Vec<String> {
        let packs = self.bundle().packs();
        let mut names = Vec::new();

        for pack in packs.values() {
            let blocks = pack.blocks();
            for block in blocks {
                names.push(DataBlock::table_name(block.id()));
            }
        }

        names
    }

    async fn table(&self, name: &str) -> Result<Option<Arc<dyn TableProvider>>> {
        let block_id = Self::parse_id(name);

        match block_id {
            Some(id) => {
                if let Some(block) = self.find_block(&id) {
                    Ok(Some(Arc::new(BlockTable::new(block))))
                } else {
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    }

    fn table_exist(&self, name: &str) -> bool {
        if let Some(block_id) = Self::parse_id(name) {
            self.find_block(&block_id).is_some()
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_block_id_non_prefixed() {
        assert!(BlockSchemaProvider::parse_id("not_a_block").is_none());
    }

    #[test]
    fn parse_block_id_with_prefix() {
        let id = ObjectId::generate();
        let table_name = DataBlock::table_name(&id);
        let parsed = BlockSchemaProvider::parse_id(&table_name);
        assert!(parsed.is_some());
        assert_eq!(parsed.unwrap(), id);
    }

    // Integration tests for BlockSchemaProvider with actual Bundle/BundleBuilder
    // are in tests/bundle_schema_e2e.rs
}
