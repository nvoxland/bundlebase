/// Shared test utilities for integration tests
use arrow::datatypes::SchemaRef;
use bundlebase::bundle::{manifest_version, BundleCommit, INIT_FILENAME};
use bundlebase::io::ObjectStoreDir;
use bundlebase::BundlebaseError;
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
    data_dir: &ObjectStoreDir,
) -> Result<Option<(String, BundleCommit, Url)>, BundlebaseError> {
    let meta_dir = data_dir.subdir("_bundlebase")?;

    let files = meta_dir.list_files().await?;
    let mut files = files
        .iter()
        .filter(|x| x.filename() != INIT_FILENAME)
        .collect::<Vec<_>>();

    files.sort_by_key(|f| manifest_version(f.filename()));

    let last_file = files.iter().last();

    match last_file {
        None => Ok(None),
        Some(file) => file.read_str().await.map(|yaml| {
            Some((
                yaml.as_ref().unwrap().to_string(),
                serde_yaml::from_str(yaml.as_ref().unwrap()).unwrap(),
                file.url().clone(),
            ))
        }),
    }
}
