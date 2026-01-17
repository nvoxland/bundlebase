use crate::catalog::PackUnionTable;
use crate::bundle::Pack;
use crate::io::ObjectId;
use async_trait::async_trait;
use datafusion::catalog::{SchemaProvider, TableProvider};
use datafusion::error::Result;
use parking_lot::RwLock;
use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

/// SchemaProvider that exposes Pack tables.
///
/// Each pack is exposed as a table with name `__pack_{id}`, representing
/// the UNION of all blocks in that pack. The actual UNION is computed lazily
/// by the PackUnionTable implementation.
pub struct PackSchemaProvider {
    packs: Arc<RwLock<HashMap<ObjectId, Arc<Pack>>>>,
}

impl std::fmt::Debug for PackSchemaProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackSchemaProvider")
            .field("packs", &"<Arc<RwLock<HashMap>>>")
            .field("ctx", &"<SessionContext>")
            .finish()
    }
}

impl PackSchemaProvider {
    pub fn new(packs: Arc<RwLock<HashMap<ObjectId, Arc<Pack>>>>) -> Self {
        Self { packs }
    }

    /// Extract pack ID from table name (e.g., "__pack_abc123" -> "abc123")
    fn parse_id(name: &str) -> Option<ObjectId> {
        name.strip_prefix("__pack_")
            .and_then(|id| id.try_into().ok())
    }
}

#[async_trait]
impl SchemaProvider for PackSchemaProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn table_names(&self) -> Vec<String> {
        let packs = self.packs.read();
        packs
            .keys()
            .map(Pack::table_name)
            .collect()
    }

    async fn table(&self, name: &str) -> Result<Option<Arc<dyn TableProvider>>> {
        let pack_id = Self::parse_id(name);

        match pack_id {
            Some(id) => {
                let packs = self.packs.read();
                if let Some(pack) = packs.get(&id) {
                    if pack.is_empty() {
                        return Ok(None);
                    }

                    let union_table = PackUnionTable::new(id, pack.clone())?;
                    Ok(Some(Arc::new(union_table)))
                } else {
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    }

    fn table_exist(&self, name: &str) -> bool {
        if let Some(pack_id) = Self::parse_id(name) {
            let packs = self.packs.read();
            if let Some(pack) = packs.get(&pack_id) {
                !pack.is_empty()
            } else {
                false
            }
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::BlockSchemaProvider;
    use crate::bundle::DataBlock;
    use crate::data::MockReader;
    use crate::{BundleConfig, JoinTypeOption};
    use arrow_schema::{DataType, Field, Schema};
    use datafusion::prelude::SessionContext;
    use parking_lot::RwLock;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[test]
    fn parse_block_id_non_prefixed() {
        assert!(PackSchemaProvider::parse_id("not_a_block").is_none());
    }

    #[tokio::test]
    async fn empty_provider() {
        let packs = Arc::new(RwLock::new(HashMap::<ObjectId, Arc<Pack>>::new()));
        let provider = PackSchemaProvider::new(packs);
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
        let ctx = Arc::new(SessionContext::new());

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

        ctx.catalog("datafusion")
            .unwrap()
            .register_schema("blocks", Arc::new(BlockSchemaProvider::new(packs.clone())))
            .unwrap();
        let provider = PackSchemaProvider::new(packs);

        let names = provider.table_names();
        assert!(names.contains(&Pack::table_name(&pack1_id)));
        assert!(names.contains(&Pack::table_name(&pack2_id)));

        assert!(provider.table_exist(&Pack::table_name(&pack1_id)));
        assert!(provider.table_exist(&Pack::table_name(&pack2_id)));

        assert!(provider
            .table(&Pack::table_name(&pack1_id))
            .await
            .unwrap()
            .is_some());
        assert!(provider
            .table(&Pack::table_name(&pack2_id))
            .await
            .unwrap()
            .is_some());

        assert_eq!(
            schema1,
            provider
                .table(&Pack::table_name(&pack1_id))
                .await
                .unwrap()
                .unwrap()
                .schema()
        );
        assert_eq!(
            schema2,
            provider
                .table(&Pack::table_name(&pack2_id))
                .await
                .unwrap()
                .unwrap()
                .schema()
        );

        assert_eq!(
            schema1,
            provider
                .table(&Pack::table_name(&pack1_id))
                .await
                .unwrap()
                .unwrap()
                .scan(&ctx.state(), None, &[], None)
                .await
                .unwrap()
                .schema()
        )
    }
}
