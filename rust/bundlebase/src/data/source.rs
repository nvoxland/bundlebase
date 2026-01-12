//! Source module for data source definitions and discovery functions.

mod remote_dir;
mod source_function;
mod source_utils;
mod web_scrape;

use crate::bundle::{AnyOperation, DefineSourceOp};
use crate::data::ObjectId;
use crate::io::IODir;
use crate::BundlebaseError;
use crate::BundleConfig;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub use remote_dir::RemoteDirFunction;
pub use source_function::{
    AttachedFileInfo, MaterializedData, RefreshAction, SourceFunction, SourceFunctionRegistry,
    SyncMode,
};
pub use web_scrape::WebScrapeFunction;

/// Represents a data source definition for a pack.
///
/// A source specifies how to discover and list data files.
/// All configuration is stored in function-specific arguments.
#[derive(Debug, Clone)]
pub struct Source {
    id: ObjectId,
    pack_id: ObjectId,
    /// Source function name (e.g., "remote_dir")
    function: String,
    /// Function-specific configuration arguments
    /// For "remote_dir": "url" (required), "patterns" (optional)
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

    pub fn function(&self) -> &str {
        &self.function
    }

    pub fn args(&self) -> &HashMap<String, String> {
        &self.args
    }

    /// Refresh this source: find new data and materialize it.
    ///
    /// Returns a list of refresh actions (Add, Replace, Remove) based on the sync mode.
    pub async fn refresh(
        &self,
        operations: &[AnyOperation],
        data_dir: &IODir,
        config: Arc<BundleConfig>,
        registry: &Arc<RwLock<SourceFunctionRegistry>>,
    ) -> Result<Vec<RefreshAction>, BundlebaseError> {
        let func = {
            let reg = registry.read();
            reg.get(&self.function)
                .ok_or_else(|| format!("Unknown source function '{}'", self.function))?
        };

        // Parse sync mode from args (defaults to "add")
        let mode = self
            .args
            .get("mode")
            .map(|s| SyncMode::from_arg(s))
            .transpose()?
            .unwrap_or_default();

        // Build attached files map with metadata
        let attached_files = self.attached_files(operations);

        func.refresh_with_mode(&self.args, &attached_files, data_dir, config, mode)
            .await
    }

    /// Get locations already attached from this source (simple set for backward compatibility).
    fn attached_locations(&self, operations: &[AnyOperation]) -> HashSet<String> {
        operations
            .iter()
            .filter_map(|op| match op {
                AnyOperation::AttachBlock(attach) if attach.source.as_ref() == Some(&self.id) => {
                    attach.source_location.clone()
                }
                _ => None,
            })
            .collect()
    }

    /// Get attached files with metadata for change detection.
    fn attached_files(&self, operations: &[AnyOperation]) -> HashMap<String, AttachedFileInfo> {
        operations
            .iter()
            .filter_map(|op| match op {
                AnyOperation::AttachBlock(attach) if attach.source.as_ref() == Some(&self.id) => {
                    attach.source_location.as_ref().map(|source_loc| {
                        (
                            source_loc.clone(),
                            AttachedFileInfo {
                                location: attach.location.clone(),
                                version: attach.version.clone(),
                                bytes: attach.bytes,
                            },
                        )
                    })
                }
                _ => None,
            })
            .collect()
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
            "remote_dir".to_string(),
            make_args("s3://bucket/data/", Some("**/*")),
        );

        assert_eq!(source.id(), &ObjectId::from(1));
        assert_eq!(source.pack_id(), &ObjectId::from(2));
        assert_eq!(source.args().get("url").map(|s| s.as_str()), Some("s3://bucket/data/"));
        assert_eq!(source.args().get("patterns").map(|s| s.as_str()), Some("**/*"));
        assert_eq!(source.function(), "remote_dir");
    }

    #[test]
    fn test_from_op() {
        let registry = SourceFunctionRegistry::new();

        let op = DefineSourceOp {
            id: ObjectId::from(1),
            pack: ObjectId::from(2),
            function: "remote_dir".to_string(),
            args: make_args("s3://bucket/data/", Some("**/*.parquet")),
        };

        let source = Source::from_op(&op, &registry).unwrap();
        assert_eq!(source.id(), &ObjectId::from(1));
        assert_eq!(source.pack_id(), &ObjectId::from(2));
        assert_eq!(source.args().get("url").map(|s| s.as_str()), Some("s3://bucket/data/"));
        assert_eq!(source.args().get("patterns").map(|s| s.as_str()), Some("**/*.parquet"));
        assert_eq!(source.function(), "remote_dir");
    }

    #[test]
    fn test_from_op_with_extra_args() {
        let registry = SourceFunctionRegistry::new();

        let mut args = make_args("s3://bucket/data/", None);
        args.insert("key".to_string(), "value".to_string());

        let op = DefineSourceOp {
            id: ObjectId::from(1),
            pack: ObjectId::from(2),
            function: "remote_dir".to_string(),
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
