//! Prepared statement management for Flight SQL.

use arrow::datatypes::SchemaRef;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// Stored prepared statement information.
pub struct PreparedStatement {
    pub sql: String,
    pub schema: SchemaRef,
}

/// Thread-safe storage for prepared statements.
pub type PreparedStatementStore = Arc<RwLock<HashMap<String, PreparedStatement>>>;

/// Create a new empty prepared statement store.
pub fn new_store() -> PreparedStatementStore {
    Arc::new(RwLock::new(HashMap::new()))
}
