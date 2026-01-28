/// Shared test utilities for integration tests
use arrow::datatypes::SchemaRef;
use bundlebase::bundle::{manifest_version, BundleCommit, INIT_FILENAME};
use bundlebase::io::{readable_file_from_url, IOReadWriteDir};
use bundlebase::{BundlebaseError, BundleConfig};
use datafusion::dataframe::DataFrame;
use url::Url;

#[allow(dead_code)]
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
    data_dir: &dyn IOReadWriteDir,
) -> Result<Option<(String, BundleCommit, Url)>, BundlebaseError> {
    let meta_dir = data_dir.subdir("_bundlebase")?;

    let files = meta_dir.list_files().await?;
    let mut files = files
        .iter()
        .filter(|x| x.filename() != Some(INIT_FILENAME))
        .collect::<Vec<_>>();

    files.sort_by_key(|f| manifest_version(f.filename().unwrap_or("")));

    let last_file = files.iter().last();

    match last_file {
        None => Ok(None),
        Some(file) => {
            let io_file = readable_file_from_url(&file.url, BundleConfig::default().into())?;
            let yaml = io_file.read_str().await?;
            Ok(yaml.map(|content| {
                (
                    content.clone(),
                    serde_yaml_ng::from_str(&content).unwrap(),
                    file.url.clone(),
                )
            }))
        }
    }
}

/// Count total rows in a DataFrame by collecting and summing batch row counts
#[allow(dead_code)]
pub async fn count_rows(df: &DataFrame) -> Result<usize, BundlebaseError> {
    let record_batches = df.clone().collect().await?;
    Ok(record_batches.iter().map(|rb| rb.num_rows()).sum())
}
