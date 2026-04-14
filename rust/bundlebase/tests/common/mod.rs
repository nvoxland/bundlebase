use std::sync::{Arc, Once};
/// Shared test utilities for integration tests
use arrow::datatypes::SchemaRef;

static INIT: Once = Once::new();

/// Initialize the catalog hook for tests. Safe to call multiple times.
#[allow(dead_code)]
pub fn init_catalog() {
    INIT.call_once(|| {
        bundlebase_catalog::init();
    });
}
use bundlebase::bundle::{manifest_version, BundleCommit, INIT_FILENAME};
use bundlebase::BundleConfig;
use bundlebase_common::{BundlebaseError, ConfigProvider};
use bundlebase_io::{readable_file_from_url, IOReadWriteDir};
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
            let io_file = readable_file_from_url(&file.url, Arc::new(BundleConfig::new(None)?) as Arc<dyn ConfigProvider>).await?;
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

/// Strip column ID fields from serialized YAML for comparison.
/// These contain random generated IDs that differ between test runs.
///
/// Strips:
/// - `columnId:` fields on single-column ops (Rename/Cast/Add/DropColumn)
/// - `columnIds:` lists / content-addressed paths on AttachBlockOp
/// - `id:` fields on column operations
///   but NOT on AttachBlock or change-level `id:` lines
#[allow(dead_code)]
pub fn strip_column_ids(yaml: &str) -> String {
    let lines: Vec<&str> = yaml.lines().collect();
    let mut result = Vec::new();
    let mut i = 0;
    // Track which operation type we're currently inside
    let mut in_column_op = false;
    let mut in_ids_list = false;

    while i < lines.len() {
        let trimmed = lines[i].trim();

        // Track operation type context (may appear as "type: X" or "- type: X" in YAML lists)
        let type_value = if trimmed.starts_with("- type: ") {
            Some(trimmed.trim_start_matches("- type: "))
        } else if trimmed.starts_with("type: ") {
            Some(trimmed.trim_start_matches("type: "))
        } else {
            None
        };
        if let Some(op_type) = type_value {
            in_column_op = matches!(
                op_type,
                "renameColumn" | "castColumn" | "addColumn" | "dropColumn"
            );
        }

        // Skip `columnId:` (singular) — appears on AddColumn etc.
        if trimmed.starts_with("columnId:") && !trimmed.starts_with("columnIds:") {
            i += 1;
            continue;
        }
        // `columnIds:` on AttachBlock is a content-addressed sidecar path
        // (or, in older serializations, an inline list) and varies with
        // the randomly-generated ColumnIds.
        if trimmed.starts_with("columnIds:") {
            in_ids_list = true;
            i += 1;
            continue;
        }

        // Skip `id:` field in column operations (Rename/Cast/AddColumn/DropColumn)
        if in_column_op && trimmed.starts_with("id: ") {
            i += 1;
            continue;
        }

        // Skip list items under ids/columnIds
        if in_ids_list {
            if trimmed.starts_with("- ") && !trimmed.contains(": ") {
                i += 1;
                continue;
            }
            in_ids_list = false;
        }

        result.push(lines[i]);
        i += 1;
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
