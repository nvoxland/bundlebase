use crate::bundle::operation::{AnyOperation, Operation, SourceInfo};
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
                        return Some(attach_op.id);
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
        for (pack_id, pack) in &*bundle.packs().read() {
            for block in pack.blocks() {
                if block.id() == &self.id {
                    return Some((*pack_id, block.clone()));
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

        // Get the new version from the reader
        let new_version = old_block.version();

        // Update source info with the new version
        let source_info = old_block.source_info().map(|info| SourceInfo {
            id: info.id,
            location: info.location.clone(),
            version: new_version.clone(),
        });

        // Create a new block with the new reader and updated source info
        let new_block = Arc::new(DataBlock::new(
            self.id,
            old_block.schema(),
            &new_version,
            reader,
            bundle.indexes().clone(),
            bundle.data_dir_arc(),
            bundle.config(),
            source_info.clone(),
        ));

        // Replace the old block with the new one in the pack
        let pack = bundle
            .packs()
            .read()
            .get(&pack_id)
            .cloned()
            .ok_or_else(|| DataFusionError::Execution(format!("Pack {} not found", pack_id)))?;

        pack.remove_block(&self.id);
        pack.add_block(new_block.clone());

        // Update source's attached_files with the new location and version
        if let Some(ref info) = source_info {
            if let Some(src) = bundle.get_source(&info.id) {
                src.update_attached_file(
                    &info.location,
                    AttachedFileInfo {
                        location: self.new_location.clone(),
                        version: info.version.clone(),
                        bytes: None, // Could read from adapter if needed
                    },
                );
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
            id: block_id,
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
            id: block_id,
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
