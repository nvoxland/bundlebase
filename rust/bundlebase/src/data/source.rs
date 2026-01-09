use crate::bundle::{AnyOperation, DefineSourceOp};
use crate::data::ObjectId;
use crate::io::{ObjectStoreDir, ObjectStoreFile};
use crate::BundlebaseError;
use crate::BundleConfig;
use glob::Pattern;
use std::collections::HashSet;
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
}

impl Source {
    pub fn new(id: ObjectId, pack_id: ObjectId, url: Url, patterns: Vec<String>) -> Self {
        Self {
            id,
            pack_id,
            url,
            patterns,
        }
    }

    pub fn from_op(op: &DefineSourceOp) -> Result<Self, BundlebaseError> {
        let url = Url::parse(&op.url)?;
        Ok(Self::new(
            op.id.clone(),
            op.pack_id.clone(),
            url,
            op.patterns.clone(),
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

    /// List all files from the source URL that match the patterns.
    pub async fn list_files(
        &self,
        config: Arc<BundleConfig>,
    ) -> Result<Vec<ObjectStoreFile>, BundlebaseError> {
        let dir = ObjectStoreDir::from_url(&self.url, config)?;
        let all_files = dir.list_files().await?;

        // Compile glob patterns
        let compiled_patterns: Vec<Pattern> = self
            .patterns
            .iter()
            .filter_map(|p| Pattern::new(p).ok())
            .collect();

        // Filter files by pattern
        let matching_files: Vec<ObjectStoreFile> = all_files
            .into_iter()
            .filter(|file| {
                let relative_path = self.relative_path(file.url());
                compiled_patterns
                    .iter()
                    .any(|pattern: &Pattern| pattern.matches(&relative_path))
            })
            .collect();

        Ok(matching_files)
    }

    /// Get the relative path of a file URL compared to the source URL.
    fn relative_path(&self, file_url: &Url) -> String {
        let source_path = self.url.path();
        let file_path = file_url.path();

        if let Some(stripped) = file_path.strip_prefix(source_path) {
            stripped.trim_start_matches('/').to_string()
        } else {
            file_path.to_string()
        }
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
    ) -> Result<Vec<ObjectStoreFile>, BundlebaseError> {
        let all_files = self.list_files(config).await?;
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
    fn test_relative_path() {
        let source = Source::new(
            ObjectId::from(1),
            ObjectId::from(2),
            Url::parse("s3://bucket/data/").unwrap(),
            vec!["**/*".to_string()],
        );

        let file_url = Url::parse("s3://bucket/data/subdir/file.parquet").unwrap();
        assert_eq!(source.relative_path(&file_url), "subdir/file.parquet");
    }

    #[test]
    fn test_pattern_matching() {
        let pattern = Pattern::new("**/*.parquet").unwrap();
        assert!(pattern.matches("file.parquet"));
        assert!(pattern.matches("subdir/file.parquet"));
        assert!(pattern.matches("a/b/c/file.parquet"));
        assert!(!pattern.matches("file.csv"));
    }

    #[test]
    fn test_from_op() {
        let op = DefineSourceOp {
            id: ObjectId::from(1),
            pack_id: ObjectId::from(2),
            url: "s3://bucket/data/".to_string(),
            patterns: vec!["**/*.parquet".to_string()],
        };

        let source = Source::from_op(&op).unwrap();
        assert_eq!(source.id(), &ObjectId::from(1));
        assert_eq!(source.pack_id(), &ObjectId::from(2));
        assert_eq!(source.url().as_str(), "s3://bucket/data/");
        assert_eq!(source.patterns(), &["**/*.parquet".to_string()]);
    }
}
