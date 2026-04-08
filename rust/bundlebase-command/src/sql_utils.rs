//! Shared SQL generation utilities for command implementations.

use arrow::datatypes::DataType;
use bundlebase_common::BundlebaseError;

/// Build a `TRY_CAST(col AS <type>)` SQL expression string using DataFusion's own
/// type-name mapping rather than maintaining a hand-rolled Arrow→SQL mapping.
///
/// Falls back to `TRY_CAST(col AS type_name)` if the DataFusion unparser fails.
pub fn build_try_cast_sql(col_sql: &str, data_type: &DataType) -> Result<String, BundlebaseError> {
    use datafusion::logical_expr::{expr::TryCast, Expr};
    use datafusion::prelude::col;
    use datafusion::sql::unparser::expr_to_sql;

    expr_to_sql(&Expr::TryCast(TryCast {
        expr: Box::new(col(col_sql)),
        data_type: data_type.clone(),
    }))
    .map(|e| e.to_string())
    .map_err(|e| BundlebaseError::from(format!("Cannot build cast expression: {}", e)))
}
