//! text_search UDF for full-text search in SQL queries
//!
//! This module provides a DataFusion scalar UDF that enables BM25-based full-text
//! search within SQL queries. The function signature is:
//!
//! ```sql
//! text_search(column, query) -> BOOLEAN
//! ```
//!
//! Example usage:
//! ```sql
//! SELECT * FROM data WHERE text_search(content, 'machine learning') LIMIT 100
//! ```
//!
//! ## Architecture
//!
//! The text_search function is implemented in two parts:
//!
//! 1. **UDF Registration (this module)**: Registers the function with DataFusion so
//!    queries with `text_search()` parse correctly and have proper type validation.
//!    The actual UDF returns placeholder values because DataFusion UDFs are synchronous
//!    and cannot perform async index lookups.
//!
//! 2. **FilterAnalyzer Integration**: The `FilterAnalyzer` (in `index/filter_analyzer.rs`)
//!    detects `text_search()` calls in WHERE clauses, extracts the arguments using
//!    `extract_text_search_args()`, and uses the text index to get matching row IDs
//!    before query execution. This allows efficient index-based filtering.
//!
//! This split design allows SQL queries to use text_search() syntax while actual
//! filtering happens through the index-aware execution layer.

use crate::index::TextColumnIndex;
use crate::io::IOReadDir;
use crate::BundlebaseError;
use arrow::array::BooleanArray;
use arrow::datatypes::DataType;
use datafusion::common::Result as DFResult;
use datafusion::logical_expr::{ColumnarValue, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature, TypeSignature, Volatility};
use parking_lot::RwLock;
use std::any::Any;
use std::collections::HashSet;
use std::sync::Arc;

/// State for the text_search UDF containing access to text indexes.
///
/// This state is used by the FilterAnalyzer to perform text searches during
/// query planning, not by the UDF itself (which returns placeholder values).
#[derive(Debug)]
pub struct TextSearchUdfState {
    /// Text indexes by column name
    text_indexes: RwLock<Vec<TextIndexEntry>>,
    /// Data directory for loading index files
    data_dir: Arc<dyn IOReadDir>,
}

/// Entry representing a registered text index
#[derive(Debug)]
struct TextIndexEntry {
    column_name: String,
    index_path: String,
}

impl TextSearchUdfState {
    /// Create a new UDF state
    pub fn new(data_dir: Arc<dyn IOReadDir>) -> Self {
        Self {
            text_indexes: RwLock::new(Vec::new()),
            data_dir,
        }
    }

    /// Register a text index for a column
    pub fn register_index(&self, column_name: &str, index_path: &str) {
        let mut indexes = self.text_indexes.write();
        // Check if already registered
        if !indexes.iter().any(|e| e.column_name == column_name) {
            indexes.push(TextIndexEntry {
                column_name: column_name.to_string(),
                index_path: index_path.to_string(),
            });
        }
    }

    /// Get row IDs matching a text search query.
    ///
    /// This method is called by the FilterAnalyzer during query planning,
    /// not by the UDF itself.
    pub async fn search(
        &self,
        column: &str,
        query: &str,
        limit: usize,
    ) -> Result<HashSet<u64>, BundlebaseError> {
        // Find the index entry for this column
        let index_path = {
            let indexes = self.text_indexes.read();
            indexes
                .iter()
                .find(|e| e.column_name == column)
                .map(|e| e.index_path.clone())
        };

        let index_path = match index_path {
            Some(p) => p,
            None => {
                return Err(BundlebaseError::from(format!(
                    "No text index found for column '{}'",
                    column
                )));
            }
        };

        // Load and search the index
        let index = self.load_index(&index_path).await?;
        let results = index.search_rowids(query, limit)?;

        // Convert to HashSet of u64 for efficient lookup
        Ok(results.into_iter().map(|r| r.as_u64()).collect())
    }

    /// Load a text index from the data directory
    async fn load_index(&self, path: &str) -> Result<TextColumnIndex, BundlebaseError> {
        let file = self.data_dir.file(path)?;

        let data = file
            .read_bytes()
            .await
            .map_err(|e| BundlebaseError::from(format!("Failed to read text index: {}", e)))?
            .ok_or_else(|| BundlebaseError::from("Text index file is empty or does not exist"))?;

        TextColumnIndex::deserialize(data)
    }
}

/// The text_search scalar UDF implementation.
///
/// This UDF is registered with DataFusion to enable `text_search(column, query)` syntax
/// in SQL queries. The UDF itself returns placeholder values - actual filtering is done
/// by the FilterAnalyzer which intercepts text_search calls and performs index lookups.
#[derive(Debug)]
pub struct TextSearchUdf {
    signature: Signature,
}

// Implement PartialEq and Hash for ScalarUDFImpl requirements
impl PartialEq for TextSearchUdf {
    fn eq(&self, other: &Self) -> bool {
        self.name() == other.name()
    }
}

impl Eq for TextSearchUdf {}

impl std::hash::Hash for TextSearchUdf {
    fn hash<H: std::hash::Hasher>(&self, hasher: &mut H) {
        self.name().hash(hasher);
    }
}

impl TextSearchUdf {
    /// Create a new text_search UDF
    pub fn new() -> Self {
        Self {
            // text_search(column: Utf8, query: Utf8) -> Boolean
            signature: Signature::new(
                TypeSignature::Exact(vec![DataType::Utf8, DataType::Utf8]),
                Volatility::Stable,
            ),
        }
    }

    /// Create the UDF as a ScalarUDF
    pub fn create_udf() -> ScalarUDF {
        ScalarUDF::new_from_impl(TextSearchUdf::new())
    }
}

impl Default for TextSearchUdf {
    fn default() -> Self {
        Self::new()
    }
}

impl ScalarUDFImpl for TextSearchUdf {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "text_search"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _arg_types: &[DataType]) -> DFResult<DataType> {
        Ok(DataType::Boolean)
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> DFResult<ColumnarValue> {
        // For now, this is a placeholder that returns false for all rows.
        // The actual text search filtering happens at a higher level using
        // the FilterAnalyzer to detect text_search calls and apply row ID filtering.
        //
        // This UDF implementation is here to:
        // 1. Register the function with DataFusion so queries parse correctly
        // 2. Provide proper type signature validation
        //
        // In the future, we could implement direct evaluation here for small datasets.

        let args = &args.args;
        if args.len() != 2 {
            return Err(datafusion::error::DataFusionError::Internal(
                "text_search requires exactly 2 arguments".to_string(),
            ));
        }

        // Get the batch size to return the correct number of results
        let len = match &args[0] {
            ColumnarValue::Array(arr) => arr.len(),
            ColumnarValue::Scalar(_) => 1,
        };

        // Return an array of false values
        // The actual filtering is done by the index-aware execution layer
        let result = BooleanArray::from(vec![false; len]);
        Ok(ColumnarValue::Array(Arc::new(result)))
    }
}

/// Helper function to check if an expression contains a text_search call
/// and extract the column name and query if it does.
///
/// This is used by the FilterAnalyzer to optimize text search queries
/// by using the index before executing the full query.
pub fn extract_text_search_args(
    expr: &datafusion::logical_expr::Expr,
) -> Option<(String, String)> {
    use datafusion::logical_expr::Expr;

    match expr {
        Expr::ScalarFunction(func) if func.name() == "text_search" => {
            if func.args.len() == 2 {
                // Extract column name (first arg should be a column reference)
                let column = match &func.args[0] {
                    Expr::Column(col) => Some(col.name.clone()),
                    _ => None,
                }?;

                // Extract query string (second arg should be a literal string)
                let query = match &func.args[1] {
                    Expr::Literal(datafusion::scalar::ScalarValue::Utf8(Some(s)), _) => {
                        Some(s.clone())
                    }
                    Expr::Literal(datafusion::scalar::ScalarValue::LargeUtf8(Some(s)), _) => {
                        Some(s.clone())
                    }
                    _ => None,
                }?;

                Some((column, query))
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::logical_expr::Expr;
    use datafusion::prelude::col;
    use datafusion::scalar::ScalarValue;

    #[test]
    fn test_extract_text_search_args() {
        // Create a text_search function call expression
        let text_search_expr = Expr::ScalarFunction(datafusion::logical_expr::expr::ScalarFunction {
            func: Arc::new(TextSearchUdf::create_udf()),
            args: vec![
                col("content"),
                Expr::Literal(ScalarValue::Utf8(Some("machine learning".to_string())), None),
            ],
        });

        let result = extract_text_search_args(&text_search_expr);
        assert!(result.is_some());

        let (column, query) = result.expect("Should extract args");
        assert_eq!(column, "content");
        assert_eq!(query, "machine learning");
    }
}
