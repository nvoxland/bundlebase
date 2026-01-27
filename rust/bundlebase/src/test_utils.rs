use crate::bundle::Operation;
/// Test utilities for data adapter tests
use crate::data::DataReaderFactory;
use crate::functions::FunctionRegistry;
use crate::io::plugin::object_store::ObjectStoreFile;
use crate::io::{writable_dir_from_url, DataStorage, IOReadWriteDir, IOReadWriteFile};
use crate::{BundleBuilder, BundleConfig, BundleFacade};
use arrow_schema::SchemaRef;
use parking_lot::RwLock;
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use tokio::runtime::Builder;
use url::Url;

/// Singleton test DataAdapterFactory for use across all tests
static TEST_FACTORY: OnceLock<Arc<DataReaderFactory>> = OnceLock::new();
static TEST_DATAFILE_RESPONSES: OnceLock<HashMap<String, String>> = OnceLock::new();

/// Get or create the singleton test DataAdapterFactory
pub fn test_adapter_factory() -> Arc<DataReaderFactory> {
    TEST_FACTORY
        .get_or_init(|| {
            Arc::new(DataReaderFactory::new(
                Arc::new(RwLock::new(FunctionRegistry::new())),
                Arc::new(DataStorage::new()),
            ))
        })
        .clone()
}

/// Serialize an optional string to YAML for testing
pub fn for_yaml(value: String) -> String {
    serde_yaml_ng::to_string(&value).unwrap().trim().to_string()
}

pub async fn empty_bundle() -> Arc<BundleBuilder> {
    BundleBuilder::create(random_memory_url().as_str(), None)
        .await
        .unwrap()
}

pub fn test_datafile(name: &str) -> &'static str {
    let responses = TEST_DATAFILE_RESPONSES.get_or_init(|| {
        // Run the async writes on a fresh thread/runtime so we don't start a Tokio runtime
        // on a thread that's already driving async tasks.
        std::thread::spawn(|| {
            let rt = Builder::new_current_thread().enable_all().build().unwrap();
            let mut map = HashMap::new();
            let data_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .and_then(|p| p.parent())
                .map(|p| p.join("test_data"))
                .unwrap();
            for datafile in data_dir.read_dir().unwrap() {
                let os_path = datafile.unwrap().path();
                let filename = os_path.file_name().unwrap().to_str().unwrap().to_string();
                let bytes = fs::read(os_path).unwrap();

                let url = Url::parse(&format!("memory:///test_data/{}", filename)).unwrap();
                let file = ObjectStoreFile::from_url(&url, BundleConfig::default().into()).unwrap();

                rt.block_on(file.write(bytes.into())).unwrap();

                map.insert(filename, url.to_string());
            }
            map
        })
        .join()
        .expect("thread panicked while initializing test datafiles")
    });

    responses
        .get(name)
        .unwrap_or_else(|| panic!("test_datafile: no datafile `{}`", name))
        .as_str()
}

pub fn random_memory_url() -> Url {
    Url::parse(&format!("memory:///{}", rand::random::<u64>())).unwrap()
}

/// Create a random memory directory for testing.
/// Returns an Arc<dyn IOReadWriteDir> that can be used in tests.
pub fn random_memory_dir() -> Arc<dyn IOReadWriteDir> {
    writable_dir_from_url(&random_memory_url(), BundleConfig::default().into()).unwrap()
}

/// Internal function for unit tests that need the concrete ObjectStoreDir type.
/// This is pub(crate) so it's only available within the crate.
#[cfg(test)]
pub(crate) fn random_memory_dir_concrete() -> crate::io::plugin::object_store::ObjectStoreDir {
    crate::io::plugin::object_store::ObjectStoreDir::from_url(&random_memory_url(), BundleConfig::default().into()).unwrap()
}

/// Create a random memory file for testing.
/// Returns a Box<dyn IOReadWriteFile> that can be used in tests.
pub fn random_memory_file(path: &str) -> Box<dyn IOReadWriteFile> {
    random_memory_dir().writable_file(path).unwrap()
}

/// Macro to extract a field from an AnyOperation enum
///
/// This reduces boilerplate when accessing fields from an  operation in tests.
#[macro_export]
macro_rules! op_field {
    ($operation:expr, $variant:path, $field:ident) => {{
        match &$operation {
            $variant(op) => op.$field.clone(),
            _ => panic!("Expected first operation to be {}", stringify!($variant)),
        }
    }};
}

/// Macro to assert that a string matches a regular expression pattern
///
/// This reduces boilerplate when testing regex matches in tests.
#[macro_export]
macro_rules! assert_regexp {
    ($pattern:expr, $actual:expr) => {{
        let pattern = $pattern;
        let actual = $actual;
        let regex = regex::Regex::new(pattern.trim())
            .unwrap_or_else(|e| panic!("Invalid regex pattern '{}': {}", pattern, e));
        assert!(
            regex.is_match(actual.trim()),
            "Pattern '{}' did not match actual value:\n{}",
            pattern,
            actual
        );
    }};
}

pub fn describe_ops(bundle: &dyn BundleFacade) -> Vec<String> {
    bundle
        .operations()
        .iter()
        .map(|x| x.describe())
        .collect::<Vec<_>>()
}

pub fn assert_vec_regexp(expected: Vec<&str>, actual: Vec<String>) {
    let patterns = expected
        .iter()
        .map(|x| Regex::new(x).unwrap())
        .collect::<Vec<Regex>>();

    assert_eq!(
        expected.len(),
        patterns.len(),
        "Expected patterns length does not match actual length"
    );

    for (desc, pat) in actual.iter().zip(patterns.iter()) {
        assert!(
            pat.is_match(desc),
            "pattern {:?} did not match {:?}",
            pat,
            desc
        );
    }
}

pub fn field_names(schema: &SchemaRef) -> Vec<&str> {
    schema.fields().iter().map(|f| f.name().as_str()).collect()
}
