use arrow::array::{RecordBatch, StringArray, UInt64Array};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use bundlebase::bundle::BundleFacade;
use bundlebase::bundle::Source;
use bundlebase_common::save_as::SaveAs;
use datafusion::catalog::Session;
use datafusion::datasource::{MemTable, TableProvider, TableType};
use datafusion::error::Result;
use datafusion::logical_expr::Expr;
use datafusion::physical_plan::ExecutionPlan;
use std::any::Any;
use std::collections::BTreeMap;
use std::sync::{Arc, Weak};

/// TableProvider that exposes configured sources in the `bundle_info.sources` table.
pub(super) struct BundleSourcesTable {
    facade: Weak<dyn BundleFacade>,
    schema: SchemaRef,
}

impl std::fmt::Debug for BundleSourcesTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BundleSourcesTable")
            .field("schema", &self.schema)
            .finish()
    }
}

impl BundleSourcesTable {
    pub fn new(facade: Weak<dyn BundleFacade>) -> Self {
        Self {
            facade,
            schema: Self::table_schema(),
        }
    }

    fn facade(&self) -> Result<Arc<dyn BundleFacade>> {
        self.facade.upgrade().ok_or_else(|| {
            datafusion::error::DataFusionError::Internal(
                "Bundle has been dropped (while accessing bundle_info.sources)".to_string(),
            )
        })
    }

    fn table_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("pack_id", DataType::Utf8, false),
            Field::new("pack_name", DataType::Utf8, true),
            Field::new("connector", DataType::Utf8, false),
            Field::new("save_as", DataType::Utf8, false),
            Field::new("min_batch_bytes", DataType::UInt64, true),
            Field::new("args", DataType::Utf8, false),
        ]))
    }

    fn save_as_name(save_as: &SaveAs) -> &'static str {
        match save_as {
            SaveAs::Auto => "auto",
            SaveAs::Copy => "copy",
            SaveAs::Parquet => "parquet",
            SaveAs::Ref => "ref",
        }
    }

    fn source_args_json(source: &Source) -> Result<String> {
        let sorted: BTreeMap<String, String> = source
            .args()
            .iter()
            .map(|(key, value): (&String, &String)| (key.clone(), value.clone()))
            .collect();

        serde_json::to_string(&sorted).map_err(|e| {
            datafusion::error::DataFusionError::Internal(format!(
                "Failed to serialize source args for {}: {}",
                source.id(),
                e
            ))
        })
    }

    fn build_batch(&self) -> Result<RecordBatch> {
        let facade = self.facade()?;
        let packs = facade.packs();
        let sources = facade.sources();

        let mut source_list: Vec<_> = sources.values().collect();
        source_list.sort_by_key(|source| *source.id());

        let ids: Vec<String> = source_list
            .iter()
            .map(|source| source.id().to_string())
            .collect();
        let pack_ids: Vec<String> = source_list
            .iter()
            .map(|source| source.pack().to_string())
            .collect();
        let pack_names: Vec<Option<String>> = source_list
            .iter()
            .map(|source| packs.get(source.pack()).map(|pack| pack.name().to_string()))
            .collect();
        let connectors: Vec<String> = source_list
            .iter()
            .map(|source| source.connector())
            .collect();
        let save_as: Vec<&str> = source_list
            .iter()
            .map(|source| Self::save_as_name(source.save_as()))
            .collect();
        let min_batch_bytes: Vec<Option<u64>> = source_list
            .iter()
            .map(|source| source.min_batch_bytes().map(|value| value as u64))
            .collect();
        let args: Vec<String> = source_list
            .iter()
            .map(|source| Self::source_args_json(source))
            .collect::<Result<Vec<_>>>()?;

        RecordBatch::try_new(
            Arc::clone(&self.schema),
            vec![
                Arc::new(StringArray::from(
                    ids.iter().map(|value| value.as_str()).collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    pack_ids
                        .iter()
                        .map(|value| value.as_str())
                        .collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(pack_names)),
                Arc::new(StringArray::from(
                    connectors
                        .iter()
                        .map(|value| value.as_str())
                        .collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(save_as)),
                Arc::new(UInt64Array::from(min_batch_bytes)),
                Arc::new(StringArray::from(
                    args.iter().map(|value| value.as_str()).collect::<Vec<_>>(),
                )),
            ],
        )
        .map_err(Into::into)
    }
}

#[async_trait]
impl TableProvider for BundleSourcesTable {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        Arc::clone(&self.schema)
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        let batch = self.build_batch()?;
        let mem_table = MemTable::try_new(self.schema.clone(), vec![vec![batch]])?;
        mem_table.scan(state, projection, filters, limit).await
    }
}
