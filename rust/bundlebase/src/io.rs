mod object_store_dir;
mod object_store_file;
mod util;

pub use crate::data::ObjectId;
pub use crate::io::object_store_dir::ObjectStoreDir;
pub use crate::io::object_store_file::ObjectStoreFile;
use object_store::memory::InMemory;
use std::sync::{Arc, OnceLock};

pub static EMPTY_SCHEME: &str = "empty";
pub static EMPTY_URL: &str = "empty:///";

static MEMORY_STORE: OnceLock<Arc<InMemory>> = OnceLock::new();
static NULL_STORE: OnceLock<Arc<InMemory>> = OnceLock::new();

pub fn get_memory_store() -> Arc<InMemory> {
    MEMORY_STORE
        .get_or_init(|| Arc::new(InMemory::new()))
        .clone()
}

pub fn get_null_store() -> Arc<InMemory> {
    NULL_STORE.get_or_init(|| Arc::new(InMemory::new())).clone()
}

pub struct DataStorage {}

impl DataStorage {
    pub fn new() -> Self {
        Self {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BundleConfig;
    use url::Url;

    #[tokio::test]
    async fn memory_file() {
        // Verify file doesn't exist initially
        let url = &Url::parse("memory:///test_key").unwrap();
        let file = ObjectStoreFile::from_url(url, BundleConfig::default().into()).unwrap();
        assert!(!file.exists().await.unwrap());
        assert_eq!(true, file.version().await.is_err());

        // Write data and verify it's persisted
        file.write(bytes::Bytes::from("hello world")).await.unwrap();
        assert_eq!(
            bytes::Bytes::from("hello world"),
            file.read_bytes().await.unwrap().unwrap(),
            "Written data should be readable"
        );
        assert!(
            file.version().await.is_ok(),
            "Version should be available after write"
        );
    }

    #[tokio::test]
    async fn memory_file_multiple_writes() {
        // Test that multiple writes overwrite previous data
        let url = &Url::parse("memory:///multi_write_test").unwrap();
        let file = ObjectStoreFile::from_url(url, BundleConfig::default().into()).unwrap();

        // First write
        file.write(bytes::Bytes::from("first")).await.unwrap();
        assert_eq!(
            bytes::Bytes::from("first"),
            file.read_bytes().await.unwrap().unwrap()
        );

        // Second write (should overwrite)
        file.write(bytes::Bytes::from("much longer content"))
            .await
            .unwrap();
        assert_eq!(
            bytes::Bytes::from("much longer content"),
            file.read_bytes().await.unwrap().unwrap()
        );
    }

    #[tokio::test]
    async fn file_file() {
        // Absolute file path
        let url = &Url::parse("file:///absolute/path/file.txt").unwrap();
        let file = ObjectStoreFile::from_url(url, BundleConfig::default().into()).unwrap();
        assert_eq!(
            "file:///absolute/path/file.txt",
            file.url().to_string(),
            "Absolute file URL should be preserved"
        );

        // File URL from relative path
        let file = ObjectStoreFile::from_url(
            &Url::from_file_path(
                std::env::current_dir()
                    .unwrap()
                    .join("relative_path/file.txt"),
            )
            .unwrap(),
            BundleConfig::default().into(),
        )
        .unwrap();
        assert!(
            file.url().to_string().contains("relative_path/file.txt"),
            "Relative file path should be converted to URL"
        );

        // File URL from absolute path
        let file = ObjectStoreFile::from_url(
            &Url::from_file_path("/absolute/path/to/file.txt").unwrap(),
            BundleConfig::default().into(),
        )
        .unwrap();
        assert_eq!(
            "file:///absolute/path/to/file.txt",
            file.url().to_string(),
            "Absolute path should be converted correctly"
        );
    }

    #[tokio::test]
    async fn test_factory_rejects_unknown_scheme() {
        // Test that unknown URL schemes are rejected
        let url = &Url::parse("unknown://test").unwrap();
        let result = ObjectStoreFile::from_url(url, BundleConfig::default().into());
        assert!(result.is_err(), "Unknown scheme should be rejected");
        assert_eq!(
            result.err().unwrap().to_string(),
            "Generic URL error: Unable to recognise URL \"unknown://test\""
        );
    }

    #[tokio::test]
    async fn s3_file() {
        // Test S3 file URL handling
        let url = &Url::parse("s3://bucket/key").unwrap();
        let file = ObjectStoreFile::from_url(url, BundleConfig::default().into());
        assert!(file.is_ok(), "S3 URL should be supported");
        assert_eq!(
            "s3://bucket/key",
            file.unwrap().url().to_string(),
            "S3 URL should be preserved"
        );
    }

    #[tokio::test]
    async fn s3_file_various_paths() {
        // Test various S3 path formats
        let cases = vec![
            ("s3://my-bucket/file.txt", "s3://my-bucket/file.txt"),
            (
                "s3://bucket/path/to/file.parquet",
                "s3://bucket/path/to/file.parquet",
            ),
            (
                "s3://bucket/deep/nested/path/data.csv",
                "s3://bucket/deep/nested/path/data.csv",
            ),
        ];

        for (url_str, expected) in cases {
            let url = Url::parse(url_str).unwrap();
            let file = ObjectStoreFile::from_url(&url, BundleConfig::default().into()).unwrap();
            assert_eq!(expected, file.url().to_string());
        }
    }

    #[test]
    fn dir_from_url() {
        for (url, expected) in vec![
            ("memory:///test", "memory:///test"),
            ("memory:///test/", "memory:///test/"),
            ("memory:///test/here", "memory:///test/here"),
            ("memory:///test/here/", "memory:///test/here/"),
            ("file:///test/path", "file:///test/path"),
        ] {
            assert_eq!(
                expected,
                ObjectStoreFile::from_url(
                    &Url::parse(url).unwrap(),
                    BundleConfig::default().into()
                )
                .unwrap()
                .url()
                .to_string()
            );
        }
    }

    #[test]
    fn file_from_url() {
        for (url, expected) in vec![
            ("memory:///test", "memory:///test"),
            ("memory:///test/", "memory:///test/"),
            ("memory:///test/here.txt", "memory:///test/here.txt"),
            ("memory:///test/here/", "memory:///test/here/"),
            ("file:///test/path", "file:///test/path"),
        ] {
            assert_eq!(
                expected,
                ObjectStoreFile::from_url(
                    &Url::parse(url).unwrap(),
                    BundleConfig::default().into()
                )
                .unwrap()
                .url()
                .to_string()
            );
        }
    }
}
