use crate::bundle::operation::{AnyOperation, Operation};
use crate::bundle::BundleBuilder;
use crate::data::ObjectId;
use crate::{Bundle, BundleFacade, BundlebaseError};
use async_trait::async_trait;
use datafusion::common::DataFusionError;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Operation to detach (remove) a block from a bundle.
///
/// This operation removes a previously attached block from the bundle.
/// The block is identified by its ID, which is looked up from the location
/// during setup.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DetachBlockOp {
    pub id: ObjectId,
}

impl DetachBlockOp {
    /// Create a DetachBlockOp by looking up the block ID from the location.
    ///
    /// Searches through AttachBlockOp operations to find a block with
    /// the matching location.
    pub async fn setup(location: &str, builder: &BundleBuilder) -> Result<Self, BundlebaseError> {
        // Find block ID by searching AttachBlockOp operations for matching location
        let block_id = builder
            .operations()
            .iter()
            .find_map(|op| {
                if let AnyOperation::AttachBlock(attach_op) = op {
                    if attach_op.location == location {
                        return Some(attach_op.id);
                    }
                }
                None
            })
            .ok_or_else(|| {
                BundlebaseError::from(format!("No block found at location '{}'", location))
            })?;

        Ok(Self { id: block_id })
    }

    /// Find the block in any pack within the bundle.
    fn find_block_in_packs(&self, bundle: &Bundle) -> bool {
        for pack in bundle.packs().read().values() {
            for block in pack.blocks() {
                if block.id() == &self.id {
                    return true;
                }
            }
        }
        false
    }
}

#[async_trait]
impl Operation for DetachBlockOp {
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Check that the block exists in some pack
        if !self.find_block_in_packs(bundle) {
            return Err(format!("Block with ID '{}' not found in any pack", self.id).into());
        }
        Ok(())
    }

    fn allowed_on_view(&self) -> bool {
        false
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        // Look up block's source info before removing
        if let Some(block) = bundle.find_block(&self.id) {
            if let Some(source_info) = block.source_info() {
                if let Some(source) = bundle.get_source(&source_info.id) {
                    source.remove_attached_file(&source_info.location);
                }
            }
        }

        // Remove the block from all packs
        for pack in bundle.packs().read().values() {
            pack.remove_block(&self.id);
        }

        log::info!("Detached block {}", self.id);

        Ok(())
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        _ctx: Arc<SessionContext>,
    ) -> Result<DataFrame, BundlebaseError> {
        // DetachBlockOp doesn't modify the dataframe (metadata-only operation)
        Ok(df)
    }

    fn describe(&self) -> String {
        format!("DETACH BLOCK: {}", self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let block_id = ObjectId::generate();
        let op = DetachBlockOp { id: block_id };
        assert_eq!(op.describe(), format!("DETACH BLOCK: {}", block_id));
    }

    #[test]
    fn test_serialization() {
        let block_id: ObjectId = "a5".try_into().unwrap();
        let op = DetachBlockOp { id: block_id };

        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        let expected = format!("id: {}\n", block_id);
        assert_eq!(serialized, expected);
    }

    #[test]
    fn test_deserialization() {
        let block_id_str = "a5";
        let yaml = format!("id: {}\n", block_id_str);

        let op: DetachBlockOp = serde_yaml_ng::from_str(&yaml).expect("Failed to deserialize");

        assert_eq!(op.id.to_string(), block_id_str);
    }
}
