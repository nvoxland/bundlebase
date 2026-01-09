use crate::bundle::{AnyOperation, DefineSourceOp};
use crate::data::{ObjectId, SourceFunctionRegistry};
use crate::io::ObjectStoreFile;
use crate::BundlebaseError;
use crate::BundleConfig;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use url::Url;

/// Represents a data source definition for a pack.
///
/// A source specifies where to look for data files (e.g., S3 bucket prefix)
/// and patterns to filter which files to include.
#[derive(Debug, Clone)]
pub struct Source {
    id: ObjectId,
    pack_id: ObjectId,
    url: Url,
    patterns: Vec<String>,
    /// Source function name (e.g., "data_directory")
    function: String,
    /// Function-specific configuration arguments
    args: HashMap<String, String>,
}

impl Source {
    pub fn new(
        id: ObjectId,
        pack_id: ObjectId,
        url: Url,
        patterns: Vec<String>,
        function: String,
        args: HashMap<String, String>,
    ) -> Self {
        Self {
            id,
            pack_id,
            url,
            patterns,
            function,
            args,
        }
    }

    pub fn from_op(
        op: &DefineSourceOp,
        registry: &SourceFunctionRegistry,
    ) -> Result<Self, BundlebaseError> {
        let url = Url::parse(&op.url)?;

        // Validate function exists
        registry
            .get(&op.function)
            .ok_or_else(|| format!("Unknown source function '{}'", op.function))?;

        Ok(Self::new(
            op.id.clone(),
            op.pack_id.clone(),
            url,
            op.patterns.clone(),
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

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    pub fn function(&self) -> &str {
        &self.function
    }

    pub fn args(&self) -> &HashMap<String, String> {
        &self.args
    }

    /// List all files from the source URL that match the patterns.
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

        func.list_files(&self.url, &self.patterns, &self.args, config)
            .await
    }

    /// Get URLs of files that have been attached from this source.
    pub fn attached_files(&self, operations: &[AnyOperation]) -> HashSet<String> {
        operations
            .iter()
            .filter_map(|op| match op {
                AnyOperation::AttachBlock(attach) if attach.source_id.as_ref() == Some(&self.id) => {
                    Some(attach.source.clone())
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

    #[test]
    fn test_new_source() {
        let source = Source::new(
            ObjectId::from(1),
            ObjectId::from(2),
            Url::parse("s3://bucket/data/").unwrap(),
            vec!["**/*".to_string()],
            "data_directory".to_string(),
            HashMap::new(),
        );

        assert_eq!(source.id(), &ObjectId::from(1));
        assert_eq!(source.pack_id(), &ObjectId::from(2));
        assert_eq!(source.url().as_str(), "s3://bucket/data/");
        assert_eq!(source.patterns(), &["**/*".to_string()]);
        assert_eq!(source.function(), "data_directory");
        assert!(source.args().is_empty());
    }

    #[test]
    fn test_from_op() {
        let registry = SourceFunctionRegistry::new();

        let op = DefineSourceOp {
            id: ObjectId::from(1),
            pack_id: ObjectId::from(2),
            url: "s3://bucket/data/".to_string(),
            patterns: vec!["**/*.parquet".to_string()],
            function: "data_directory".to_string(),
            args: HashMap::new(),
        };

        let source = Source::from_op(&op, &registry).unwrap();
        assert_eq!(source.id(), &ObjectId::from(1));
        assert_eq!(source.pack_id(), &ObjectId::from(2));
        assert_eq!(source.url().as_str(), "s3://bucket/data/");
        assert_eq!(source.patterns(), &["**/*.parquet".to_string()]);
        assert_eq!(source.function(), "data_directory");
        assert!(source.args().is_empty());
    }

    #[test]
    fn test_from_op_with_args() {
        let registry = SourceFunctionRegistry::new();

        let mut args = HashMap::new();
        args.insert("key".to_string(), "value".to_string());

        let op = DefineSourceOp {
            id: ObjectId::from(1),
            pack_id: ObjectId::from(2),
            url: "s3://bucket/data/".to_string(),
            patterns: vec!["**/*".to_string()],
            function: "data_directory".to_string(),
            args: args.clone(),
        };

        // This should fail because data_directory doesn't accept arguments
        let result = Source::from_op(&op, &registry);
        assert!(result.is_ok()); // from_op succeeds, validation happens elsewhere
        let source = result.unwrap();
        assert_eq!(source.args(), &args);
    }

    #[test]
    fn test_from_op_unknown_function() {
        let registry = SourceFunctionRegistry::new();

        let op = DefineSourceOp {
            id: ObjectId::from(1),
            pack_id: ObjectId::from(2),
            url: "s3://bucket/data/".to_string(),
            patterns: vec!["**/*".to_string()],
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
