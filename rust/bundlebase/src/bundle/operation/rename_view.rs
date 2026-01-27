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
pub struct RenameViewOp {
    pub id: ObjectId,
    pub new_name: String,
}

impl RenameViewOp {
    pub async fn setup(
        old_name: &str,
        new_name: &str,
        builder: &BundleBuilder,
    ) -> Result<Self, BundlebaseError> {
        let bundle = builder.bundle();
        // Look up the view ID from the old name
        let views = bundle.views.read();
        let view_id = *views
            .get(old_name)
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
                    old_name, available_list
                ))
            })?;

        Ok(Self {
            id: view_id,
            new_name: new_name.to_string(),
        })
    }
}

#[async_trait]
impl Operation for RenameViewOp {
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        let views = bundle.views.read();

        // Check that the view id exists
        let view_exists = views.values().any(|id| id == &self.id);
        if !view_exists {
            return Err(format!("View with ID '{}' not found", self.id).into());
        }

        // Check that new_name doesn't already exist
        if views.contains_key(&self.new_name) {
            return Err(format!("View '{}' already exists", self.new_name).into());
        }

        Ok(())
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        let mut views = bundle.views.write();

        // Find and remove the old name->id mapping
        let old_name = views
            .iter()
            .find(|(_, id)| *id == &self.id)
            .map(|(name, _)| name.clone());

        if let Some(old_name) = old_name {
            views.remove(&old_name);
        }

        // Insert new name->id mapping
        views.insert(self.new_name.clone(), self.id);

        Ok(())
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        _ctx: Arc<SessionContext>,
    ) -> Result<DataFrame, BundlebaseError> {
        // RenameViewOp doesn't modify the dataframe (metadata-only operation)
        Ok(df)
    }

    fn describe(&self) -> String {
        format!("RENAME VIEW to '{}'", self.new_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let op = RenameViewOp {
            id: ObjectId::generate(),
            new_name: "new_view_name".to_string(),
        };
        assert_eq!(op.describe(), "RENAME VIEW to 'new_view_name'");
    }

    #[test]
    fn test_describe_multiple_cases() {
        let cases = vec![
            ("adults", "RENAME VIEW to 'adults'"),
            ("customers_filtered", "RENAME VIEW to 'customers_filtered'"),
            ("v2", "RENAME VIEW to 'v2'"),
        ];

        for (new_name, expected) in cases {
            let op = RenameViewOp {
                id: ObjectId::generate(),
                new_name: new_name.to_string(),
            };
            assert_eq!(op.describe(), expected);
        }
    }

    #[test]
    fn test_serialization() {
        let view_id: ObjectId = "a5".try_into().unwrap();
        let op = RenameViewOp {
            id: view_id,
            new_name: "new_view".to_string(),
        };

        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        let expected = format!("id: {}\nnewName: new_view\n", view_id);
        assert_eq!(serialized, expected);
    }

    #[test]
    fn test_deserialization() {
        let view_id_str = "a5";
        let yaml = format!("id: {}\nnewName: renamed_view\n", view_id_str);

        let op: RenameViewOp = serde_yaml_ng::from_str(&yaml).expect("Failed to deserialize");

        assert_eq!(op.id.to_string(), view_id_str);
        assert_eq!(op.new_name, "renamed_view");
    }
}
