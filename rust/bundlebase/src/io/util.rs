use crate::io::{get_memory_store, get_null_store, EMPTY_SCHEME};
use crate::BundlebaseError;
use datafusion::datasource::object_store::ObjectStoreUrl;
use object_store::path::Path;
use object_store::{path::Path as ObjectPath, ObjectStore};
use std::collections::HashMap;
use std::sync::Arc;
use url::Url;

pub(super) fn compute_store_url(url: &Url) -> ObjectStoreUrl {
    ObjectStoreUrl::parse(format!("{}://{}", url.scheme(), url.authority())).unwrap()
}

/// Parse a URL and return an ObjectStore and Path
///
/// # Arguments
/// * `url` - The URL to parse
/// * `config` - Optional configuration to apply to the ObjectStore
pub(super) fn parse_url(
    url: &Url,
    config: &HashMap<String, String>,
) -> Result<(Arc<dyn ObjectStore>, Path), BundlebaseError> {
    if url.scheme() == EMPTY_SCHEME {
        let store: Arc<dyn ObjectStore> = get_null_store();

        if !url.authority().is_empty() {
            return Err("Empty URL must be empty:///<path>.".into());
        }
        Ok((store, ObjectPath::from(url.path())))
    } else if url.scheme() == "memory" {
        if !url.authority().is_empty() {
            return Err("Memory URL must be memory:///<path>".into());
        }
        Ok((get_memory_store(), url.path().into()))
    } else if !config.is_empty() {
        // Use config to build ObjectStore
        let store = build_object_store(url, config)?;
        let path = Path::from(url.path());
        Ok((Arc::new(store), path))
    } else {
        // Fallback to object_store::parse_url when no config
        let (store, path) = object_store::parse_url(url)?;
        Ok((Arc::new(store), path))
    }
}

/// Build an ObjectStore with configuration
///
/// Starts with Builder::from_env() to pick up environment variables,
/// then applies config values on top (config overrides env vars).
fn build_object_store(
    url: &Url,
    config: &HashMap<String, String>,
) -> Result<Box<dyn ObjectStore>, BundlebaseError> {
    use object_store::aws::AmazonS3Builder;
    use object_store::azure::MicrosoftAzureBuilder;
    use object_store::gcp::GoogleCloudStorageBuilder;

    match url.scheme() {
        "s3" => {
            let mut builder = AmazonS3Builder::from_env().with_url(url.as_str());

            // Apply config values
            for (key, value) in config {
                builder = builder.with_config(key.parse()?, value);
            }

            Ok(Box::new(builder.build()?))
        }
        "gs" => {
            let mut builder = GoogleCloudStorageBuilder::from_env().with_url(url.as_str());

            // Apply config values
            for (key, value) in config {
                builder = builder.with_config(key.parse()?, value);
            }

            Ok(Box::new(builder.build()?))
        }
        "azure" | "az" => {
            let mut builder = MicrosoftAzureBuilder::from_env().with_url(url.as_str());

            // Apply config values
            for (key, value) in config {
                builder = builder.with_config(key.parse()?, value);
            }

            Ok(Box::new(builder.build()?))
        }
        scheme => {
            // For unknown schemes, fall back to object_store::parse_url
            let (store, _) = object_store::parse_url(url)
                .map_err(|e| format!("Unsupported URL scheme '{}': {}", scheme, e))?;
            Ok(Box::new(store))
        }
    }
}

/// Like Url::join but allows an input with multiple sub-paths. The appended path is always treated as a relative path.
pub(super) fn join_url(base: &Url, append: &str) -> Result<Url, BundlebaseError> {
    let base = if !base.path().ends_with('/') {
        &Url::parse(format!("{}/", base.to_string()).as_str())?
    } else {
        &base
    };

    let mut return_url = base.clone();
    for segment in append.split("/").filter(|s| !s.is_empty()) {
        return_url = return_url.join(format!("{}/", segment).as_str())?;
    }
    if !append.ends_with('/') {
        return_url.set_path(return_url.path().trim_end_matches('/').to_string().as_str());
    }
    Ok(return_url)
}

pub(super) fn join_path(base: &Path, append: &str) -> Result<Path, BundlebaseError> {
    let mut obj_path = base.clone();
    for segment in append.split('/').filter(|s| !s.is_empty()) {
        if segment == ".." {
            let mut path_str = obj_path.to_string();
            if path_str.ends_with('/') {
                path_str = path_str[0..path_str.len() - 1].to_string();
            }
            obj_path = Path::parse(match path_str.rfind("/") {
                Some(idx) => path_str[0..idx].to_string(),
                None => "/".to_string(),
            })?;
        } else {
            obj_path = obj_path.child(segment);
        }
    }
    if append.ends_with('/') {
        obj_path = obj_path.child("");
    }
    Ok(obj_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case("s3://bucket/path/to/dir", "s3://bucket/")]
    #[case("s3://bucket/path/to/dir", "s3://bucket/")]
    #[case("memory:///path/to/dir", "memory:///")]
    #[case("memory:///path/to/dir", "memory:///")]
    fn test_compute_store_url(#[case] url: &str, #[case] expected: &str) {
        let url = Url::parse(url).unwrap();
        assert_eq!(expected, compute_store_url(&url).as_str());
    }

    #[rstest]
    #[case(
        "s3://bucket/path/to/dir",
        "file.txt",
        "s3://bucket/path/to/dir/file.txt"
    )]
    #[case("memory:///path/to/dir", "file.txt", "memory:///path/to/dir/file.txt")]
    #[case("memory:///base", "dir", "memory:///base/dir")]
    #[case("memory:///base", "more/here", "memory:///base/more/here")]
    #[case("memory:///base/", "more/here", "memory:///base/more/here")]
    #[case("memory:///base/", "/more/here", "memory:///base/more/here")]
    #[case("memory:///base/", "/more/here/", "memory:///base/more/here/")]
    #[case("memory:///path/to/dir", "../file.txt", "memory:///path/to/file.txt")]
    #[case("memory:///path/to/dir", "../../file.txt", "memory:///path/file.txt")]
    fn test_join_url(#[case] base: &str, #[case] append: &str, #[case] expected: &str) {
        assert_eq!(
            expected,
            join_url(&Url::parse(base).unwrap(), append)
                .unwrap()
                .to_string()
        )
    }

    #[rstest]
    #[case("path/to/dir", "file.txt", "path/to/dir/file.txt")]
    #[case("/path/to/dir", "file.txt", "path/to/dir/file.txt")]
    #[case("/path/to/dir", "../file.txt", "path/to/file.txt")]
    #[case("/path/to/dir", "../../file.txt", "path/file.txt")]
    #[case("/path/to/dir", "../../../file.txt", "file.txt")]
    #[case("/path/to/dir", "../../../../file.txt", "file.txt")]
    #[case("/base", "dir", "base/dir")]
    #[case("/base", "more/here", "base/more/here")]
    #[case("/base/", "more/here", "base/more/here")]
    #[case("/base/", "/more/here", "base/more/here")]
    #[case("/base/", "/more/here/", "base/more/here/")]
    fn test_join_path(#[case] base: &str, #[case] append: &str, #[case] expected: &str) {
        assert_eq!(
            expected,
            join_path(&Path::parse(base).unwrap(), append)
                .unwrap()
                .to_string()
        )
    }
}
