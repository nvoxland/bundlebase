//! DELETE selections and their Puffin persistence adapter.
//!
//! Row ordinals belong to a stable `(BlockId, version)`. Puffin supplies the
//! container and standard Roaring deletion vectors; Bundlebase properties
//! preserve that versioned identity (ADR-011).

use bundlebase_common::{BlockId, BundlebaseError, VersionedBlockId};
use bundlebase_data::puffin::{self, DeletionVector, RoaringTreemap};
use bytes::Bytes;
use std::collections::{BTreeMap, BTreeSet, HashMap};

const BLOCK_ID: &str = "bundlebase.block-id";
const BLOCK_VERSION: &str = "bundlebase.block-version";

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
        self.blocks
            .get(block)
            .is_some_and(|rows| rows.contains(&row))
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

/// Encode one deletion vector per selected block version.
pub fn serialize(
    selection: &RowSelection,
    locations: &HashMap<BlockId, String>,
) -> Result<Bytes, BundlebaseError> {
    let vectors = selection
        .blocks()
        .map(|(key, rows)| {
            let location = locations.get(&key.block).ok_or_else(|| {
                BundlebaseError::from(format!(
                    "Missing attachment location for deleted block {key}"
                ))
            })?;
            Ok(DeletionVector {
                properties: BTreeMap::from([
                    (BLOCK_ID.to_string(), key.block.to_string()),
                    (BLOCK_VERSION.to_string(), key.version.clone()),
                    ("referenced-data-file".to_string(), location.clone()),
                ]),
                positions: rows.iter().map(|&row| u64::from(row)).collect(),
            })
        })
        .collect::<Result<Vec<_>, BundlebaseError>>()?;
    Ok(Bytes::from(puffin::serialize(&vectors)?))
}

/// Decoded positions stay compressed until the complete operation is validated.
pub struct BlockDeletion {
    /// The exact attached block version the ordinals describe.
    pub block: VersionedBlockId,
    /// Deleted ordinals, validated to fit Bundlebase row numbers.
    pub rows: RoaringTreemap,
}

/// Validate all blobs, including Bundlebase identity and ordinal range, before
/// returning any deletion to the caller. No base block is mutated here.
pub fn deserialize(bytes: &[u8]) -> Result<Vec<BlockDeletion>, BundlebaseError> {
    if bytes.starts_with(b"BBTOMB") {
        return Err(
            "Unsupported legacy DELETE tombstone format; Puffin deletion vectors are required"
                .into(),
        );
    }
    puffin::deserialize(bytes)?
        .into_iter()
        .map(|vector| {
            let id = vector
                .properties
                .get(BLOCK_ID)
                .ok_or("Puffin deletion vector missing bundlebase.block-id")?;
            let version = vector
                .properties
                .get(BLOCK_VERSION)
                .filter(|v| !v.is_empty())
                .ok_or("Puffin deletion vector missing bundlebase.block-version")?;
            let block = BlockId::try_from(id.as_str())?;
            if vector
                .positions
                .max()
                .is_some_and(|row| row > u64::from(u32::MAX))
            {
                return Err(
                    "Puffin deletion vector row position exceeds Bundlebase's u32 range".into(),
                );
            }
            Ok(BlockDeletion {
                block: VersionedBlockId::new(block, version.clone()),
                rows: vector.positions,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_tracks_versions_and_removes_rows() {
        let block = VersionedBlockId::new(BlockId::generate(), "v1".to_string());
        let other = VersionedBlockId::new(block.block, "v2".to_string());
        let mut selection = RowSelection::default();
        assert!(selection.insert(&block, 7));
        assert!(!selection.insert(&block, 7));
        assert!(!selection.contains(&other, 7));
        assert!(selection.remove(&block, 7));
        assert!(selection.is_empty());
    }

    #[test]
    fn rejects_legacy_format() {
        assert!(deserialize(b"BBTOMB02")
            .err()
            .expect("legacy error")
            .to_string()
            .contains("Unsupported legacy"));
    }

    #[test]
    fn validates_identity_and_u32_range_before_expansion() -> Result<(), BundlebaseError> {
        let mut vector = DeletionVector {
            properties: BTreeMap::from([
                (
                    "referenced-data-file".to_string(),
                    "data.parquet".to_string(),
                ),
                (BLOCK_ID.to_string(), BlockId::generate().to_string()),
            ]),
            positions: [1].into_iter().collect(),
        };
        assert!(deserialize(&puffin::serialize(&[vector.clone()])?).is_err());
        vector
            .properties
            .insert(BLOCK_VERSION.to_string(), "v1".to_string());
        vector.positions.insert(u64::from(u32::MAX) + 1);
        assert!(deserialize(&puffin::serialize(&[vector])?).is_err());
        Ok(())
    }
}
