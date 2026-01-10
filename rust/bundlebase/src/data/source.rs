use crate::bundle::{AnyOperation, DefineSourceOp};
use crate::data::{ObjectId, SourceFunctionRegistry};
use crate::io::ObjectStoreFile;
use crate::BundlebaseError;
use crate::BundleConfig;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Represents a data source definition for a pack.
///
/// A source specifies how to discover and list data files.
/// All configuration is stored in function-specific arguments.
#[derive(Debug, Clone)]
pub struct Source {
    id: ObjectId,
    pack_id: ObjectId,
    /// Source function name (e.g., "data_directory")
    function: String,
    /// Function-specific configuration arguments
    /// For "data_directory": "url" (required), "patterns" (optional)
    args: HashMap<String, String>,
}

impl Source {
    pub fn new(
        id: ObjectId,
        pack_id: ObjectId,
        function: String,
        args: HashMap<String, String>,
    ) -> Self {
        Self {
            id,
            pack_id,
            function,
            args,
        }
    }

    pub fn from_op(
        op: &DefineSourceOp,
        registry: &SourceFunctionRegistry,
    ) -> Result<Self, BundlebaseError> {
        // Validate function exists
        registry
            .get(&op.function)
            .ok_or_else(|| format!("Unknown source function '{}'", op.function))?;

        Ok(Self::new(
            op.id.clone(),
            op.pack.clone(),
            op.function.clone(),
            op.args.clone(),
        ))
    }

    pub fn id(&self) -> &ObjectId {
        &self.id
    }

    pub fn pack_id(&self) -> &ObjectId {
        &self.pack_id
    }

    /// Get the URL from args, if present (e.g., for data_directory function).
    pub fn url(&self) -> Option<&str> {
        self.args.get("url").map(|s| s.as_str())
    }

    /// Get patterns from args, if present (e.g., for data_directory function).
    pub fn patterns(&self) -> Option<&str> {
        self.args.get("patterns").map(|s| s.as_str())
    }

    pub fn function(&self) -> &str {
        &self.function
    }

    pub fn args(&self) -> &HashMap<String, String> {
        &self.args
    }

    /// List all files from the source using the configured function.
    pub async fn list_files(
        &self,
        config: Arc<BundleConfig>,
        registry: &Arc<RwLock<SourceFunctionRegistry>>,
    ) -> Result<Vec<ObjectStoreFile>, BundlebaseError> {
        let func = {
            let reg = registry.read();
            reg.get(&self.function)
                .ok_or_else(|| format!("Unknown source function '{}'", self.function))?
        };

        func.list_files(&self.args, config).await
    }

    /// Get URLs of files that have been attached from this source.
    /// Uses source_location if present (original URL from source), otherwise falls back to location.
    pub fn attached_files(&self, operations: &[AnyOperation]) -> HashSet<String> {
        operations
            .iter()
            .filter_map(|op| match op {
                AnyOperation::AttachBlock(attach) if attach.source.as_ref() == Some(&self.id) => {
                    Some(
                        attach
                            .source_location
                            .clone()
                            .unwrap_or_else(|| attach.location.clone()),
                    )
                }
                _ => None,
            })
            .collect()
    }

    /// Get files that exist in the source but haven't been attached yet.
    pub async fn pending_files(
        &self,
        operations: &[AnyOperation],
        config: Arc<BundleConfig>,
        registry: &Arc<RwLock<SourceFunctionRegistry>>,
    ) -> Result<Vec<ObjectStoreFile>, BundlebaseError> {
        let all_files = self.list_files(config, registry).await?;
        let attached = self.attached_files(operations);

        Ok(all_files
            .into_iter()
            .filter(|f| !attached.contains(f.url().as_str()))
            .collect())
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
    fn test_new_source() {
        let source = Source::new(
            ObjectId::from(1),
            ObjectId::from(2),
            "data_directory".to_string(),
            make_args("s3://bucket/data/", Some("**/*")),
        );

        assert_eq!(source.id(), &ObjectId::from(1));
        assert_eq!(source.pack_id(), &ObjectId::from(2));
        assert_eq!(source.url(), Some("s3://bucket/data/"));
        assert_eq!(source.patterns(), Some("**/*"));
        assert_eq!(source.function(), "data_directory");
    }

    #[test]
    fn test_from_op() {
        let registry = SourceFunctionRegistry::new();

        let op = DefineSourceOp {
            id: ObjectId::from(1),
            pack: ObjectId::from(2),
            function: "data_directory".to_string(),
            args: make_args("s3://bucket/data/", Some("**/*.parquet")),
        };

        let source = Source::from_op(&op, &registry).unwrap();
        assert_eq!(source.id(), &ObjectId::from(1));
        assert_eq!(source.pack_id(), &ObjectId::from(2));
        assert_eq!(source.url(), Some("s3://bucket/data/"));
        assert_eq!(source.patterns(), Some("**/*.parquet"));
        assert_eq!(source.function(), "data_directory");
    }

    #[test]
    fn test_from_op_with_extra_args() {
        let registry = SourceFunctionRegistry::new();

        let mut args = make_args("s3://bucket/data/", None);
        args.insert("key".to_string(), "value".to_string());

        let op = DefineSourceOp {
            id: ObjectId::from(1),
            pack: ObjectId::from(2),
            function: "data_directory".to_string(),
            args: args.clone(),
        };

        // from_op succeeds, validation happens in check()
        let result = Source::from_op(&op, &registry);
        assert!(result.is_ok());
        let source = result.unwrap();
        assert_eq!(source.args(), &args);
    }

    #[test]
    fn test_from_op_unknown_function() {
        let registry = SourceFunctionRegistry::new();

        let op = DefineSourceOp {
            id: ObjectId::from(1),
            pack: ObjectId::from(2),
            function: "unknown_function".to_string(),
            args: HashMap::new(),
        };

        let result = Source::from_op(&op, &registry);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unknown source function"));
    }
}
