use crate::bundle::operation::{AnyOperation, Operation};
use crate::bundle::DataBlock;
use crate::data::ObjectId;
use crate::source::AttachedFileInfo;
use crate::{Bundle, BundleBuilder, BundlebaseError};
use async_trait::async_trait;
use datafusion::common::DataFusionError;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Operation to replace a block's location in a bundle.
///
/// This operation changes where a block's data is read from without
/// changing the block's identity. Useful when data files are moved
/// to a new location.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceBlockOp {
    /// The block ID to replace
    pub id: ObjectId,
    /// The new location to read data from
    pub new_location: String,
}

impl ReplaceBlockOp {
    /// Create a ReplaceBlockOp by looking up the block ID from the old location.
    ///
    /// Searches through AttachBlockOp operations to find a block with
    /// the matching location.
    pub async fn setup(
        old_location: &str,
        new_location: &str,
        builder: &BundleBuilder,
    ) -> Result<Self, BundlebaseError> {
        // Find block ID by searching AttachBlockOp operations for matching location
        let block_id = builder
            .bundle
            .operations
            .iter()
            .find_map(|op| {
                if let AnyOperation::AttachBlock(attach_op) = op {
                    if attach_op.location == old_location {
                        return Some(attach_op.id.clone());
                    }
                }
                None
            })
            .ok_or_else(|| {
                BundlebaseError::from(format!("No block found at location '{}'", old_location))
            })?;

        Ok(Self {
            id: block_id,
            new_location: new_location.to_string(),
        })
    }

    /// Find the block in any pack within the bundle.
    fn find_block_in_packs(&self, bundle: &Bundle) -> Option<(ObjectId, Arc<DataBlock>)> {
        for (pack_id, pack) in &*bundle.data_packs.read() {
            for block in pack.blocks() {
                if block.id() == &self.id {
                    return Some((pack_id.clone(), block.clone()));
                }
            }
        }
        None
    }
}

#[async_trait]
impl Operation for ReplaceBlockOp {
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Check that the block exists in some pack
        if self.find_block_in_packs(bundle).is_none() {
            return Err(format!("Block with ID '{}' not found in any pack", self.id).into());
        }
        Ok(())
    }

    fn allowed_on_view(&self) -> bool {
        false
    }

    async fn apply(&self, bundle: &mut Bundle) -> Result<(), DataFusionError> {
        // Find the block and its pack
        let (pack_id, old_block) = self
            .find_block_in_packs(bundle)
            .ok_or_else(|| DataFusionError::Execution(format!("Block {} not found", self.id)))?;

        // Preserve source info from old block
        let source = old_block.source().cloned();
        let source_location = old_block.source_location().map(|s| s.to_string());

        // Create a new reader for the new location
        let reader = bundle
            .adapter_factory
            .reader(
                &self.new_location,
                &self.id,
                bundle,
                Some(old_block.schema()),
                None, // Layout will be rebuilt if needed
            )
            .await?;

        // Create a new block with the new reader (preserving source info)
        let new_block = Arc::new(DataBlock::new(
            self.id.clone(),
            old_block.schema(),
            &old_block.version(),
            reader,
            bundle.indexes().clone(),
            bundle.data_dir_arc(),
            bundle.config(),
            source.clone(),
            source_location.clone(),
        ));

        // Replace the old block with the new one in the pack
        let pack = bundle
            .data_packs
            .read()
            .get(&pack_id)
            .cloned()
            .ok_or_else(|| DataFusionError::Execution(format!("Pack {} not found", pack_id)))?;

        pack.remove_block(&self.id);
        pack.add_block(new_block.clone());

        // Update source's attached_files with the new location
        if let Some(source_id) = &source {
            if let Some(src) = bundle.get_source(source_id) {
                if let Some(source_loc) = &source_location {
                    src.update_attached_file(
                        source_loc,
                        AttachedFileInfo {
                            location: self.new_location.clone(),
                            version: old_block.version(),
                            bytes: None, // Could read from adapter if needed
                        },
                    );
                }
            }
        }

        log::info!(
            "Replaced block {} location to {}",
            self.id,
            self.new_location
        );

        Ok(())
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        _ctx: Arc<SessionContext>,
    ) -> Result<DataFrame, BundlebaseError> {
        // ReplaceBlockOp doesn't modify the dataframe (metadata-only operation)
        Ok(df)
    }

    fn describe(&self) -> String {
        format!("REPLACE BLOCK {} -> {}", self.id, self.new_location)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let block_id = ObjectId::generate();
        let op = ReplaceBlockOp {
            id: block_id.clone(),
            new_location: "s3://bucket/new_data.parquet".to_string(),
        };
        assert_eq!(
            op.describe(),
            format!("REPLACE BLOCK {} -> s3://bucket/new_data.parquet", block_id)
        );
    }

    #[test]
    fn test_serialization() {
        let block_id: ObjectId = "a5".try_into().unwrap();
        let op = ReplaceBlockOp {
            id: block_id.clone(),
            new_location: "file:///new/path.csv".to_string(),
        };

        let serialized = serde_yaml::to_string(&op).expect("Failed to serialize");
        assert!(serialized.contains("id: a5"));
        assert!(serialized.contains("newLocation: file:///new/path.csv"));
    }

    #[test]
    fn test_deserialization() {
        let yaml = "id: a5\nnewLocation: file:///new/path.csv\n";

        let op: ReplaceBlockOp = serde_yaml::from_str(yaml).expect("Failed to deserialize");

        assert_eq!(op.id.to_string(), "a5");
        assert_eq!(op.new_location, "file:///new/path.csv");
    }
}
