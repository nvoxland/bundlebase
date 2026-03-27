//! RowId module for logical row identification and batch handling.
//!
//! A RowId uniquely identifies a row within a bundle by its block and
//! sequential row number. Physical resolution (byte offsets, row groups)
//! is handled by the reader/layout layer, not encoded in the RowId.

use crate::object_id::ObjectIdAlias;
use crate::BundlebaseError;
use arrow::record_batch::RecordBatch;
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

/// RowId encodes a logical row position as a u64:
/// - Bits 63-60 (4 bits): Reserved (always 0)
/// - Bits 59-44 (16 bits): ObjectIdAlias (compact reference to a block)
/// - Bits 43-32 (12 bits): Reserved (always 0)
/// - Bits 31-0 (32 bits): Row number within the block (0-indexed, up to ~4 billion rows)
///
/// Physical resolution (byte offsets for CSV, row group mapping for Parquet)
/// is the responsibility of the format-specific reader and layout files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RowId(u64);

impl RowId {
    /// Create a new RowId from a block reference and row number.
    ///
    /// # Arguments
    /// * `block_ref` - Compact reference identifying the data block
    /// * `row_number` - Sequential row number within the block (0-indexed)
    pub fn new(block_ref: ObjectIdAlias, row_number: u32) -> Self {
        let block_ref_val = block_ref.as_u16() as u64;
        // Pack: [4 reserved (0)][16 block_ref][12 reserved (0)][32 row_number]
        let packed = (block_ref_val << 44) | (row_number as u64);
        RowId(packed)
    }

    /// Extract the sequential row number within the block.
    pub fn row_number(&self) -> u32 {
        (self.0 & 0x0000_0000_FFFF_FFFF) as u32
    }

    /// Extract the ObjectIdAlias (block reference) from this RowId.
    pub fn block_ref(&self) -> ObjectIdAlias {
        let id = ((self.0 >> 44) & 0xFFFF) as u16;
        ObjectIdAlias::from(id)
    }

    /// Replace the block_ref bits with a new ObjectIdAlias, returning a new RowId.
    /// Preserves the row number.
    pub fn with_block_ref(self, new_ref: ObjectIdAlias) -> RowId {
        let row_number = self.row_number();
        RowId::new(new_ref, row_number)
    }

    /// Get the raw u64 value.
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

impl From<u64> for RowId {
    fn from(value: u64) -> Self {
        RowId(value)
    }
}

impl From<RowId> for u64 {
    fn from(id: RowId) -> u64 {
        id.0
    }
}

impl std::fmt::Display for RowId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "RowId(block={}, row={})",
            self.block_ref().as_u16(),
            self.row_number()
        )
    }
}

/// Type alias for a stream of RowIdBatches
pub type SendableRowIdBatchStream =
    Pin<Box<dyn Stream<Item = Result<RowIdBatch, BundlebaseError>> + Send>>;

/// Helper function to create a SendableRowIdBatchStream from a stream
pub fn boxed_rowid_stream<S>(stream: S) -> SendableRowIdBatchStream
where
    S: Stream<Item = Result<RowIdBatch, BundlebaseError>> + Send + 'static,
{
    Box::pin(stream)
}

/// A record batch paired with RowIds for index building
/// Used by extract_rowids_stream() to pass both data and row position info
#[derive(Debug)]
pub struct RowIdBatch {
    /// The actual data
    pub batch: RecordBatch,
    /// RowId for each row in the batch, in order
    /// row_ids[i] corresponds to batch.row(i)
    pub row_ids: Vec<RowId>,
}

impl RowIdBatch {
    pub fn new(batch: RecordBatch, row_ids: Vec<RowId>) -> Result<Self, BundlebaseError> {
        if batch.num_rows() != row_ids.len() {
            return Err(format!(
                "Number of rows ({}) must match number of row IDs ({})",
                batch.num_rows(),
                row_ids.len()
            )
            .into());
        }
        Ok(Self { batch, row_ids })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== RowId Tests =====

    #[test]
    fn test_rowid_new() {
        let block_ref = ObjectIdAlias::from(5u16);
        let row_id = RowId::new(block_ref, 42);
        assert_eq!(row_id.block_ref(), block_ref);
        assert_eq!(row_id.row_number(), 42);
    }

    #[test]
    fn test_rowid_row_zero() {
        let block_ref = ObjectIdAlias::from(1u16);
        let row_id = RowId::new(block_ref, 0);
        assert_eq!(row_id.row_number(), 0);
        assert_eq!(row_id.block_ref(), block_ref);
    }

    #[test]
    fn test_rowid_max_row_number() {
        let block_ref = ObjectIdAlias::from(10u16);
        let row_id = RowId::new(block_ref, u32::MAX);
        assert_eq!(row_id.row_number(), u32::MAX);
        assert_eq!(row_id.block_ref(), block_ref);
    }

    #[test]
    fn test_rowid_reserved_bits_zero() {
        for row_num in [0u32, 100, 1000, u32::MAX] {
            let block_ref = ObjectIdAlias::from(70u16);
            let row_id = RowId::new(block_ref, row_num);
            // Bits 63-60 should be 0
            let high_bits = (row_id.as_u64() >> 60) & 0xF;
            assert_eq!(high_bits, 0, "High reserved bits should be 0");
            // Bits 43-32 should be 0
            let mid_bits = (row_id.as_u64() >> 32) & 0xFFF;
            assert_eq!(mid_bits, 0, "Mid reserved bits should be 0");
        }
    }

    #[test]
    fn test_rowid_block_ref_extraction() {
        let test_ids: Vec<u16> = (0..=255)
            .chain([256, 1000, 10000, 32768, 65535].iter().copied())
            .collect();
        for id in test_ids {
            let block_ref = ObjectIdAlias::from(id);
            let row_id = RowId::new(block_ref, 1000);
            assert_eq!(row_id.block_ref(), block_ref, "Failed for id={}", id);
        }
    }

    #[test]
    fn test_rowid_from_u64_conversion() {
        let value = (10u64 << 44) | 42; // block_ref=10, row_number=42
        let row_id = RowId::from(value);
        assert_eq!(row_id.as_u64(), value);
        assert_eq!(row_id.block_ref(), ObjectIdAlias::from(10u16));
        assert_eq!(row_id.row_number(), 42);
    }

    #[test]
    fn test_rowid_into_u64_conversion() {
        let block_ref = ObjectIdAlias::from(90u16);
        let row_id = RowId::new(block_ref, 2000);
        let value: u64 = row_id.into();
        assert_eq!(value, row_id.as_u64());
    }

    #[test]
    fn test_rowid_clone_and_copy() {
        let block_ref = ObjectIdAlias::from(110u16);
        let original = RowId::new(block_ref, 2000);
        let cloned = original.clone();
        let copied = original;
        assert_eq!(original, cloned);
        assert_eq!(original, copied);
    }

    #[test]
    fn test_rowid_equality() {
        let block_ref1 = ObjectIdAlias::from(120u16);
        let block_ref2 = ObjectIdAlias::from(121u16);

        let row_id1 = RowId::new(block_ref1, 2000);
        let row_id2 = RowId::new(block_ref1, 2000);
        let row_id3 = RowId::new(block_ref2, 2000);

        assert_eq!(row_id1, row_id2);
        assert_ne!(row_id1, row_id3);
    }

    #[test]
    fn test_rowid_hash() {
        use std::collections::HashSet;

        let block_ref = ObjectIdAlias::from(130u16);
        let row_id1 = RowId::new(block_ref, 2000);
        let row_id2 = RowId::new(block_ref, 2000);

        let mut set = HashSet::new();
        set.insert(row_id1);
        assert!(set.contains(&row_id2));
    }

    #[test]
    fn test_rowid_with_block_ref() {
        let ref1 = ObjectIdAlias::from(10u16);
        let ref2 = ObjectIdAlias::from(99u16);
        let row_id = RowId::new(ref1, 500);

        let remapped = row_id.with_block_ref(ref2);
        assert_eq!(remapped.block_ref(), ref2);
        assert_eq!(remapped.row_number(), 500);
    }

    #[test]
    fn test_rowid_display() {
        let row_id = RowId::new(ObjectIdAlias::from(3u16), 42);
        assert_eq!(format!("{}", row_id), "RowId(block=3, row=42)");
    }

    // ===== RowIdBatch Tests =====

    #[test]
    fn test_rowid_batch_creation() {
        use arrow::array::Int32Array;
        use arrow::datatypes::{DataType, Field, Schema};
        use std::sync::Arc;

        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int32, false)]));
        let array = Arc::new(Int32Array::from(vec![1, 2, 3]));
        let batch = RecordBatch::try_new(schema, vec![array]).unwrap();

        let row_ids = vec![RowId::from(0u64), RowId::from(1u64), RowId::from(2u64)];

        let rowid_batch = RowIdBatch::new(batch.clone(), row_ids.clone()).unwrap();

        assert_eq!(rowid_batch.batch.num_rows(), 3);
        assert_eq!(rowid_batch.row_ids.len(), 3);
        assert_eq!(rowid_batch.row_ids[0], RowId::from(0u64));
    }

    #[test]
    fn test_rowid_batch_mismatch() {
        use arrow::array::Int32Array;
        use arrow::datatypes::{DataType, Field, Schema};
        use std::sync::Arc;

        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int32, false)]));
        let array = Arc::new(Int32Array::from(vec![1, 2, 3]));
        let batch = RecordBatch::try_new(schema, vec![array]).unwrap();

        let row_ids = vec![RowId::from(0u64), RowId::from(1u64)]; // Only 2, but batch has 3

        let result = RowIdBatch::new(batch, row_ids);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Number of rows"));
    }
}
