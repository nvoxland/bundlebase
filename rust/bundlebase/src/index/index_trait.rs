//! Common trait for index implementations
//!
//! This module defines the `Index` trait that provides a polymorphic interface
//! for different index types (ColumnIndex, TextColumnIndex, etc.).

use crate::index::IndexType;
use crate::BundlebaseError;
use bytes::Bytes;

/// Common trait for all index implementations.
///
/// This trait enables polymorphic handling of different index types,
/// reducing code duplication when working with indexes.
pub trait Index: Send + Sync + std::fmt::Debug {
    /// Serialize the index to bytes for storage
    fn serialize(&self) -> Result<Bytes, BundlebaseError>;

    /// Get the cardinality (number of distinct values) in the index
    fn cardinality(&self) -> u64;

    /// Get the name of the column this index is for
    fn column_name(&self) -> &str;

    /// Get the index type (Column or Text)
    fn index_type(&self) -> IndexType;

    /// Get the total number of rows indexed
    fn total_rows(&self) -> u64;
}
