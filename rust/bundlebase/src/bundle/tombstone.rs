//! Tombstone file I/O for DELETE support.
//!
//! A tombstone records deleted rows keyed by the block they belong to — the
//! block's stable `(BlockId, version)` — and the row number within it. Never
//! by the block's position in the base pack: positions shift when a block is
//! DETACHed or REPLACEd, and when another writer's commit is replayed first.
//! Rows recorded against a block version that is no longer attached simply
//! don't apply (see ADR-009).
//!
//! Files are content-addressed.
//!
//! # File Format
//!
//! ```text
//! Magic:        b"BBTOMB02" (8 bytes)
//! Block count:  u32 little-endian
//! Per block (sorted by key):
//!   Key length: u16 little-endian
//!   Key:        UTF-8 "<block_id>@<version>"
//!   Row count:  u64 little-endian
//!   Rows:       [u32 little-endian; row count] (sorted ascending)
//! ```

use bundlebase_common::{BlockId, BundlebaseError, VersionedBlockId};
use bytes::Bytes;
use std::collections::{BTreeSet, HashMap};

const MAGIC: &[u8; 8] = b"BBTOMB02";

/// A set of rows keyed by block `(BlockId, version)` and row number.
///
/// Used for DELETE selections, the builder's pending tombstones and the
/// persisted tombstone file contents.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RowSelection {
    blocks: HashMap<VersionedBlockId, BTreeSet<u32>>,
}

impl RowSelection {
    /// Add a row; returns true if it was not already present.
    pub fn insert(&mut self, block: &VersionedBlockId, row: u32) -> bool {
        match self.blocks.get_mut(block) {
            Some(rows) => rows.insert(row),
            None => {
                self.blocks.insert(block.clone(), BTreeSet::from([row]));
                true
            }
        }
    }

    /// Remove a row; returns true if it was present.
    pub fn remove(&mut self, block: &VersionedBlockId, row: u32) -> bool {
        let Some(rows) = self.blocks.get_mut(block) else {
            return false;
        };
        let removed = rows.remove(&row);
        if rows.is_empty() {
            self.blocks.remove(block);
        }
        removed
    }

    pub fn contains(&self, block: &VersionedBlockId, row: u32) -> bool {
        self.blocks.get(block).is_some_and(|rows| rows.contains(&row))
    }

    /// The selected rows of one block version, if any.
    pub fn rows_for(&self, block: &VersionedBlockId) -> Option<&BTreeSet<u32>> {
        self.blocks.get(block)
    }

    /// Iterate `(block, rows)` pairs.
    pub fn blocks(&self) -> impl Iterator<Item = (&VersionedBlockId, &BTreeSet<u32>)> {
        self.blocks.iter()
    }

    /// Keep only the blocks for which `keep` returns true.
    pub fn retain_blocks(&mut self, mut keep: impl FnMut(&VersionedBlockId) -> bool) {
        self.blocks.retain(|block, _| keep(block));
    }

    /// Total number of selected rows.
    pub fn len(&self) -> usize {
        self.blocks.values().map(|rows| rows.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

/// Parse a `"<block_id>@<version>"` key (the `VersionedBlockId` display form).
pub fn parse_block_key(key: &str) -> Result<VersionedBlockId, BundlebaseError> {
    let (block, version) = key
        .split_once('@')
        .ok_or_else(|| BundlebaseError::from(format!("Invalid block key '{}'", key)))?;
    let block = BlockId::try_from(block)
        .map_err(|e| BundlebaseError::from(format!("Invalid block id in '{}': {}", key, e)))?;
    Ok(VersionedBlockId::new(block, version.to_string()))
}

/// Serialize a row selection into the tombstone binary format.
pub fn serialize(selection: &RowSelection) -> Result<Bytes, BundlebaseError> {
    let mut blocks: Vec<(String, &BTreeSet<u32>)> = selection
        .blocks()
        .map(|(block, rows)| (block.to_string(), rows))
        .collect();
    blocks.sort_by(|a, b| a.0.cmp(&b.0));

    let block_count = u32::try_from(blocks.len())
        .map_err(|_| BundlebaseError::from("Too many blocks in tombstone"))?;
    let mut buffer = Vec::with_capacity(12 + selection.len() * 4);
    buffer.extend_from_slice(MAGIC);
    buffer.extend_from_slice(&block_count.to_le_bytes());
    for (key, rows) in blocks {
        let key_len = u16::try_from(key.len())
            .map_err(|_| BundlebaseError::from(format!("Block key too long: '{}'", key)))?;
        buffer.extend_from_slice(&key_len.to_le_bytes());
        buffer.extend_from_slice(key.as_bytes());
        buffer.extend_from_slice(&(rows.len() as u64).to_le_bytes());
        for row in rows {
            buffer.extend_from_slice(&row.to_le_bytes());
        }
    }
    Ok(Bytes::from(buffer))
}

/// Bounds-checked little-endian reader over a tombstone buffer.
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], BundlebaseError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| BundlebaseError::from("Invalid tombstone file: truncated"))?;
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], BundlebaseError> {
        self.take(N)?
            .try_into()
            .map_err(|_| BundlebaseError::from("Invalid tombstone file: truncated"))
    }
}

/// Deserialize a tombstone file.
pub fn deserialize(bytes: &[u8]) -> Result<RowSelection, BundlebaseError> {
    let mut reader = Reader { bytes, pos: 0 };
    if reader.take(MAGIC.len())? != MAGIC {
        return Err("Invalid tombstone file: bad magic bytes".into());
    }
    let block_count = u32::from_le_bytes(reader.array()?);
    let mut selection = RowSelection::default();
    for _ in 0..block_count {
        let key_len = u16::from_le_bytes(reader.array()?) as usize;
        let key = std::str::from_utf8(reader.take(key_len)?)
            .map_err(|_| BundlebaseError::from("Invalid tombstone file: bad block key"))?;
        let block = parse_block_key(key)?;
        let row_count = u64::from_le_bytes(reader.array()?);
        for _ in 0..row_count {
            selection.insert(&block, u32::from_le_bytes(reader.array()?));
        }
    }
    Ok(selection)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(version: &str) -> VersionedBlockId {
        VersionedBlockId::new(BlockId::generate(), version.to_string())
    }

    #[test]
    fn test_roundtrip_empty() {
        let bytes = serialize(&RowSelection::default()).expect("serialize");
        assert!(deserialize(&bytes).expect("deserialize").is_empty());
    }

    #[test]
    fn test_roundtrip_with_data() {
        let a = block("v1");
        let b = block("etag-2");
        let mut sel = RowSelection::default();
        for row in [2, 0, 1] {
            sel.insert(&a, row);
        }
        sel.insert(&b, 42);

        let bytes = serialize(&sel).expect("serialize");
        let result = deserialize(&bytes).expect("deserialize");
        assert_eq!(result, sel);
        assert_eq!(result.len(), 4);
        assert!(result.contains(&b, 42));
        assert!(!result.contains(&block("v1"), 0), "keys are per block id");
    }

    #[test]
    fn test_same_block_different_version_is_distinct() {
        let a = block("v1");
        let a2 = VersionedBlockId::new(a.block, "v2".to_string());
        let mut sel = RowSelection::default();
        sel.insert(&a, 7);
        assert!(!sel.contains(&a2, 7));
    }

    #[test]
    fn test_invalid_magic() {
        assert!(deserialize(b"WRONGMAG\x00\x00\x00\x00").is_err());
    }

    #[test]
    fn test_truncated() {
        let mut sel = RowSelection::default();
        sel.insert(&block("v1"), 3);
        let bytes = serialize(&sel).expect("serialize");
        assert!(deserialize(&bytes[..bytes.len() - 1]).is_err());
        assert!(deserialize(&[0u8; 5]).is_err());
    }

    #[test]
    fn test_remove() {
        let a = block("v1");
        let mut sel = RowSelection::default();
        assert!(sel.insert(&a, 1));
        assert!(!sel.insert(&a, 1));
        assert!(sel.remove(&a, 1));
        assert!(sel.is_empty());
    }
}
