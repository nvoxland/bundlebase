//! Source struct representing a data source definition for a pack.

use crate::bundle::{BundleBuilder, CreateSourceOp};
use crate::data::ObjectId;
use crate::source::{AttachedFileInfo, FetchAction, SourceFunctionRegistry, SyncMode, orchestrate_fetch};
use crate::source::source_utils;
use crate::BundlebaseError;
use parking_lot::RwLock;
use std::collections::HashMap;

/// Represents a data source definition for a pack.
///
/// A source specifies how to discover and list data files.
/// All configuration is stored in function-specific arguments.
#[derive(Debug)]
pub struct Source {
    id: ObjectId,
    pack: ObjectId,
    /// Source function name (e.g., "remote_dir")
    function: String,
    /// Function-specific configuration arguments
    /// For "remote_dir": "url" (required), "patterns" (optional)
    args: HashMap<String, String>,
    /// Attached files from this source, keyed by source_location
    attached_files: RwLock<HashMap<String, AttachedFileInfo>>,
}

impl Source {
    pub fn new(
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
            attached_files: RwLock::new(HashMap::new()),
        }
    }

    pub fn from_op(
        op: &CreateSourceOp,
        registry: &SourceFunctionRegistry,
    ) -> Result<Self, BundlebaseError> {
        // Validate function exists
        registry
            .get(&op.function)
            .ok_or_else(|| format!("Unknown source function '{}'", op.function))?;

        Ok(Self::new(
            op.id,
            op.pack,
            op.function.clone(),
            op.args.clone(),
        ))
    }

    pub fn id(&self) -> &ObjectId {
        &self.id
    }

    pub fn pack(&self) -> &ObjectId {
        &self.pack
    }

    pub fn function(&self) -> &str {
        &self.function
    }

    pub fn args(&self) -> &HashMap<String, String> {
        &self.args
    }

    /// Fetch this source: find new data and materialize it.
    ///
    /// Returns a list of fetch actions (Add, Replace, Remove) based on the sync mode.
    pub async fn fetch(
        &self,
        builder: &BundleBuilder,
        mode: SyncMode,
    ) -> Result<Vec<FetchAction>, BundlebaseError> {
        let (func, data_dir, config) = {
            let bundle = builder.bundle();
            let registry = bundle.source_function_registry();
            let reg = registry.read();
            let func = reg
                .create_instance(&self.function)
                .ok_or_else(|| format!("Unknown source function '{}'", self.function))?;
            (func, bundle.data_dir(), bundle.config())
        };

        // Get attached files directly from self
        let attached_files = self.attached_files();
        let should_copy = source_utils::should_copy(&self.args);

        orchestrate_fetch(
            func.as_ref(),
            &self.args,
            mode,
            should_copy,
            data_dir.as_ref(),
            &attached_files,
            &config,
        )
        .await
    }

    /// Get attached files with metadata for change detection.
    /// Returns a clone of the internal HashMap.
    pub fn attached_files(&self) -> HashMap<String, AttachedFileInfo> {
        self.attached_files.read().clone()
    }

    /// Add an attached file to this source.
    pub(crate) fn add_attached_file(&self, source_location: &str, info: AttachedFileInfo) {
        self.attached_files
            .write()
            .insert(source_location.to_string(), info);
    }

    /// Remove an attached file from this source.
    pub(crate) fn remove_attached_file(&self, source_location: &str) {
        self.attached_files.write().remove(source_location);
    }

    /// Update an attached file in this source.
    pub(crate) fn update_attached_file(&self, source_location: &str, info: AttachedFileInfo) {
        self.attached_files
            .write()
            .insert(source_location.to_string(), info);
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
        let id = ObjectId::generate();
        let pack = ObjectId::generate();
        let source = Source::new(
            id,
            pack,
            "remote_dir".to_string(),
            make_args("s3://bucket/data/", Some("**/*")),
        );

        assert_eq!(source.id(), &id);
        assert_eq!(source.pack(), &pack);
        assert_eq!(source.args().get("url").map(|s| s.as_str()), Some("s3://bucket/data/"));
        assert_eq!(source.args().get("patterns").map(|s| s.as_str()), Some("**/*"));
        assert_eq!(source.function(), "remote_dir");
    }

    #[test]
    fn test_from_op() {
        let registry = SourceFunctionRegistry::new();

        let id = ObjectId::generate();
        let pack = ObjectId::generate();
        let op = CreateSourceOp {
            id,
            pack,
            function: "remote_dir".to_string(),
            args: make_args("s3://bucket/data/", Some("**/*.parquet")),
        };

        let source = Source::from_op(&op, &registry).unwrap();
        assert_eq!(source.id(), &id);
        assert_eq!(source.pack(), &pack);
        assert_eq!(source.args().get("url").map(|s| s.as_str()), Some("s3://bucket/data/"));
        assert_eq!(source.args().get("patterns").map(|s| s.as_str()), Some("**/*.parquet"));
        assert_eq!(source.function(), "remote_dir");
    }

    #[test]
    fn test_from_op_with_extra_args() {
        let registry = SourceFunctionRegistry::new();

        let mut args = make_args("s3://bucket/data/", None);
        args.insert("key".to_string(), "value".to_string());

        let op = CreateSourceOp {
            id: ObjectId::generate(),
            pack: ObjectId::generate(),
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

        let op = CreateSourceOp {
            id: ObjectId::generate(),
            pack: ObjectId::generate(),
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
