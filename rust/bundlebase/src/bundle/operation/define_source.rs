use crate::bundle::operation::Operation;
use crate::data::ObjectId;
use crate::{Bundle, BundlebaseError};
use async_trait::async_trait;
use datafusion::error::DataFusionError;
use serde::{Deserialize, Serialize};

/// Operation that defines a data source for a pack.
///
/// A source specifies where to look for data files (e.g., S3 bucket prefix)
/// and patterns to filter which files to include. This enables the `refresh()`
/// functionality to discover and auto-attach new files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DefineSourceOp {
    /// Unique identifier for this source
    pub id: ObjectId,

    /// The pack this source is associated with
    pub pack_id: ObjectId,

    /// URL prefix for file discovery (e.g., "s3://bucket/data/")
    pub url: String,

    /// Glob patterns for filtering files. Defaults to ["**/*"] (all files).
    /// Examples:
    /// - ["**/*"] - all files recursively
    /// - ["**/*.parquet"] - all parquet files recursively
    /// - ["2024/**/*.csv"] - CSV files in 2024 directory
    pub patterns: Vec<String>,
}

impl DefineSourceOp {
    pub fn setup(
        id: ObjectId,
        pack_id: ObjectId,
        url: String,
        patterns: Option<Vec<String>>,
    ) -> Self {
        Self {
            id,
            pack_id,
            url,
            patterns: patterns.unwrap_or_else(|| vec!["**/*".to_string()]),
        }
    }
}

#[async_trait]
impl Operation for DefineSourceOp {
    fn describe(&self) -> String {
        format!("DEFINE SOURCE {} at {} for pack {}", self.id, self.url, self.pack_id)
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Verify pack exists
        if bundle.get_pack(&self.pack_id).is_none() {
            return Err(format!("Pack {} not found", self.pack_id).into());
        }

        // Verify no source already defined for this pack
        if bundle.get_source_for_pack(&self.pack_id).is_some() {
            return Err(format!("Pack {} already has a source defined", self.pack_id).into());
        }

        Ok(())
    }

    fn allowed_on_view(&self) -> bool {
        false
    }

    async fn apply(&self, bundle: &mut Bundle) -> Result<(), DataFusionError> {
        bundle.add_source(self.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let op = DefineSourceOp {
            id: ObjectId::from(1),
            pack_id: ObjectId::from(2),
            url: "s3://bucket/data/".to_string(),
            patterns: vec!["**/*.parquet".to_string()],
        };

        assert_eq!(
            op.describe(),
            "DEFINE SOURCE 01 at s3://bucket/data/ for pack 02"
        );
    }

    #[test]
    fn test_setup_default_patterns() {
        let op = DefineSourceOp::setup(
            ObjectId::from(1),
            ObjectId::from(2),
            "s3://bucket/".to_string(),
            None,
        );

        assert_eq!(op.patterns, vec!["**/*".to_string()]);
    }

    #[test]
    fn test_setup_custom_patterns() {
        let op = DefineSourceOp::setup(
            ObjectId::from(1),
            ObjectId::from(2),
            "s3://bucket/".to_string(),
            Some(vec!["**/*.parquet".to_string(), "**/*.csv".to_string()]),
        );

        assert_eq!(
            op.patterns,
            vec!["**/*.parquet".to_string(), "**/*.csv".to_string()]
        );
    }

    #[test]
    fn test_serialization() {
        let op = DefineSourceOp {
            id: ObjectId::from(1),
            pack_id: ObjectId::from(2),
            url: "s3://bucket/data/".to_string(),
            patterns: vec!["**/*.parquet".to_string()],
        };

        let yaml = serde_yaml::to_string(&op).unwrap();
        assert!(yaml.contains("id: '01'"));
        assert!(yaml.contains("packId: '02'"));
        assert!(yaml.contains("url: s3://bucket/data/"));
        assert!(yaml.contains("'**/*.parquet'"));
    }
}
