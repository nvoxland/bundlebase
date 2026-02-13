//! RowId module for row position encoding and batch handling.

use crate::object_id::ObjectIdAlias;
use crate::BundlebaseError;
use arrow::record_batch::RecordBatch;
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

/// RowId encodes row position and metadata as a u64:
/// - Bits 63-60 (4 bits): Reserved (always 0)
/// - Bits 59-44 (16 bits): ObjectIdAlias (compact reference to a block)
/// - Bits 43-37 (7 bits): Reserved (always 0)
/// - Bits 36-32 (5 bits): Row size in megabytes, rounded up (0-31 MB)
/// - Bits 31-0 (32 bits): Byte offset in file (up to 4 GB)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RowId(u64);

impl RowId {
    /// Create a new RowId from a block ref, byte offset, and row size
    ///
    /// # Arguments
    /// * `block_ref` - Compact reference identifying the data source
    /// * `offset` - Byte offset where the row starts in the file
    /// * `size_bytes` - Size of the row in bytes
    ///
    /// # Returns
    /// A new RowId with size rounded up to the nearest MB and capped at 31 MB
    pub fn new(block_ref: ObjectIdAlias, offset: u64, size_bytes: usize) -> Self {
        // Round up bytes to whole MB, cap at 31 MB for 5-bit field
        let size_mb = if size_bytes == 0 {
            0
        } else {
            ((size_bytes + 1_048_575) / 1_048_576).min(31) as u64
        };

        // Mask offset to 32 bits (bits 0-31)
        let offset = offset & 0x0000_0000_FFFF_FFFF;

        // Get block_ref as u16
        let block_ref_val = block_ref.as_u16() as u64;

        // Pack: [4 reserved (0)][16 block_ref][7 reserved (0)][5 size][32 offset]
        let packed = (block_ref_val << 44) | (size_mb << 32) | offset;
        RowId(packed)
    }

    /// Extract the byte offset from this RowId
    pub fn offset(&self) -> u64 {
        self.0 & 0x0000_0000_FFFF_FFFF
    }

    /// Extract the ObjectIdAlias from this RowId
    pub fn block_ref(&self) -> ObjectIdAlias {
        let id = ((self.0 >> 44) & 0xFFFF) as u16;
        ObjectIdAlias::from(id)
    }

    /// Replace the block_ref bits with a new ObjectIdAlias, returning a new RowId
    pub fn with_block_ref(self, new_ref: ObjectIdAlias) -> RowId {
        // Clear the block_ref bits (59-44) and set the new value
        let cleared = self.0 & !(0xFFFF_u64 << 44);
        let new_val = (new_ref.as_u16() as u64) << 44;
        RowId(cleared | new_val)
    }

    /// Extract the size in megabytes from this RowId
    pub fn size_mb(&self) -> u8 {
        ((self.0 >> 32) & 0x1F) as u8
    }

    /// Estimate the byte size from the encoded MB value
    /// Note: This is an estimate since actual size is rounded up to MB boundaries
    pub fn size_bytes(&self) -> usize {
        (self.size_mb() as usize) * 1_048_576
    }

    /// Get the raw u64 value
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
            "RowId(source={}, offset={}, size={}MB)",
            self.block_ref().as_u16(),
            self.offset(),
            self.size_mb()
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
    pub fn new(batch: RecordBatch, row_ids: Vec<RowId>) -> Self {
        assert_eq!(
            batch.num_rows(),
            row_ids.len(),
            "Number of rows must match number of row IDs"
        );
        Self { batch, row_ids }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== RowId Tests =====

    #[test]
    fn test_rowid_new_zero_size() {
        let block_ref = ObjectIdAlias::from(5u16);
        let row_id = RowId::new(block_ref, 1000, 0);
        assert_eq!(row_id.block_ref(), block_ref);
        assert_eq!(row_id.offset(), 1000);
        assert_eq!(row_id.size_mb(), 0);
    }

    #[test]
    fn test_rowid_new_small_row() {
        // 500 bytes should round up to 1 MB
        let block_ref = ObjectIdAlias::from(10u16);
        let row_id = RowId::new(block_ref, 1000, 500);
        assert_eq!(row_id.block_ref(), block_ref);
        assert_eq!(row_id.offset(), 1000);
        assert_eq!(row_id.size_mb(), 1);
    }

    #[test]
    fn test_rowid_new_exact_mb() {
        let block_ref = ObjectIdAlias::from(20u16);
        let row_id = RowId::new(block_ref, 1000, 1_048_576);
        assert_eq!(row_id.block_ref(), block_ref);
        assert_eq!(row_id.offset(), 1000);
        assert_eq!(row_id.size_mb(), 1);
    }

    #[test]
    fn test_rowid_new_large_row() {
        // 5.5 MB should round up to 6 MB
        let block_ref = ObjectIdAlias::from(30u16);
        let row_id = RowId::new(block_ref, 2000, 5_767_168);
        assert_eq!(row_id.block_ref(), block_ref);
        assert_eq!(row_id.offset(), 2000);
        assert_eq!(row_id.size_mb(), 6);
    }

    #[test]
    fn test_rowid_new_exceeds_max() {
        // 32 MB should cap at 31 MB
        let block_ref = ObjectIdAlias::from(40u16);
        let row_id = RowId::new(block_ref, 3000, 33_554_432);
        assert_eq!(row_id.block_ref(), block_ref);
        assert_eq!(row_id.offset(), 3000);
        assert_eq!(row_id.size_mb(), 31);
    }

    #[test]
    fn test_rowid_offset_extraction() {
        let block_ref = ObjectIdAlias::from(50u16);
        // Max 32-bit offset (4 GB)
        let max_offset = 0x0000_0000_FFFF_FFFF;
        let row_id = RowId::new(block_ref, max_offset, 1000);
        assert_eq!(row_id.offset(), max_offset);
    }

    #[test]
    fn test_rowid_size_mb_extraction() {
        let block_ref = ObjectIdAlias::from(60u16);
        for size_mb in 0..=31 {
            let size_bytes = (size_mb as usize) * 1_048_576;
            let row_id = RowId::new(block_ref, 1000, size_bytes);
            assert_eq!(row_id.size_mb(), size_mb);
        }
    }

    #[test]
    fn test_rowid_reserved_bits_zero() {
        // Verify that bits 60-63 are always 0
        for offset in [0, 100, 1000] {
            let block_ref = ObjectIdAlias::from(70u16);
            let row_id = RowId::new(block_ref, offset, 1_000_000);
            let high_bits = (row_id.as_u64() >> 60) & 0xF;
            assert_eq!(high_bits, 0, "Reserved bits should be 0");
        }
    }

    #[test]
    fn test_rowid_max_offset() {
        // Test maximum 32-bit offset (4 GB)
        let block_ref = ObjectIdAlias::from(80u16);
        let max_offset = 0x0000_0000_FFFF_FFFF;
        let row_id = RowId::new(block_ref, max_offset, 1_048_576);
        assert_eq!(row_id.offset(), max_offset);
    }

    #[test]
    fn test_rowid_block_ref_extraction() {
        // Test representative u16 ObjectIdAlias values
        let test_ids: Vec<u16> = (0..=255)
            .chain([256, 1000, 10000, 32768, 65535].iter().copied())
            .collect();
        for id in test_ids {
            let block_ref = ObjectIdAlias::from(id);
            let row_id = RowId::new(block_ref, 1000, 2_000_000);
            assert_eq!(row_id.block_ref(), block_ref, "Failed for id={}", id);
        }
    }

    #[test]
    fn test_rowid_from_u64_conversion() {
        let value = 0x123456789ABCDEF0u64;
        let row_id = RowId::from(value);
        assert_eq!(row_id.as_u64(), value);
    }

    #[test]
    fn test_rowid_into_u64_conversion() {
        let block_ref = ObjectIdAlias::from(90u16);
        let row_id = RowId::new(block_ref, 2000, 2_000_000);
        let value: u64 = row_id.into();
        assert_eq!(value, row_id.as_u64());
    }

    #[test]
    fn test_rowid_size_bytes_estimate() {
        // 5_000_000 bytes = 4.768... MB, rounds up to 5 MB
        let block_ref = ObjectIdAlias::from(100u16);
        let row_id = RowId::new(block_ref, 1000, 5_000_000);
        let estimated = row_id.size_bytes();
        // Should be 5 MB * 1_048_576 bytes
        assert_eq!(estimated, 5 * 1_048_576);
    }

    #[test]
    fn test_rowid_clone_and_copy() {
        let block_ref = ObjectIdAlias::from(110u16);
        let original = RowId::new(block_ref, 2000, 2_000_000);
        let cloned = original.clone();
        let copied = original;

        assert_eq!(original, cloned);
        assert_eq!(original, copied);
    }

    #[test]
    fn test_rowid_equality() {
        let block_ref1 = ObjectIdAlias::from(120u16);
        let block_ref2 = ObjectIdAlias::from(121u16);

        let row_id1 = RowId::new(block_ref1, 2000, 2_000_000);
        let row_id2 = RowId::new(block_ref1, 2000, 2_000_000);
        let row_id3 = RowId::new(block_ref2, 2000, 2_000_000);

        assert_eq!(row_id1, row_id2);
        assert_ne!(row_id1, row_id3);
    }

    #[test]
    fn test_rowid_hash() {
        use std::collections::HashSet;

        let block_ref = ObjectIdAlias::from(130u16);
        let row_id1 = RowId::new(block_ref, 2000, 2_000_000);
        let row_id2 = RowId::new(block_ref, 2000, 2_000_000);

        let mut set = HashSet::new();
        set.insert(row_id1);
        assert!(set.contains(&row_id2));
    }

    #[test]
    fn test_rowid_with_block_ref() {
        let ref1 = ObjectIdAlias::from(10u16);
        let ref2 = ObjectIdAlias::from(99u16);
        let row_id = RowId::new(ref1, 500, 1024);

        let remapped = row_id.with_block_ref(ref2);
        assert_eq!(remapped.block_ref(), ref2);
        assert_eq!(remapped.offset(), 500);
        assert_eq!(remapped.size_mb(), row_id.size_mb());
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

        let rowid_batch = RowIdBatch::new(batch.clone(), row_ids.clone());

        assert_eq!(rowid_batch.batch.num_rows(), 3);
        assert_eq!(rowid_batch.row_ids.len(), 3);
        assert_eq!(rowid_batch.row_ids[0], RowId::from(0u64));
    }

    #[test]
    #[should_panic(expected = "Number of rows must match")]
    fn test_rowid_batch_mismatch() {
        use arrow::array::Int32Array;
        use arrow::datatypes::{DataType, Field, Schema};
        use std::sync::Arc;

        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int32, false)]));
        let array = Arc::new(Int32Array::from(vec![1, 2, 3]));
        let batch = RecordBatch::try_new(schema, vec![array]).unwrap();

        let row_ids = vec![RowId::from(0u64), RowId::from(1u64)]; // Only 2, but batch has 3

        RowIdBatch::new(batch, row_ids);
    }
}
