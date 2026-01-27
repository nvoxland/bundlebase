use crate::bundle::operation::Operation;
use crate::bundle::DataBlock;
use crate::data::ObjectId;
use crate::{Bundle, BundlebaseError};
use async_trait::async_trait;
use datafusion::common::DataFusionError;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Operation to update a block's version metadata.
///
/// This operation is used when verify_data finds that a block's hash matches
/// but its version has changed (e.g., file was touched but content unchanged).
/// It updates the stored version to match the current file state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateVersionOp {
    /// The block ID to update
    pub block: ObjectId,
    /// The new version to store
    pub new_version: String,
}

impl UpdateVersionOp {
    pub fn setup(block: ObjectId, new_version: String) -> Self {
        Self { block, new_version }
    }

    /// Find the block in any pack within the bundle.
    fn find_block_in_packs(&self, bundle: &Bundle) -> Option<(ObjectId, Arc<DataBlock>)> {
        for (pack_id, pack) in &*bundle.packs().read() {
            for block in pack.blocks() {
                if block.id() == &self.block {
                    return Some((*pack_id, block.clone()));
                }
            }
        }
        None
    }
}

#[async_trait]
impl Operation for UpdateVersionOp {
    fn describe(&self) -> String {
        format!("UPDATE VERSION: block {} -> {}", self.block, self.new_version)
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Verify block exists
        if self.find_block_in_packs(bundle).is_none() {
            return Err(format!("Block with ID '{}' not found in any pack", self.block).into());
        }
        Ok(())
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        // Find the block and its pack
        let (pack_id, old_block) = self
            .find_block_in_packs(bundle)
            .ok_or_else(|| DataFusionError::Execution(format!("Block {} not found", self.block)))?;

        // Create a new block with the updated version
        let new_block = Arc::new(DataBlock::new(
            self.block,
            old_block.schema(),
            &self.new_version,
            old_block.reader(),
            bundle.indexes().clone(),
            bundle.data_dir(),
            bundle.config(),
            old_block.source_info().cloned(),
        ));

        // Replace the old block with the new one in the pack
        let pack = bundle
            .packs()
            .read()
            .get(&pack_id)
            .cloned()
            .ok_or_else(|| DataFusionError::Execution(format!("Pack {} not found", pack_id)))?;

        pack.remove_block(&self.block);
        pack.add_block(new_block);

        log::info!(
            "Updated version for block {} to {}",
            self.block,
            self.new_version
        );

        Ok(())
    }

    fn allowed_on_view(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let block_id = ObjectId::generate();
        let op = UpdateVersionOp::setup(block_id, "etag:new123".to_string());
        assert_eq!(
            op.describe(),
            format!("UPDATE VERSION: block {} -> etag:new123", block_id)
        );
    }

    #[test]
    fn test_serialization() {
        let block_id: ObjectId = "a5".try_into().expect("Failed to create ObjectId");
        let op = UpdateVersionOp::setup(block_id, "etag:abc123".to_string());

        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        assert!(serialized.contains("block: a5"));
        assert!(serialized.contains("newVersion: 'etag:abc123'") || serialized.contains("newVersion: etag:abc123"));
    }

    #[test]
    fn test_deserialization() {
        let yaml = r#"block: a5
newVersion: 'etag:abc123'
"#;

        let op: UpdateVersionOp = serde_yaml_ng::from_str(yaml).expect("Failed to deserialize");

        assert_eq!(op.block.to_string(), "a5");
        assert_eq!(op.new_version, "etag:abc123");
    }
}
