use crate::bundle::operation::Operation;
use crate::bundle::{Bundle, BundleFacade};
use crate::data_storage::ObjectId;
use crate::BundlebaseError;
use async_trait::async_trait;
use datafusion::error::DataFusionError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DropIndexOp {
    pub index_id: ObjectId,
}

impl DropIndexOp {
    pub async fn setup(index_id: &ObjectId) -> Result<Self, BundlebaseError> {
        Ok(Self {
            index_id: index_id.clone(),
        })
    }
}

#[async_trait]
impl Operation for DropIndexOp {
    fn describe(&self) -> String {
        format!("DROP INDEX {}", self.index_id)
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Verify index exists
        let indexes = bundle.indexes().read();
        if !indexes.iter().any(|idx| idx.id() == &self.index_id) {
            return Err(format!("Index with ID '{}' not found", self.index_id).into());
        }

        Ok(())
    }

    async fn apply(&self, bundle: &mut Bundle) -> Result<(), DataFusionError> {
        // Find the index definition
        let index_def = {
            let indexes = bundle.indexes().read();
            indexes
                .iter()
                .find(|idx| idx.id() == &self.index_id)
                .cloned()
        };

        if let Some(index_def) = index_def {
            // Collect all index file paths before removing the index
            let mut index_file_paths = Vec::new();
            {
                let blocks = index_def.all_indexed_blocks();
                for indexed_blocks in blocks.iter() {
                    index_file_paths.push(indexed_blocks.path().to_string());
                }
            }

            // Delete physical index files
            for path in index_file_paths {
                match bundle.data_dir.file(&path) {
                    Ok(file) => {
                        if let Err(e) = file.delete().await {
                            log::warn!(
                                "Failed to delete index file '{}' for index {}: {}",
                                path,
                                self.index_id,
                                e
                            );
                            // Continue deletion even if one file fails
                        } else {
                            log::debug!("Deleted index file: {}", path);
                        }
                    }
                    Err(e) => {
                        log::warn!(
                            "Failed to get file handle for index file '{}': {}",
                            path,
                            e
                        );
                    }
                }
            }

            // Remove index definition from bundle
            bundle.indexes.write().retain(|idx| idx.id() != &self.index_id);

            log::info!("Dropped index {}", self.index_id);
        } else {
            log::warn!(
                "IndexDefinition {} not found when applying DropIndexOp",
                self.index_id
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_drop_index_describe() {
        let index_id = ObjectId::generate();
        let op = DropIndexOp {
            index_id: index_id.clone(),
        };

        assert_eq!(op.describe(), format!("DROP INDEX {}", index_id));
    }

    #[test]
    fn test_drop_index_serialization() {
        let index_id = ObjectId::generate();
        let op = DropIndexOp { index_id };

        let json = serde_json::to_string(&op).unwrap();
        let deserialized: DropIndexOp = serde_json::from_str(&json).unwrap();

        assert_eq!(op, deserialized);
    }
}
