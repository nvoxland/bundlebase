use arrow::array::{Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use bundlebase_sdk::{Connector, Location};
use std::collections::HashMap;
use std::sync::Arc;

struct TestSource;

impl Connector for TestSource {
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
                num_rows: Some(3),
            },
            Location {
                location: "test_file_2.parquet".into(),
                must_copy: true,
                format: "parquet".into(),
                version: "v1".into(),
                num_rows: Some(2),
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

fn main() {
    bundlebase_sdk::serve(&TestSource);
}
