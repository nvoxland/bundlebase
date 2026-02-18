//! `search()` table function for BM25 full-text search
//!
//! Provides a table function that replaces `FROM bundle` with search results
//! including BM25 scores. Usage:
//!
//! ```sql
//! SELECT Company, City, _score
//! FROM search('my_search', 'company:group AND city:east')
//! ORDER BY _score DESC
//! LIMIT 10
//! ```

use crate::bundle::{BundleFacade, Pack};
use crate::data::{ObjectId, RowId};
use crate::index::{IndexDefinition, TextColumnIndex};
use crate::io::plugin::object_store::ObjectStoreFile;
use crate::io::IOReadFile;
use arrow::array::{Float64Array, RecordBatch, UInt64Array};
use arrow::compute;
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use datafusion::catalog::{Session, TableFunctionImpl, TableProvider};
use datafusion::datasource::TableType;
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{Expr, TableProviderFilterPushDown};
use datafusion::physical_plan::memory::LazyMemoryExec;
use datafusion::physical_plan::ExecutionPlan;
use futures::StreamExt;
use parking_lot::RwLock;
use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Weak};

/// Table function that creates a `SearchResultTableProvider` for text search
pub struct SearchTableFunction {
    facade: Weak<dyn BundleFacade>,
}

impl std::fmt::Debug for SearchTableFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchTableFunction").finish()
    }
}

impl SearchTableFunction {
    pub fn new(facade: Weak<dyn BundleFacade>) -> Self {
        Self { facade }
    }
}

impl TableFunctionImpl for SearchTableFunction {
    fn call(&self, args: &[Expr]) -> datafusion::common::Result<Arc<dyn TableProvider>> {
        if args.len() != 2 {
            return Err(DataFusionError::Plan(
                "search() requires exactly 2 arguments: search('index_name', 'query')".to_string(),
            ));
        }

        let index_name = match &args[0] {
            Expr::Literal(datafusion::common::ScalarValue::Utf8(Some(s)), _) => s.clone(),
            Expr::Literal(datafusion::common::ScalarValue::Utf8View(Some(s)), _) => s.to_string(),
            other => {
                return Err(DataFusionError::Plan(format!(
                    "search() first argument must be a string literal (index name), got: {:?}",
                    other
                )));
            }
        };

        let query = match &args[1] {
            Expr::Literal(datafusion::common::ScalarValue::Utf8(Some(s)), _) => s.clone(),
            Expr::Literal(datafusion::common::ScalarValue::Utf8View(Some(s)), _) => s.to_string(),
            other => {
                return Err(DataFusionError::Plan(format!(
                    "search() second argument must be a string literal (query), got: {:?}",
                    other
                )));
            }
        };

        let facade = self.facade.upgrade().ok_or_else(|| {
            DataFusionError::Internal("Bundle has been dropped".to_string())
        })?;

        // Look up the index definition to validate it exists
        let indexes = facade.indexes();
        let index_def = indexes
            .iter()
            .find(|idx| {
                idx.is_text() && (idx.name() == index_name || idx.columns().contains(&index_name.to_string()))
            })
            .ok_or_else(|| {
                let available_text: Vec<String> = indexes
                    .iter()
                    .filter(|idx| idx.is_text())
                    .map(|idx| idx.name().to_string())
                    .collect();

                if available_text.is_empty() {
                    DataFusionError::Plan(format!(
                        "Text index '{}' not found. No text indexes exist on this bundle",
                        index_name
                    ))
                } else {
                    DataFusionError::Plan(format!(
                        "Text index '{}' not found. Available text indexes: {}",
                        index_name,
                        available_text.join(", ")
                    ))
                }
            })?;

        let ctx = facade.ctx();
        let data_dir = facade.data_dir();
        let config = facade.config();
        let packs = facade.packs();

        Ok(Arc::new(SearchResultTableProvider {
            index_name,
            query,
            index_def: index_def.clone(),
            ctx,
            data_dir,
            config,
            packs,
        }))
    }
}

/// Table provider that executes a text search and returns matching rows with scores
struct SearchResultTableProvider {
    index_name: String,
    query: String,
    index_def: Arc<IndexDefinition>,
    ctx: Arc<datafusion::prelude::SessionContext>,
    data_dir: Arc<dyn crate::io::IOReadWriteDir>,
    config: Arc<crate::BundleConfig>,
    packs: HashMap<ObjectId, Arc<Pack>>,
}

impl std::fmt::Debug for SearchResultTableProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchResultTableProvider")
            .field("index_name", &self.index_name)
            .field("query", &self.query)
            .finish()
    }
}

impl SearchResultTableProvider {
    fn data_schema(&self) -> SchemaRef {
        for pack in self.packs.values() {
            for block in pack.blocks() {
                return block.schema();
            }
        }
        Arc::new(Schema::empty())
    }

    fn output_schema(&self) -> SchemaRef {
        let data_schema = self.data_schema();
        let mut fields: Vec<Arc<Field>> = data_schema.fields().iter().cloned().collect();
        fields.push(Arc::new(Field::new("_score", DataType::Float64, false)));
        Arc::new(Schema::new(fields))
    }
}

#[async_trait]
impl TableProvider for SearchResultTableProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        self.output_schema()
    }

    fn table_type(&self) -> TableType {
        TableType::Temporary
    }

    fn supports_filters_pushdown(
        &self,
        filters: &[&Expr],
    ) -> datafusion::common::Result<Vec<TableProviderFilterPushDown>> {
        Ok(filters
            .iter()
            .map(|_| TableProviderFilterPushDown::Inexact)
            .collect())
    }

    async fn scan(
        &self,
        _state: &dyn Session,
        projection: Option<&Vec<usize>>,
        _filters: &[Expr],
        limit: Option<usize>,
    ) -> datafusion::common::Result<Arc<dyn ExecutionPlan>> {
        let output_schema = self.output_schema();

        // Step 1: Load text indexes and execute search across all indexed blocks
        let all_indexed_blocks = self.index_def.all_indexed_blocks();

        if all_indexed_blocks.is_empty() {
            return self.empty_exec(&output_schema, projection);
        }

        // Collect (row_id, score) pairs from all indexed blocks
        let mut row_id_scores: Vec<(RowId, f64)> = Vec::new();
        let search_limit = limit.unwrap_or(10000);

        for indexed_blocks in &all_indexed_blocks {
            let index_path = indexed_blocks.path();

            let index_file = ObjectStoreFile::from_str(
                index_path,
                self.data_dir.as_ref(),
                self.config.clone(),
            )
            .map_err(|e| DataFusionError::External(e))?;

            let index_bytes = index_file
                .read_bytes()
                .await
                .map_err(|e| DataFusionError::External(e))?
                .ok_or_else(|| {
                    DataFusionError::Execution(format!(
                        "Text index file not found: {}",
                        index_path
                    ))
                })?;

            let text_index = TextColumnIndex::deserialize(index_bytes)
                .map_err(|e| DataFusionError::External(e))?;

            let results = text_index
                .search(&self.query, search_limit)
                .map_err(|e| DataFusionError::External(e))?;

            for result in results {
                row_id_scores.push((result.row_id, result.score as f64));
            }
        }

        if row_id_scores.is_empty() {
            return self.empty_exec(&output_schema, projection);
        }

        // Sort by score descending
        row_id_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Build a map from row_id to score for lookup
        let score_map: HashMap<u64, f64> = row_id_scores
            .iter()
            .map(|(row_id, score)| (row_id.as_u64(), *score))
            .collect();

        // Scan all blocks to find matching rows
        let mut result_batches: Vec<RecordBatch> = Vec::new();

        for pack in self.packs.values() {
            for block in pack.blocks() {
                let reader = block.reader();
                let block_ref = crate::data::ObjectIdAlias::from(0u16);

                let mut rowid_stream = reader
                    .extract_rowids_stream(block_ref, self.ctx.clone(), None)
                    .await
                    .map_err(|e| DataFusionError::External(e))?;

                while let Some(batch_result) = rowid_stream.next().await {
                    let rowid_batch =
                        batch_result.map_err(|e| DataFusionError::External(e))?;

                    let batch = &rowid_batch.batch;
                    let row_ids = &rowid_batch.row_ids;

                    // Find matching rows in this batch
                    let mut matching_indices: Vec<usize> = Vec::new();
                    let mut matching_scores: Vec<f64> = Vec::new();

                    for (idx, row_id) in row_ids.iter().enumerate() {
                        if let Some(&score) = score_map.get(&row_id.as_u64()) {
                            matching_indices.push(idx);
                            matching_scores.push(score);
                        }
                    }

                    if matching_indices.is_empty() {
                        continue;
                    }

                    // Filter the batch to only matching rows using take
                    let indices = UInt64Array::from(
                        matching_indices
                            .iter()
                            .map(|&i| i as u64)
                            .collect::<Vec<_>>(),
                    );
                    let mut filtered_columns: Vec<Arc<dyn arrow::array::Array>> = Vec::new();
                    for col in batch.columns() {
                        let filtered = compute::take(col.as_ref(), &indices, None)?;
                        filtered_columns.push(filtered);
                    }

                    // Append score column
                    let score_array = Float64Array::from(matching_scores);
                    filtered_columns.push(Arc::new(score_array));

                    let result_batch =
                        RecordBatch::try_new(output_schema.clone(), filtered_columns)?;

                    result_batches.push(result_batch);
                }
            }
        }

        if result_batches.is_empty() {
            return self.empty_exec(&output_schema, projection);
        }

        // Wrap result batches in a LazyMemoryExec
        let generator = SearchBatchGenerator::new(result_batches);
        let exec = LazyMemoryExec::try_new(
            output_schema,
            vec![Arc::new(RwLock::new(generator))],
        )?;

        let exec = if let Some(proj) = projection {
            exec.with_projection(Some(proj.clone()))
        } else {
            exec
        };

        Ok(Arc::new(exec))
    }
}

impl SearchResultTableProvider {
    fn empty_exec(
        &self,
        schema: &SchemaRef,
        projection: Option<&Vec<usize>>,
    ) -> datafusion::common::Result<Arc<dyn ExecutionPlan>> {
        let generator = SearchBatchGenerator::new(vec![]);
        let exec = LazyMemoryExec::try_new(schema.clone(), vec![Arc::new(RwLock::new(generator))])?;
        let exec = if let Some(proj) = projection {
            exec.with_projection(Some(proj.clone()))
        } else {
            exec
        };
        Ok(Arc::new(exec))
    }
}

/// Simple batch generator that yields pre-computed batches
#[derive(Debug)]
struct SearchBatchGenerator {
    batches: Vec<RecordBatch>,
    index: usize,
}

impl SearchBatchGenerator {
    fn new(batches: Vec<RecordBatch>) -> Self {
        Self { batches, index: 0 }
    }
}

impl fmt::Display for SearchBatchGenerator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SearchBatchGenerator({} batches, at {})",
            self.batches.len(),
            self.index
        )
    }
}

impl datafusion::physical_plan::memory::LazyBatchGenerator for SearchBatchGenerator {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn generate_next_batch(
        &mut self,
    ) -> datafusion::common::Result<Option<RecordBatch>> {
        if self.index < self.batches.len() {
            let batch = self.batches[self.index].clone();
            self.index += 1;
            Ok(Some(batch))
        } else {
            Ok(None)
        }
    }

    fn reset_state(&self) -> Arc<RwLock<dyn datafusion::physical_plan::memory::LazyBatchGenerator>> {
        Arc::new(RwLock::new(SearchBatchGenerator::new(self.batches.clone())))
    }
}
