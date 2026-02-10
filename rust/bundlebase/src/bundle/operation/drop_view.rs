use crate::bundle::operation::Operation;
use crate::bundle::BundleBuilder;
use crate::bundle::BundleFacade;
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
pub struct DropViewOp {
    pub id: ObjectId,
}

impl DropViewOp {
    pub async fn setup(view_name: &str, builder: &BundleBuilder) -> Result<Self, BundlebaseError> {
        // Look up the view ID from the name
        let views = builder.views_by_name();
        let view_id = *views
            .get(view_name)
            .ok_or_else(|| {
                let available_views: Vec<String> = views
                    .iter()
                    .map(|(name, id)| format!("{} ({})", name, id))
                    .collect();
                let available_list = if available_views.is_empty() {
                    "none".to_string()
                } else {
                    available_views.join(", ")
                };
                BundlebaseError::from(format!(
                    "View '{}' not found. Available views: {}",
                    view_name, available_list
                ))
            })?;

        Ok(Self { id: view_id })
    }
}

#[async_trait]
impl Operation for DropViewOp {
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Check that the view id exists
        let view_exists = bundle.views.read().values().any(|id| id == &self.id);
        if !view_exists {
            return Err(format!("View with ID '{}' not found", self.id).into());
        }

        Ok(())
    }

    fn allowed_on_view(&self) -> bool {
        false
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        // Find and remove the name->id mapping
        bundle.views.write().retain(|_, id| id != &self.id);

        log::info!("Dropped view {}", self.id);

        Ok(())
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        _ctx: Arc<SessionContext>,
    ) -> Result<DataFrame, BundlebaseError> {
        // DropViewOp doesn't modify the dataframe (metadata-only operation)
        Ok(df)
    }

    fn describe(&self) -> String {
        format!("DROP VIEW {}", self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let view_id = ObjectId::generate();
        let op = DropViewOp { id: view_id };
        assert_eq!(op.describe(), format!("DROP VIEW {}", view_id));
    }

    #[test]
    fn test_serialization() {
        let view_id: ObjectId = "a5".try_into().unwrap();
        let op = DropViewOp { id: view_id };

        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        let expected = format!("id: {}\n", view_id);
        assert_eq!(serialized, expected);
    }

    #[test]
    fn test_deserialization() {
        let view_id_str = "a5";
        let yaml = format!("id: {}\n", view_id_str);

        let op: DropViewOp = serde_yaml_ng::from_str(&yaml).expect("Failed to deserialize");

        assert_eq!(op.id.to_string(), view_id_str);
    }
}
