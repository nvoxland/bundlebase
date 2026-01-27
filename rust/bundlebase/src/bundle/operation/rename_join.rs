use crate::bundle::operation::Operation;
use crate::bundle::BundleBuilder;
use crate::data::ObjectId;
use crate::{Bundle, BundlebaseError};
use async_trait::async_trait;
use datafusion::common::DataFusionError;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RenameJoinOp {
    pub id: ObjectId,
    pub new_name: String,
}

impl RenameJoinOp {
    pub async fn setup(
        old_name: &str,
        new_name: &str,
        builder: &BundleBuilder,
    ) -> Result<Self, BundlebaseError> {
        let bundle = builder.bundle();
        // Look up the pack by old name
        let pack = bundle.pack_by_name(old_name).ok_or_else(|| {
            let available_joins: Vec<String> = bundle
                .packs()
                .read()
                .iter()
                .filter(|(_, p)| p.is_join())
                .map(|(_, p)| p.name().to_string())
                .collect();
            let available_list = if available_joins.is_empty() {
                "none".to_string()
            } else {
                available_joins.join(", ")
            };
            BundlebaseError::from(format!(
                "Join '{}' not found. Available joins: {}",
                old_name, available_list
            ))
        })?;

        // Verify it's a join pack (not the base pack)
        if !pack.is_join() {
            return Err(format!("'{}' is not a join pack", old_name).into());
        }

        // Check that new_name doesn't already exist
        if bundle.pack_by_name(new_name).is_some() {
            return Err(format!("A pack with name '{}' already exists", new_name).into());
        }

        Ok(Self {
            id: *pack.id(),
            new_name: new_name.to_string(),
        })
    }
}

#[async_trait]
impl Operation for RenameJoinOp {
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Check that the pack id exists
        let pack = bundle.get_pack(&self.id);
        if pack.is_none() {
            return Err(format!("Pack with ID '{}' not found", self.id).into());
        }

        // Check that new_name doesn't already exist
        if bundle.pack_by_name(&self.new_name).is_some() {
            return Err(format!("A pack with name '{}' already exists", self.new_name).into());
        }

        Ok(())
    }

    fn allowed_on_view(&self) -> bool {
        false
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        // Clone the pack, update name, replace in HashMap
        let old_pack = bundle
            .get_pack(&self.id)
            .expect("Pack must exist - check() already verified");
        let mut new_pack = (*old_pack).clone();
        new_pack.set_name(&self.new_name);
        bundle.add_pack(self.id, Arc::new(new_pack));

        Ok(())
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        _ctx: Arc<SessionContext>,
    ) -> Result<DataFrame, BundlebaseError> {
        // RenameJoinOp doesn't modify the dataframe (metadata-only operation)
        Ok(df)
    }

    fn describe(&self) -> String {
        format!("RENAME JOIN to '{}'", self.new_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let op = RenameJoinOp {
            id: ObjectId::generate(),
            new_name: "new_join_name".to_string(),
        };
        assert_eq!(op.describe(), "RENAME JOIN to 'new_join_name'");
    }

    #[test]
    fn test_describe_multiple_cases() {
        let cases = vec![
            ("customers", "RENAME JOIN to 'customers'"),
            ("users_filtered", "RENAME JOIN to 'users_filtered'"),
            ("j2", "RENAME JOIN to 'j2'"),
        ];

        for (new_name, expected) in cases {
            let op = RenameJoinOp {
                id: ObjectId::generate(),
                new_name: new_name.to_string(),
            };
            assert_eq!(op.describe(), expected);
        }
    }

    #[test]
    fn test_serialization() {
        let pack_id: ObjectId = "a5".try_into().unwrap();
        let op = RenameJoinOp {
            id: pack_id,
            new_name: "new_join".to_string(),
        };

        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        let expected = format!("id: {}\nnewName: new_join\n", pack_id);
        assert_eq!(serialized, expected);
    }

    #[test]
    fn test_deserialization() {
        let pack_id_str = "a5";
        let yaml = format!("id: {}\nnewName: renamed_join\n", pack_id_str);

        let op: RenameJoinOp = serde_yaml_ng::from_str(&yaml).expect("Failed to deserialize");

        assert_eq!(op.id.to_string(), pack_id_str);
        assert_eq!(op.new_name, "renamed_join");
    }
}
