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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DropJoinOp {
    pub id: ObjectId,
}

impl DropJoinOp {
    pub async fn setup(join_name: &str, builder: &BundleBuilder) -> Result<Self, BundlebaseError> {
        let bundle = builder.bundle();
        let pack = bundle.pack_by_name(join_name).ok_or_else(|| {
            let available_joins = bundle.join_names();
            let available_list = if available_joins.is_empty() {
                "none".to_string()
            } else {
                available_joins.join(", ")
            };
            BundlebaseError::from(format!(
                "Join '{}' not found. Available joins: {}",
                join_name, available_list
            ))
        })?;
        Ok(Self { id: *pack.id() })
    }
}

#[async_trait]
impl Operation for DropJoinOp {
    fn describe(&self) -> String {
        format!("DROP JOIN {}", self.id)
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Check if a pack with this ID exists and is a join pack
        let pack = bundle.get_pack(&self.id);
        match pack {
            Some(p) if p.is_join() => Ok(()),
            Some(_) => Err(format!("Pack '{}' is not a join pack", self.id).into()),
            None => Err(format!("Join pack with ID '{}' not found", self.id).into()),
        }
    }

    fn allowed_on_view(&self) -> bool {
        false
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        bundle.packs().write().remove(&self.id);
        log::info!("Dropped join with pack {}", self.id);
        Ok(())
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        _ctx: Arc<SessionContext>,
    ) -> Result<DataFrame, BundlebaseError> {
        Ok(df)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let pack_id = ObjectId::generate();
        let op = DropJoinOp { id: pack_id };
        assert_eq!(op.describe(), format!("DROP JOIN {}", pack_id));
    }

    #[test]
    fn test_serialization() {
        let pack_id: ObjectId = "a5".try_into().unwrap();
        let op = DropJoinOp { id: pack_id };

        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        let expected = format!("id: {}\n", pack_id);
        assert_eq!(serialized, expected);
    }

    #[test]
    fn test_deserialization() {
        let pack_id_str = "a5";
        let yaml = format!("id: {}\n", pack_id_str);

        let op: DropJoinOp = serde_yaml_ng::from_str(&yaml).expect("Failed to deserialize");

        assert_eq!(op.id.to_string(), pack_id_str);
    }
}
