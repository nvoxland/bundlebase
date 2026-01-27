use crate::bundle::operation::{AnyOperation, Operation, SourceInfo};
use crate::bundle::BundleFacade;
use crate::bundle::DataBlock;
use crate::data::ObjectId;
use crate::io::readable_file_from_path;
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
    /// The version at the new location
    pub new_version: String,
    /// SHA256 hash of the content at the new location
    pub new_hash: String,
    /// Updated source info (if the block was originally from a source)
    #[serde(rename = "source", skip_serializing_if = "Option::is_none")]
    pub source_info: Option<SourceInfo>,
}

impl ReplaceBlockOp {
    /// Create a ReplaceBlockOp by looking up the block ID from the old location.
    ///
    /// Searches through AttachBlockOp and ReplaceBlockOp operations to find a block with
    /// the matching location. Uses the most recent metadata for that block.
    /// Reads version and computes hash from the new location.
    pub async fn setup(
        old_location: &str,
        new_location: &str,
        builder: &BundleBuilder,
    ) -> Result<Self, BundlebaseError> {
        // Find block ID by searching AttachBlockOp operations for matching location
        // Also check ReplaceBlockOp in case the block was already replaced
        let (block_id, old_source_info) =
            Self::find_block_by_location(old_location, &builder.bundle().operations.read()).ok_or_else(
                || BundlebaseError::from(format!("No block found at location '{}'", old_location)),
            )?;

        // Create adapter to read version from the new location
        let temp_id = ObjectId::generate();
        let adapter_factory = builder.bundle().reader_factory.clone();
        let adapter = adapter_factory
            .reader(new_location, &temp_id, builder, None, None, None)
            .await?;
        let new_version = adapter.read_version().await?;

        // Compute hash from the new location
        // function:// URLs don't support file-based hash, use version-based hash instead
        // todo: do this right
        let new_hash = if new_location.starts_with("function://") {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(new_version.as_bytes());
            hex::encode(hasher.finalize())
        } else {
            let file = readable_file_from_path(new_location, builder.data_dir(), builder.config())?;
            file.compute_hash().await?
        };

        // Update source info with the new version (if source info exists)
        let source_info = old_source_info.map(|info| SourceInfo {
            id: info.id,
            location: info.location,
            version: new_version.clone(),
        });

        Ok(Self {
            id: block_id,
            new_location: new_location.to_string(),
            new_version,
            new_hash,
            source_info,
        })
    }

    /// Find a block by its current location, searching through both AttachBlockOp and ReplaceBlockOp.
    ///
    /// Returns the block ID and the most recent source_info for that block.
    /// For blocks that have been replaced, this finds the ReplaceBlockOp with the matching new_location
    /// and returns the updated source_info from that operation.
    fn find_block_by_location(location: &str, operations: &[AnyOperation]) -> Option<(ObjectId, Option<SourceInfo>)> {
        // First, check if any ReplaceBlockOp has this as its new_location (most recent state)
        // We iterate in reverse to find the most recent replacement
        for op in operations.iter().rev() {
            if let AnyOperation::ReplaceBlock(replace_op) = op {
                if replace_op.new_location == location {
                    return Some((replace_op.id, replace_op.source_info.clone()));
                }
            }
        }

        // If not found in ReplaceBlockOp, check AttachBlockOp
        for op in operations.iter() {
            if let AnyOperation::AttachBlock(attach_op) = op {
                if attach_op.location == location {
                    return Some((attach_op.id, attach_op.source_info.clone()));
                }
            }
        }

        None
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

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        // Find the block and its pack
        let (pack_id, old_block) = self
            .find_block_in_packs(bundle)
            .ok_or_else(|| DataFusionError::Execution(format!("Block {} not found", self.id)))?;

        // Create a new reader for the new location
        let reader = bundle
            .reader_factory
            .reader(
                &self.new_location,
                &self.id,
                bundle as &dyn BundleFacade,
                Some(old_block.schema()),
                None, // Layout will be rebuilt if needed
                Some(self.new_version.clone()), // Validate version during query execution
            )
            .await?;

        // Create a new block with the new reader and stored metadata
        let new_block = Arc::new(DataBlock::new(
            self.id,
            old_block.schema(),
            &self.new_version,
            reader,
            bundle.indexes().clone(),
            bundle.data_dir(),
            bundle.config(),
            self.source_info.clone(),
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
        if let Some(ref info) = self.source_info {
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
            new_version: "etag:abc123".to_string(),
            new_hash: "0".repeat(64),
            source_info: None,
        };
        assert_eq!(
            op.describe(),
            format!("REPLACE BLOCK {} -> s3://bucket/new_data.parquet", block_id)
        );
    }

    #[test]
    fn test_serialization_without_source() {
        let block_id: ObjectId = "a5".try_into().unwrap();
        let op = ReplaceBlockOp {
            id: block_id,
            new_location: "file:///new/path.csv".to_string(),
            new_version: "etag:abc123".to_string(),
            new_hash: "0".repeat(64),
            source_info: None,
        };

        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        assert!(serialized.contains("id: a5"));
        assert!(serialized.contains("newLocation: file:///new/path.csv"));
        assert!(serialized.contains("newVersion:"), "serialized: {}", serialized);
        assert!(serialized.contains("etag:abc123"), "serialized: {}", serialized);
        assert!(serialized.contains("newHash:"), "serialized: {}", serialized);
        assert!(serialized.contains(&"0".repeat(64)), "serialized: {}", serialized);
        // source should not appear when None
        assert!(!serialized.contains("source:"));
    }

    #[test]
    fn test_serialization_with_source() {
        let block_id: ObjectId = "a5".try_into().unwrap();
        let source_id: ObjectId = "b3".try_into().unwrap();
        let op = ReplaceBlockOp {
            id: block_id,
            new_location: "file:///new/path.csv".to_string(),
            new_version: "etag:abc123".to_string(),
            new_hash: "0".repeat(64),
            source_info: Some(SourceInfo {
                id: source_id,
                location: "original/path.csv".to_string(),
                version: "etag:abc123".to_string(),
            }),
        };

        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        assert!(serialized.contains("id: a5"));
        assert!(serialized.contains("newLocation: file:///new/path.csv"));
        assert!(serialized.contains("newVersion:"), "serialized: {}", serialized);
        assert!(serialized.contains("etag:abc123"), "serialized: {}", serialized);
        assert!(serialized.contains("newHash:"), "serialized: {}", serialized);
        assert!(serialized.contains(&"0".repeat(64)), "serialized: {}", serialized);
        assert!(serialized.contains("source:"));
        assert!(serialized.contains("location: original/path.csv"));
    }

    #[test]
    fn test_deserialization_without_source() {
        let yaml = r#"id: a5
newLocation: file:///new/path.csv
newVersion: 'etag:abc123'
newHash: 0000000000000000000000000000000000000000000000000000000000000000
"#;

        let op: ReplaceBlockOp = serde_yaml_ng::from_str(yaml).expect("Failed to deserialize");

        assert_eq!(op.id.to_string(), "a5");
        assert_eq!(op.new_location, "file:///new/path.csv");
        assert_eq!(op.new_version, "etag:abc123");
        assert_eq!(op.new_hash, "0".repeat(64));
        assert!(op.source_info.is_none());
    }

    #[test]
    fn test_deserialization_with_source() {
        let yaml = r#"id: a5
newLocation: file:///new/path.csv
newVersion: 'etag:abc123'
newHash: 0000000000000000000000000000000000000000000000000000000000000000
source:
  id: b3
  location: original/path.csv
  version: 'etag:abc123'
"#;

        let op: ReplaceBlockOp = serde_yaml_ng::from_str(yaml).expect("Failed to deserialize");

        assert_eq!(op.id.to_string(), "a5");
        assert_eq!(op.new_location, "file:///new/path.csv");
        assert_eq!(op.new_version, "etag:abc123");
        assert_eq!(op.new_hash, "0".repeat(64));
        assert!(op.source_info.is_some());
        let source = op.source_info.unwrap();
        assert_eq!(source.id.to_string(), "b3");
        assert_eq!(source.location, "original/path.csv");
        assert_eq!(source.version, "etag:abc123");
    }
}
