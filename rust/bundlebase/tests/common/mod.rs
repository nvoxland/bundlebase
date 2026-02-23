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
            let io_file = readable_file_from_url(&file.url, BundleConfig::new(None)?.into()).await?;
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

/// Strip columnId/columnIds fields from serialized YAML for comparison.
/// These contain random generated IDs that differ between test runs.
#[allow(dead_code)]
pub fn strip_column_ids(yaml: &str) -> String {
    let mut result = Vec::new();
    let mut in_column_ids_list = false;

    for line in yaml.lines() {
        let trimmed = line.trim();

        // Skip standalone columnId lines (e.g., "    columnId: abc123")
        if trimmed.starts_with("columnId: ") || trimmed.starts_with("columnId:") {
            continue;
        }

        // Detect start of columnIds list
        if trimmed.starts_with("columnIds:") {
            in_column_ids_list = true;
            continue;
        }

        // Skip list items under columnIds
        if in_column_ids_list {
            if trimmed.starts_with("- ") && !trimmed.contains(": ") {
                continue;
            }
            in_column_ids_list = false;
        }

        result.push(line);
    }

    let mut output = result.join("\n");
    if yaml.ends_with('\n') {
        output.push('\n');
    }
    output
}

/// Count total rows in a DataFrame by collecting and summing batch row counts
#[allow(dead_code)]
pub async fn count_rows(df: &DataFrame) -> Result<usize, BundlebaseError> {
    let record_batches = df.clone().collect().await?;
    Ok(record_batches.iter().map(|rb| rb.num_rows()).sum())
}
