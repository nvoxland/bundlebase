//! File metadata information.

use chrono::{DateTime, Utc};
use url::Url;

/// Information about a file in storage.
/// Protocol-agnostic metadata common to all storage backends.
#[derive(Debug, Clone)]
pub struct FileInfo {
    /// Full URL of the file
    pub url: Url,
    /// File size in bytes (if known)
    pub size: Option<u64>,
    /// Last modified time (if available)
    pub modified: Option<DateTime<Utc>>,
}

impl FileInfo {
    /// Create a new FileInfo with the given URL.
    pub fn new(url: Url) -> Self {
        Self {
            url,
            size: None,
            modified: None,
        }
    }

    /// Create a FileInfo with size information.
    pub fn with_size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }

    /// Create a FileInfo with modification time.
    pub fn with_modified(mut self, modified: DateTime<Utc>) -> Self {
        self.modified = Some(modified);
        self
    }

    /// Get the filename portion of the URL path.
    /// Returns None if the URL has no path segments or the last segment is empty.
    pub fn filename(&self) -> Option<&str> {
        self.url
            .path_segments()
            .and_then(|segments| segments.last())
            .filter(|s| !s.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_info_builder() {
        let url = Url::parse("memory:///test.txt").unwrap();
        let info = FileInfo::new(url.clone())
            .with_size(1024)
            .with_modified(Utc::now());

        assert_eq!(info.url, url);
        assert_eq!(info.size, Some(1024));
        assert!(info.modified.is_some());
    }

    #[test]
    fn test_file_info_filename_with_valid_filename() {
        let url = Url::parse("memory:///path/to/file.txt").unwrap();
        let info = FileInfo::new(url);
        assert_eq!(info.filename(), Some("file.txt"));
    }

    #[test]
    fn test_file_info_filename_root_path() {
        let url = Url::parse("memory:///file.txt").unwrap();
        let info = FileInfo::new(url);
        assert_eq!(info.filename(), Some("file.txt"));
    }

    #[test]
    fn test_file_info_filename_trailing_slash() {
        // Trailing slash means the last segment is empty
        let url = Url::parse("memory:///path/to/").unwrap();
        let info = FileInfo::new(url);
        // Returns None because last segment is empty
        assert_eq!(info.filename(), None);
    }

    #[test]
    fn test_file_info_filename_empty_path() {
        // URL with empty path
        let url = Url::parse("memory:///").unwrap();
        let info = FileInfo::new(url);
        assert_eq!(info.filename(), None);
    }
}
