use crate::bundle::operation::Operation;
use crate::bundle::Bundle;
use crate::io::ObjectId;
use crate::BundlebaseError;
use async_trait::async_trait;
use datafusion::error::DataFusionError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DropIndexOp {
    pub id: ObjectId,
}

impl DropIndexOp {
    pub async fn setup(id: &ObjectId) -> Result<Self, BundlebaseError> {
        Ok(Self { id: *id })
    }
}

#[async_trait]
impl Operation for DropIndexOp {
    fn describe(&self) -> String {
        format!("DROP INDEX {}", self.id)
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Verify index exists
        let indexes = bundle.indexes().read();
        if !indexes.iter().any(|idx| idx.id() == &self.id) {
            return Err(format!("Index with ID '{}' not found", self.id).into());
        }

        Ok(())
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        // Remove index definition from bundle
        bundle
            .indexes
            .write()
            .retain(|idx| idx.id() != &self.id);

        log::info!("Dropped index {}", self.id);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_drop_index_describe() {
        let index_id = ObjectId::generate();
        let op = DropIndexOp { id: index_id };

        assert_eq!(op.describe(), format!("DROP INDEX {}", index_id));
    }

    #[test]
    fn test_drop_index_serialization() {
        let index_id = ObjectId::generate();
        let op = DropIndexOp { id: index_id };

        let json = serde_json::to_string(&op).unwrap();
        let deserialized: DropIndexOp = serde_json::from_str(&json).unwrap();

        assert_eq!(op, deserialized);
    }
}
