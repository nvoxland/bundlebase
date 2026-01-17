use crate::bundle::{DataBlock, Pack};
use crate::io::ObjectId;
use async_trait::async_trait;
use datafusion::catalog::{SchemaProvider, TableProvider};
use datafusion::error::Result;
use parking_lot::RwLock;
use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

/// SchemaProvider that exposes individual DataBlock tables.
///
/// Each block in each pack is exposed as a table with name `__block_{id}`.
/// This provider dynamically discovers blocks by scanning through all data packs.
#[derive(Debug)]
pub struct BlockSchemaProvider {
    packs: Arc<RwLock<HashMap<ObjectId, Arc<Pack>>>>,
}

impl BlockSchemaProvider {
    pub fn new(packs: Arc<RwLock<HashMap<ObjectId, Arc<Pack>>>>) -> Self {
        Self { packs }
    }

    /// Extract block ID from table name (e.g., "__block_abc123" -> "abc123")
    fn parse_id(name: &str) -> Option<ObjectId> {
        name.strip_prefix("__block_")
            .and_then(|id| id.try_into().ok())
    }

    /// Find a block by ID across all packs
    fn find_block(&self, block_id: &ObjectId) -> Option<Arc<DataBlock>> {
        let packs = self.packs.read();
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
        let packs = self.packs.read();
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
                    Ok(Some(block as Arc<dyn TableProvider>))
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
    use crate::data::MockReader;
    use crate::{BundleConfig, JoinTypeOption};
    use arrow_schema::{DataType, Field, Schema};
    use parking_lot::RwLock;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[test]
    fn parse_block_id_non_prefixed() {
        assert!(BlockSchemaProvider::parse_id("not_a_block").is_none());
    }

    #[tokio::test]
    async fn empty_provider() {
        let packs = Arc::new(RwLock::new(HashMap::<ObjectId, Arc<Pack>>::new()));
        let provider = BlockSchemaProvider::new(packs);
        assert!(provider.table_names().is_empty());
        assert!(!provider.table_exist("__block_nonexistent"));
        assert!(provider
            .table("__block_nonexistent")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn provider_with_blocks() {
        let block11_id = ObjectId::generate();
        let block12_id = ObjectId::generate();
        let block21_id = ObjectId::generate();
        let pack1_id = ObjectId::generate();
        let pack2_id = ObjectId::generate();

        let schema1 = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("value1", DataType::Utf8, true),
        ]));

        let schema2 = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("value2", DataType::Utf8, true),
        ]));

        // Create empty indexes and data_dir for test
        let indexes = Arc::new(parking_lot::RwLock::new(Vec::new()));
        let data_dir = Arc::new(
            crate::io::plugin::object_store::ObjectStoreDir::from_str("memory:///test", BundleConfig::default().into())
                .unwrap(),
        );

        let block11 = Arc::new(DataBlock::new(
            block11_id,
            schema1.clone(),
            "32",
            Arc::new(MockReader::with_schema(schema1.clone())),
            indexes.clone(),
            data_dir.clone(),
            BundleConfig::default().into(),
            None,
        ));

        let block12 = Arc::new(DataBlock::new(
            block12_id,
            schema1.clone(),
            "32",
            Arc::new(MockReader::with_schema(schema1.clone())),
            indexes.clone(),
            data_dir.clone(),
            BundleConfig::default().into(),
            None,
        ));

        let block21 = Arc::new(DataBlock::new(
            block21_id,
            schema2.clone(),
            "32",
            Arc::new(MockReader::with_schema(schema2.clone())),
            indexes.clone(),
            data_dir.clone(),
            BundleConfig::default().into(),
            None,
        ));

        let pack1 = Arc::new(Pack::new(pack1_id, "pack1", "", JoinTypeOption::Full));
        pack1.add_block(block11);
        pack1.add_block(block12);

        let pack2 = Arc::new(Pack::new(pack2_id, "pack2", "", JoinTypeOption::Full));
        pack2.add_block(block21);

        let mut map = HashMap::new();
        map.insert(pack1_id, pack1);
        map.insert(pack2_id, pack2);
        let packs = Arc::new(RwLock::new(map));

        let provider = BlockSchemaProvider::new(packs);

        let names = provider.table_names();
        assert!(names.contains(&DataBlock::table_name(&block11_id)));
        assert!(names.contains(&DataBlock::table_name(&block12_id)));
        assert!(names.contains(&DataBlock::table_name(&block21_id)));

        assert!(provider.table_exist(&DataBlock::table_name(&block11_id)));
        assert!(provider.table_exist(&DataBlock::table_name(&block12_id)));
        assert!(provider.table_exist(&DataBlock::table_name(&block11_id)));

        assert!(provider
            .table(&DataBlock::table_name(&block11_id))
            .await
            .unwrap()
            .is_some());
        assert!(provider
            .table(&DataBlock::table_name(&block12_id))
            .await
            .unwrap()
            .is_some());
        assert!(provider
            .table(&DataBlock::table_name(&block21_id))
            .await
            .unwrap()
            .is_some());

        assert_eq!(
            schema1,
            provider
                .table(&DataBlock::table_name(&block11_id))
                .await
                .unwrap()
                .unwrap()
                .schema()
        );
    }
}
