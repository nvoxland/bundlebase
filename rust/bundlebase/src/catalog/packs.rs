mod pack_table;

pub use pack_table::PackTable;

use crate::bundle::{BundleFacade, Pack};
use crate::io::ObjectId;
use async_trait::async_trait;
use datafusion::catalog::{SchemaProvider, TableProvider};
use datafusion::error::Result;
use std::any::Any;
use std::sync::Arc;

/// SchemaProvider that exposes Pack tables.
///
/// Each pack is exposed as a table with name `__pack_{id}`, representing
/// the UNION of all blocks in that pack. The actual UNION is computed lazily
/// by the PackUnionTable implementation.
/// Tables query data dynamically from the BundleFacade on each access,
/// ensuring they always reflect the current state.
pub struct PackSchemaProvider {
    facade: Arc<dyn BundleFacade>,
}

impl std::fmt::Debug for PackSchemaProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackSchemaProvider").finish()
    }
}

impl PackSchemaProvider {
    /// Create a new PackSchemaProvider with the given BundleFacade.
    pub fn new(facade: Arc<dyn BundleFacade>) -> Self {
        Self { facade }
    }

    /// Get the BundleFacade reference.
    fn facade(&self) -> &Arc<dyn BundleFacade> {
        &self.facade
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
        let packs = self.facade().packs();
        packs
            .keys()
            .map(Pack::table_name)
            .collect()
    }

    async fn table(&self, name: &str) -> Result<Option<Arc<dyn TableProvider>>> {
        let pack_id = Self::parse_id(name);

        match pack_id {
            Some(id) => {
                let packs = self.facade().packs();
                if let Some(pack) = packs.get(&id) {
                    if pack.is_empty() {
                        return Ok(None);
                    }

                    let union_table = PackTable::new(id, pack.clone())?;
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
            let packs = self.facade().packs();
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

    #[test]
    fn parse_pack_id_non_prefixed() {
        assert!(PackSchemaProvider::parse_id("not_a_pack").is_none());
    }

    #[test]
    fn parse_pack_id_with_prefix() {
        let id = ObjectId::generate();
        let table_name = Pack::table_name(&id);
        let parsed = PackSchemaProvider::parse_id(&table_name);
        assert!(parsed.is_some());
        assert_eq!(parsed.unwrap(), id);
    }

    // Integration tests for PackSchemaProvider with actual Bundle/BundleBuilder
    // are in tests/bundle_schema_e2e.rs
}
