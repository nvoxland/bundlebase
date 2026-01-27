use crate::bundle::operation::Operation;
use crate::bundle::BundleFacade;
// use crate::progress::ProgressScope; // Temporarily commented out
use crate::{Bundle, BundlebaseError};
use async_trait::async_trait;
use datafusion::error::DataFusionError;
use serde::{Deserialize, Serialize};

// TODO: get rid of
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RebuildIndexOp {
    pub column: String,
}

impl RebuildIndexOp {
    pub async fn setup(column: String) -> Result<Self, BundlebaseError> {
        // TODO
        // // Drop existing index if it exists
        // let _ = ctx.index_manager.drop_index(&self.column).await;
        //
        // // Get column data type from schema
        // let schema = df.schema();
        // let field = schema
        //     .field_with_name(None, &self.column)
        //     .map_err(|e| format!("Column not found: {}", e))?;
        // let data_type = field.data_type().clone();
        //
        // // Build index from DataFrame
        // let mut value_to_rowids: HashMap<IndexedValue, Vec<crate::data::RowId>> =
        //     HashMap::new();
        //
        // // TODO: re-enable when we figure out why it's segfaulting
        // // // Get total rows for progress tracking (if available)
        // // let total_rows = bundle.num_rows().await.ok().map(|n| n as u64);
        // //
        // // // Create progress scope for tracking
        // // let _progress = ProgressScope::new(
        // //     &format!("Rebuilding index on '{}'", self.column),
        // //     total_rows,
        // // );
        //
        // // Stream batches and collect (value, rowid) pairs
        // let mut stream = df.clone().execute_stream().await?;
        // let mut row_index = 0u64;
        //
        // while let Some(batch_result) = stream.next().await {
        //     let batch = batch_result?;
        //
        //     // Get the column array
        //     let col_idx = batch
        //         .schema()
        //         .index_of(&self.column)
        //         .map_err(|e| format!("Column not found in batch: {}", e))?;
        //     let array = batch.column(col_idx);
        //
        //     // Extract values and build mapping
        //     for row in 0..array.len() {
        //         let scalar = ScalarValue::try_from_array(array, row)?;
        //         let indexed_value = IndexedValue::from_scalar(&scalar)?;
        //
        //         // For MVP, use row index as a simple RowId
        //         let row_id = crate::data::RowId::from(row_index + row as u64);
        //
        //         value_to_rowids
        //             .entry(indexed_value)
        //             .or_insert_with(Vec::new)
        //             .push(row_id);
        //     }
        //
        //     row_index += batch.num_rows() as u64;
        //
        //     // // Update progress after each batch
        //     // _progress.update(row_index, None);
        // }
        //
        // // Build the index
        // let index = ColumnIndex::build(self.column.as_str(), &data_type, value_to_rowids)?;
        //
        // // Get source ID from first attached adapter (simplified for MVP)
        // let block_id = ctx
        //     .operations
        //     .iter()
        //     .find_map(|op| {
        //         if let crate::bundle::operation::AnyOperation::AttachBlock(attach_block_op) = op {
        //             Some(attach_block_op.id)
        //         } else {
        //             None
        //         }
        //     })
        //     .unwrap_or_else(|| crate::data::ObjectId::from(0u8));
        //
        // let version = "current".to_string();
        //
        // // Register the index
        // ctx
        //     .index_manager
        //     .register_index(Arc::new(index), &block_id, version.clone())?;
        //
        // // Save to disk
        // let _ = ctx.index_manager.save_index(&self.column).await?;

        Ok(Self { column })
    }
}

#[async_trait]
impl Operation for RebuildIndexOp {
    fn describe(&self) -> String {
        format!("REBUILD INDEX on column '{}'", self.column)
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Verify column exists in schema
        if !bundle
            .schema()
            .await?
            .column_with_name(&self.column)
            .is_some()
        {
            return Err(format!("Column '{}' not found in schema", self.column).into());
        }

        Ok(())
    }

    async fn apply(&self, _bundle: &Bundle) -> Result<(), DataFusionError> {
        // Rebuilding an index doesn't change schema or row count
        Ok(())
    }
}
