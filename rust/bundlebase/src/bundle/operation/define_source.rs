use crate::bundle::operation::Operation;
use crate::data::ObjectId;
use crate::{Bundle, BundlebaseError};
use async_trait::async_trait;
use datafusion::error::DataFusionError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Operation that defines a data source for a pack.
///
/// A source specifies where to look for data files and enables the `refresh()`
/// functionality to discover and auto-attach new files.
///
/// The source function is responsible for file discovery. Each function may require
/// different arguments. For example, "data_directory" requires:
/// - "url": Directory URL to list (e.g., "s3://bucket/data/")
/// - "patterns": Comma-separated glob patterns (e.g., "**/*.parquet,**/*.csv")
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DefineSourceOp {
    /// Unique identifier for this source
    pub id: ObjectId,

    /// The pack this source is associated with
    pub pack: ObjectId,

    /// Source function name (e.g., "data_directory")
    pub function: String,

    /// Function-specific configuration arguments.
    /// For "data_directory":
    /// - "url": Directory URL (required)
    /// - "patterns": Comma-separated glob patterns (optional, defaults to "**/*")
    #[serde(default)]
    pub args: HashMap<String, String>,
}

impl DefineSourceOp {
    pub fn setup(
        id: ObjectId,
        pack: ObjectId,
        function: String,
        args: HashMap<String, String>,
    ) -> Self {
        Self {
            id,
            pack,
            function,
            args,
        }
    }
}

#[async_trait]
impl Operation for DefineSourceOp {
    fn describe(&self) -> String {
        let url = self.args.get("url").map(|s| s.as_str()).unwrap_or("<no url>");
        format!("DEFINE SOURCE {} at {} for pack {}", self.id, url, self.pack)
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Verify pack exists
        if bundle.get_pack(&self.pack).is_none() {
            return Err(format!("Pack {} not found", self.pack).into());
        }

        // Verify no source already defined for this pack
        if bundle.get_source_for_pack(&self.pack).is_some() {
            return Err(format!("Pack {} already has a source defined", self.pack).into());
        }

        // Verify function exists and validate arguments
        let registry = bundle.source_function_registry();
        let func = registry
            .read()
            .get(&self.function)
            .ok_or_else(|| format!("Unknown source function '{}'", self.function))?;

        func.validate_args(&self.args)?;

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

    fn make_args(url: &str, patterns: Option<&str>) -> HashMap<String, String> {
        let mut args = HashMap::new();
        args.insert("url".to_string(), url.to_string());
        if let Some(p) = patterns {
            args.insert("patterns".to_string(), p.to_string());
        }
        args
    }

    #[test]
    fn test_describe() {
        let op = DefineSourceOp {
            id: ObjectId::from(1),
            pack: ObjectId::from(2),
            function: "data_directory".to_string(),
            args: make_args("s3://bucket/data/", Some("**/*.parquet")),
        };

        assert_eq!(
            op.describe(),
            "DEFINE SOURCE 01 at s3://bucket/data/ for pack 02"
        );
    }

    #[test]
    fn test_describe_no_url() {
        let op = DefineSourceOp {
            id: ObjectId::from(1),
            pack: ObjectId::from(2),
            function: "custom_function".to_string(),
            args: HashMap::new(),
        };

        assert_eq!(
            op.describe(),
            "DEFINE SOURCE 01 at <no url> for pack 02"
        );
    }

    #[test]
    fn test_setup() {
        let op = DefineSourceOp::setup(
            ObjectId::from(1),
            ObjectId::from(2),
            "data_directory".to_string(),
            make_args("s3://bucket/", None),
        );

        assert_eq!(op.function, "data_directory");
        assert_eq!(op.args.get("url"), Some(&"s3://bucket/".to_string()));
    }

    #[test]
    fn test_setup_with_patterns() {
        let op = DefineSourceOp::setup(
            ObjectId::from(1),
            ObjectId::from(2),
            "data_directory".to_string(),
            make_args("s3://bucket/", Some("**/*.parquet,**/*.csv")),
        );

        assert_eq!(op.function, "data_directory");
        assert_eq!(
            op.args.get("patterns"),
            Some(&"**/*.parquet,**/*.csv".to_string())
        );
    }

    #[test]
    fn test_setup_with_extra_args() {
        let mut args = make_args("s3://bucket/", Some("**/*"));
        args.insert("key".to_string(), "value".to_string());

        let op = DefineSourceOp::setup(
            ObjectId::from(1),
            ObjectId::from(2),
            "custom_function".to_string(),
            args.clone(),
        );

        assert_eq!(op.function, "custom_function");
        assert_eq!(op.args, args);
    }

    #[test]
    fn test_serialization() {
        let op = DefineSourceOp {
            id: ObjectId::from(1),
            pack: ObjectId::from(2),
            function: "data_directory".to_string(),
            args: make_args("s3://bucket/data/", Some("**/*.parquet")),
        };

        let yaml = serde_yaml::to_string(&op).unwrap();
        assert!(yaml.contains("id: '01'"));
        assert!(yaml.contains("pack: '02'"));
        assert!(yaml.contains("function: data_directory"));
        assert!(yaml.contains("url: s3://bucket/data/"));
        assert!(yaml.contains("patterns: '**/*.parquet'"));
    }
}
