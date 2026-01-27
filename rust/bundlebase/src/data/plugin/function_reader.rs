use crate::bundle::BundleFacade;
use crate::data::plugin::ReaderPlugin;
use crate::data::{DataReader, ObjectId, RowId};
use crate::functions::FunctionDataSource;
use crate::functions::FunctionImpl;
use crate::functions::FunctionRegistry;
use crate::BundlebaseError;
use arrow::array::RecordBatch;
use arrow::datatypes::SchemaRef;
use async_trait::async_trait;
use datafusion::common::{DataFusionError, Statistics};
use datafusion::datasource::source::DataSource;
use datafusion::logical_expr::Expr;
use parking_lot::RwLock;
use std::fmt::Debug;
use std::sync::Arc;
use url::Url;

pub struct FunctionPlugin {
    pub function_registry: Arc<RwLock<FunctionRegistry>>,
}

impl FunctionPlugin {
    pub fn new(function_registry: Arc<RwLock<FunctionRegistry>>) -> Self {
        Self { function_registry }
    }
}

#[async_trait]
impl ReaderPlugin for FunctionPlugin {
    async fn reader(
        &self,
        source: &str,
        block_id: &ObjectId,
        _bundle: &dyn BundleFacade,
        _schema: Option<SchemaRef>,
        _layout: Option<String>,
        _expected_version: Option<String>, // Functions use their own versioning, not file-based
    ) -> Result<Option<Arc<dyn DataReader>>, BundlebaseError> {
        if !source.starts_with("function://") {
            return Ok(None);
        }

        let url = Url::parse(source)?;
        let host = match url.host() {
            Some(h) => h.to_string(),
            None => return Err("No function specified".into()),
        };

        let registry = self.function_registry.read();
        let function = match registry.get_function(host.as_str()) {
            Some(s) => s,
            None => return Err(format!("Unknown function: {}", host).into()),
        };

        let implementation = match registry.get_impl(host.as_str()) {
            Some(impl_) => impl_,
            None => {
                let err_msg = format!("Function implementation not set for: {}", host);
                return Err(err_msg.into());
            }
        };

        Ok(Some(Arc::new(FunctionDataReader {
            url,
            output: function.output().clone(),
            implementation,
            block_id: *block_id,
        })))
    }
}

#[derive(Debug)]
struct FunctionDataReader {
    url: Url,
    output: SchemaRef,
    implementation: Arc<dyn FunctionImpl>,
    block_id: ObjectId,
}

#[async_trait]
impl DataReader for FunctionDataReader {
    fn url(&self) -> &Url {
        &self.url
    }

    fn block_id(&self) -> ObjectId {
        self.block_id
    }

    async fn read_schema(&self) -> Result<Option<SchemaRef>, BundlebaseError> {
        Ok(Some(self.output.clone()))
    }

    async fn read_statistics(&self) -> Result<Option<Statistics>, BundlebaseError> {
        use datafusion::common::stats::Precision;

        match self
            .implementation
            .execute(Arc::new(crate::functions::FunctionSignature::new(
                "", // Not used
                self.output.clone(),
            ))) {
            Ok(generator) => {
                // Count total rows by iterating through all pages
                let mut total_rows = 0;
                let mut page = 0;
                loop {
                    match generator.next(page) {
                        Ok(Some(batch)) => {
                            total_rows += batch.num_rows();
                            page += 1;
                        }
                        Ok(None) => break,
                        Err(_) => return Ok(None), // If there's an error, don't provide statistics
                    }
                }

                let stats = Statistics {
                    num_rows: Precision::Exact(total_rows),
                    ..Default::default()
                };
                Ok(Some(stats))
            }
            Err(_) => Ok(None),
        }
    }

    async fn read_version(&self) -> Result<String, BundlebaseError> {
        Ok(self.implementation.version())
    }

    async fn data_source(
        &self,
        projection: Option<&Vec<usize>>,
        _filters: &[Expr],
        _limit: Option<usize>,
        _row_ids: Option<&[RowId]>,
    ) -> Result<Arc<dyn DataSource>, DataFusionError> {
        let generator = self
            .implementation
            .execute(Arc::new(crate::functions::FunctionSignature::new(
                "", // Not used
                self.output.clone(),
            )))
            .map_err(|e| DataFusionError::Internal(format!("Failed to execute function: {}", e)))?;

        let source = FunctionDataSource::new(generator, self.output.clone(), projection.cloned())
            .map_err(|e| {
            DataFusionError::Internal(format!("Failed to create FunctionDataSource: {}", e))
        })?;

        Ok(Arc::new(source))
    }
}

pub trait DataGenerator: Debug + Sync + Send {
    fn next(&self, page: usize) -> Result<Option<RecordBatch>, BundlebaseError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::functions::{FunctionSignature, StaticImpl};
    use crate::{Bundle, BundlebaseError};
    use arrow::array::{downcast_array, record_batch, Int32Array, StringArray};
    use arrow::datatypes::{DataType, Field};
    use arrow_schema::Schema;
    use futures::StreamExt;

    #[tokio::test]
    async fn test_wrong_scheme() -> Result<(), BundlebaseError> {
        // Function plugin should only adapt function:// URLs
        let plugin = FunctionPlugin::new(Arc::new(RwLock::new(FunctionRegistry::new())));

        let binding = Bundle::empty().await?;
        let result = plugin
            .reader("file:///test.csv", &1.into(), &*binding, None, None, None)
            .await?;

        assert!(result.is_none());

        Ok(())
    }

    #[tokio::test]
    async fn test_missing_function_name() -> Result<(), BundlebaseError> {
        // function:// without a function name should error
        let plugin = FunctionPlugin::new(Arc::new(RwLock::new(FunctionRegistry::new())));

        let binding = Bundle::empty().await?;
        let error = plugin
            .reader(
                "function://",
                &1.into(),
                &*binding,
                None,
                None,
                None,
            )
            .await
            .unwrap_err();

        assert!(
            error.to_string().starts_with("No function specified"),
            "Should error when no function specified"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_unknown_function() -> Result<(), BundlebaseError> {
        let plugin = FunctionPlugin::new(Arc::new(RwLock::new(FunctionRegistry::new())));

        let binding = Bundle::empty().await?;
        let error = plugin
            .reader(
                "function://invalid",
                &1.into(),
                &*binding,
                None,
                None,
                None,
            )
            .await
            .unwrap_err();

        assert!(
            error.to_string().starts_with("Unknown function: invalid"),
            "Should error when function doesn't exist"
        );

        Ok(())
    }

    #[tokio::test]
    async fn read() -> Result<(), BundlebaseError> {
        // Test complete function data read and data validation

        // Set up function registry with a test function
        let function_registry = Arc::new(RwLock::new(FunctionRegistry::new()));
        function_registry.write().register(FunctionSignature::new(
            "mock",
            SchemaRef::new(Schema::new(vec![
                Field::new("letter", DataType::Utf8, false),
                Field::new("num", DataType::Int32, false),
            ])),
        ))?;

        function_registry.write().set_impl(
            "mock",
            Arc::new(StaticImpl::new(
                vec![record_batch!(
                    ("letter", Utf8, ["x", "y", "z"]),
                    ("num", Int32, [1_i32, 2_i32, 3_i32])
                )?],
                "mock-v2".to_string(),
            )),
        )?;

        let plugin = FunctionPlugin::new(function_registry.clone());

        let binding = Bundle::empty().await?;
        let reader = plugin
            .reader("function://mock", &1.into(), &*binding, None, None, None)
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        // Expected columns
        let column_names = vec!["letter", "num"];

        // Validate schema
        let schema = reader
            .read_schema()
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected schema"))?;

        let actual_columns: Vec<_> = schema.fields().iter().map(|f| f.name().clone()).collect();

        assert_eq!(
            column_names, actual_columns,
            "Function schema should match expected columns"
        );

        // Validate data reading
        let binding = Bundle::empty().await?;
        let ctx = binding.ctx();
        let ds = reader.data_source(None, &[], None, None).await?;
        let results = ds.open(0, ctx.task_ctx())?;

        let result_columns: Vec<_> = results
            .schema()
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect();

        assert_eq!(
            column_names, result_columns,
            "Data source schema should match expected columns"
        );

        // Validate actual data
        let batches = results.collect::<Vec<_>>().await;
        assert_eq!(1, batches.len(), "Should have one record batch");

        let row1 = batches[0]
            .as_ref()
            .map_err(|e| BundlebaseError::from(e.to_string()))?;

        // Validate "letter" column (index 0)
        assert_eq!(
            "Utf8",
            row1.column(0).data_type().to_string(),
            "letter column should be Utf8 type"
        );

        let letter_array: StringArray = downcast_array(row1.column(0).as_ref());
        let letters: Vec<_> = letter_array.iter().map(|v| v.unwrap()).collect();
        assert_eq!(
            vec!["x", "y", "z"],
            letters,
            "letter values should match expected data"
        );

        // Validate "num" column (index 1)
        assert_eq!(
            "Int32",
            row1.column(1).data_type().to_string(),
            "num column should be Int32 type"
        );

        let num_array: Int32Array = downcast_array(row1.column(1).as_ref());
        let nums: Vec<_> = num_array.iter().map(|v| v.unwrap()).collect();
        assert_eq!(vec![1, 2, 3], nums, "num values should match expected data");

        Ok(())
    }
}
