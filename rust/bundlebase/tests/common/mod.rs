/// Shared test utilities for integration tests
use arrow::datatypes::SchemaRef;
use bundlebase::bundle::{manifest_version, BundleCommit, INIT_FILENAME};
use bundlebase::io::{IOFile, IOLister, IOReader, IODir};
use bundlebase::{BundlebaseError, BundleConfig};
use url::Url;


pub fn enable_logging() {
    let _ = env_logger::builder().is_test(true).try_init();
}

/// Helper to check if schema has a column
#[allow(dead_code)]
pub fn has_column(schema: &SchemaRef, name: &str) -> bool {
    schema.fields().iter().any(|f| f.name() == name)
}

#[allow(dead_code)]
pub async fn latest_commit(
    data_dir: &IODir,
) -> Result<Option<(String, BundleCommit, Url)>, BundlebaseError> {
    let meta_dir = data_dir.io_subdir("_bundlebase")?;

    let files = meta_dir.list_files().await?;
    let mut files = files
        .iter()
        .filter(|x| x.filename() != INIT_FILENAME)
        .collect::<Vec<_>>();

    files.sort_by_key(|f| manifest_version(f.filename()));

    let last_file = files.iter().last();

    match last_file {
        None => Ok(None),
        Some(file) => {
            let io_file = IOFile::from_url(&file.url, BundleConfig::default().into())?;
            let yaml = io_file.read_str().await?;
            Ok(yaml.map(|content| {
                (
                    content.clone(),
                    serde_yaml::from_str(&content).unwrap(),
                    file.url.clone(),
                )
            }))
        }
    }
}
