// Example: Build a plugin shared library for Bundlebase.
//
// Add to your Cargo.toml:
//
//   [lib]
//   crate-type = ["cdylib"]
//
// Build with:
//
//   cargo build --release
//
// Use from Python:
//
//   bundle.create_source("plugin", {"call": "lib:target/release/libmy_source.so"})

use arrow::array::{Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use bundlebase_sdk::{export_source, Location, SourceFunction};
use std::collections::HashMap;
use std::sync::Arc;

struct TestSource;

impl SourceFunction for TestSource {
    fn discover(
        &self,
        _attached: &[String],
        _args: &HashMap<String, String>,
    ) -> Result<Vec<Location>, Box<dyn std::error::Error>> {
        Ok(vec![
            Location {
                location: "test_file_1.parquet".into(),
                must_copy: true,
                format: "parquet".into(),
                version: "v1".into(),
            },
            Location {
                location: "test_file_2.parquet".into(),
                must_copy: true,
                format: "parquet".into(),
                version: "v1".into(),
            },
        ])
    }

    fn data(
        &self,
        location: &Location,
        _args: &HashMap<String, String>,
    ) -> Result<Option<Vec<RecordBatch>>, Box<dyn std::error::Error>> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, false),
        ]));

        match location.location.as_str() {
            "test_file_1.parquet" => {
                let batch1 = RecordBatch::try_new(
                    schema.clone(),
                    vec![
                        Arc::new(Int64Array::from(vec![1, 2])),
                        Arc::new(StringArray::from(vec!["alice", "bob"])),
                    ],
                )?;
                let batch2 = RecordBatch::try_new(
                    schema,
                    vec![
                        Arc::new(Int64Array::from(vec![3])),
                        Arc::new(StringArray::from(vec!["charlie"])),
                    ],
                )?;
                Ok(Some(vec![batch1, batch2]))
            }
            "test_file_2.parquet" => {
                let batch = RecordBatch::try_new(
                    schema,
                    vec![
                        Arc::new(Int64Array::from(vec![4, 5])),
                        Arc::new(StringArray::from(vec!["dave", "eve"])),
                    ],
                )?;
                Ok(Some(vec![batch]))
            }
            _ => Ok(None),
        }
    }
}

// This macro generates the extern "C" functions required by the Bundlebase
// plugin source ABI: bundlebase_discover, bundlebase_data, bundlebase_free,
// and bundlebase_stable_url.
export_source!(TestSource);

// Required for Rust example compilation. In a real cdylib crate, no main() is needed.
fn main() {}
