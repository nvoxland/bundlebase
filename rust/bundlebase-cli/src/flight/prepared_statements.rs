//! Prepared statement management for Flight SQL.

use arrow::datatypes::SchemaRef;

/// Stored prepared statement information.
pub struct PreparedStatement {
    pub sql: String,
    pub schema: SchemaRef,
}
