//! Tombstone file I/O for DELETE support.
//!
//! Tombstone files track deleted RowIds in a compact binary format.
//! Files are content-addressed: `{sha256_hash_12}.tomb`
//!
//! # File Format
//!
//! ```text
//! Magic:   b"BBRID001" (8 bytes)
//! Count:   u64 little-endian (8 bytes)
//! RowIds:  [u64 little-endian; count] (sorted)
//! ```

use bundlebase_common::{BundlebaseError, RowId};
use bytes::Bytes;
use std::collections::HashSet;

const MAGIC: &[u8; 8] = b"BBRID001";
const HEADER_SIZE: usize = 8 + 8; // magic + count

/// Serialize a set of RowIds into the tombstone binary format.
///
/// Returns the serialized bytes. The caller is responsible for writing
/// to content-addressed storage (e.g., via `write_stream()`).
pub fn serialize_rowids(row_ids: &HashSet<RowId>) -> Bytes {
    let mut sorted: Vec<u64> = row_ids.iter().map(|id| id.as_u64()).collect();
    sorted.sort();

    let mut buffer = Vec::with_capacity(HEADER_SIZE + sorted.len() * 8);

    // Magic
    buffer.extend_from_slice(MAGIC);

    // Count
    buffer.extend_from_slice(&(sorted.len() as u64).to_le_bytes());

    // Sorted RowIds
    for id in &sorted {
        buffer.extend_from_slice(&id.to_le_bytes());
    }

    Bytes::from(buffer)
}

/// Deserialize a tombstone file from bytes into a set of RowIds.
pub fn deserialize_rowids(bytes: &[u8]) -> Result<HashSet<RowId>, BundlebaseError> {
    if bytes.len() < HEADER_SIZE {
        return Err("Invalid tombstone file: too short".into());
    }

    // Verify magic
    if &bytes[0..8] != MAGIC {
        return Err("Invalid tombstone file: bad magic bytes".into());
    }

    // Read count
    let count = u64::from_le_bytes(
        bytes[8..16]
            .try_into()
            .map_err(|_| BundlebaseError::from("Invalid tombstone file: bad count"))?,
    ) as usize;

    // Verify we have enough bytes
    let expected_size = HEADER_SIZE + count * 8;
    if bytes.len() < expected_size {
        return Err(format!(
            "Invalid tombstone file: expected {} bytes, got {}",
            expected_size,
            bytes.len()
        )
        .into());
    }

    // Read RowIds
    let mut row_ids = HashSet::with_capacity(count);
    for i in 0..count {
        let offset = HEADER_SIZE + i * 8;
        let id = u64::from_le_bytes(
            bytes[offset..offset + 8]
                .try_into()
                .map_err(|_| BundlebaseError::from("Invalid tombstone file: bad RowId"))?,
        );
        row_ids.insert(RowId::from(id));
    }

    Ok(row_ids)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use bundlebase_common::ObjectIdAlias;

    #[test]
    fn test_roundtrip_empty() {
        let row_ids = HashSet::new();
        let bytes = serialize_rowids(&row_ids);

        let result = deserialize_rowids(&bytes).expect("Failed to deserialize");
        assert!(result.is_empty());
    }

    #[test]
    fn test_roundtrip_with_data() {
        let block_ref = ObjectIdAlias::from(1u16);
        let mut row_ids = HashSet::new();
        row_ids.insert(RowId::new(block_ref, 0));
        row_ids.insert(RowId::new(block_ref, 1));
        row_ids.insert(RowId::new(block_ref, 2));

        let bytes = serialize_rowids(&row_ids);

        let result = deserialize_rowids(&bytes).expect("Failed to deserialize");
        assert_eq!(result.len(), 3);
        assert_eq!(result, row_ids);
    }

    #[test]
    fn test_invalid_magic() {
        let bytes = b"WRONGMAG\x00\x00\x00\x00\x00\x00\x00\x00";
        let result = deserialize_rowids(bytes);
        assert!(result.is_err());
    }

    #[test]
    fn test_too_short() {
        let result = deserialize_rowids(&[0u8; 5]);
        assert!(result.is_err());
    }

    #[test]
    fn test_sorted_output() {
        let block_ref = ObjectIdAlias::from(1u16);
        let mut row_ids = HashSet::new();
        row_ids.insert(RowId::new(block_ref, 2));
        row_ids.insert(RowId::new(block_ref, 0));
        row_ids.insert(RowId::new(block_ref, 1));

        let bytes = serialize_rowids(&row_ids);

        // Read raw RowIds from the bytes to verify sorting
        let id1 = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
        let id2 = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
        let id3 = u64::from_le_bytes(bytes[32..40].try_into().unwrap());
        assert!(id1 < id2);
        assert!(id2 < id3);
    }
}
