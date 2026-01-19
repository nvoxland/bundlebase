use crate::data::RowId;
use crate::index::{Index, IndexType};
use crate::BundlebaseError;
use arrow::datatypes::DataType;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use datafusion::scalar::ScalarValue;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::io::Cursor;

const MAGIC: &[u8; 8] = b"COLIDX\0\0";
const VERSION: u8 = 1;
const TARGET_BLOCK_SIZE: usize = 64 * 1024; // 64KB blocks

/// Indexed value supporting common Arrow data types
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IndexedValue {
    Int64(i64),
    Float64(OrderedFloat),
    Utf8(String),
    Boolean(bool),
    Timestamp(i64), // Nanoseconds since epoch
    Null,
}

// Wrapper for f64 that implements Eq and Hash
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct OrderedFloat(pub f64);

impl Eq for OrderedFloat {}

impl std::hash::Hash for OrderedFloat {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

impl From<f64> for OrderedFloat {
    fn from(f: f64) -> Self {
        OrderedFloat(f)
    }
}

impl From<OrderedFloat> for f64 {
    fn from(of: OrderedFloat) -> f64 {
        of.0
    }
}

impl IndexedValue {
    /// Create from ScalarValue
    pub fn from_scalar(scalar: &ScalarValue) -> Result<Self, BundlebaseError> {
        match scalar {
            ScalarValue::Int64(Some(v)) => Ok(IndexedValue::Int64(*v)),
            ScalarValue::Int32(Some(v)) => Ok(IndexedValue::Int64(*v as i64)),
            ScalarValue::Int16(Some(v)) => Ok(IndexedValue::Int64(*v as i64)),
            ScalarValue::Int8(Some(v)) => Ok(IndexedValue::Int64(*v as i64)),
            ScalarValue::UInt64(Some(v)) => Ok(IndexedValue::Int64(*v as i64)),
            ScalarValue::UInt32(Some(v)) => Ok(IndexedValue::Int64(*v as i64)),
            ScalarValue::UInt16(Some(v)) => Ok(IndexedValue::Int64(*v as i64)),
            ScalarValue::UInt8(Some(v)) => Ok(IndexedValue::Int64(*v as i64)),
            ScalarValue::Float64(Some(v)) => Ok(IndexedValue::Float64(OrderedFloat(*v))),
            ScalarValue::Float32(Some(v)) => Ok(IndexedValue::Float64(OrderedFloat(*v as f64))),
            ScalarValue::Utf8(Some(v)) | ScalarValue::LargeUtf8(Some(v)) => {
                Ok(IndexedValue::Utf8(v.clone()))
            }
            ScalarValue::Boolean(Some(v)) => Ok(IndexedValue::Boolean(*v)),
            ScalarValue::TimestampNanosecond(Some(v), _)
            | ScalarValue::TimestampMicrosecond(Some(v), _)
            | ScalarValue::TimestampMillisecond(Some(v), _)
            | ScalarValue::TimestampSecond(Some(v), _) => Ok(IndexedValue::Timestamp(*v)),
            ScalarValue::Null => Ok(IndexedValue::Null),
            _ if scalar.is_null() => Ok(IndexedValue::Null),
            _ => Err(format!("Unsupported scalar type for indexing: {:?}", scalar).into()),
        }
    }

    /// Convert to ScalarValue
    pub fn to_scalar(&self, data_type: &DataType) -> ScalarValue {
        match (self, data_type) {
            (IndexedValue::Int64(v), DataType::Int64) => ScalarValue::Int64(Some(*v)),
            (IndexedValue::Int64(v), DataType::Int32) => ScalarValue::Int32(Some(*v as i32)),
            (IndexedValue::Float64(v), DataType::Float64) => ScalarValue::Float64(Some(v.0)),
            (IndexedValue::Float64(v), DataType::Float32) => ScalarValue::Float32(Some(v.0 as f32)),
            (IndexedValue::Utf8(v), DataType::Utf8) => ScalarValue::Utf8(Some(v.clone())),
            (IndexedValue::Boolean(v), DataType::Boolean) => ScalarValue::Boolean(Some(*v)),
            (IndexedValue::Timestamp(v), DataType::Timestamp(_, _)) => {
                ScalarValue::TimestampNanosecond(Some(*v), None)
            }
            (IndexedValue::Null, _) => ScalarValue::Null,
            _ => ScalarValue::Null,
        }
    }

    /// Serialize to bytes
    pub fn serialize(&self) -> Bytes {
        let mut buf = BytesMut::new();
        match self {
            IndexedValue::Int64(v) => {
                buf.put_u8(1); // Type tag
                buf.put_i64(*v);
            }
            IndexedValue::Float64(v) => {
                buf.put_u8(2);
                buf.put_f64(v.0);
            }
            IndexedValue::Utf8(v) => {
                buf.put_u8(3);
                buf.put_u32(v.len() as u32);
                buf.put_slice(v.as_bytes());
            }
            IndexedValue::Boolean(v) => {
                buf.put_u8(4);
                buf.put_u8(if *v { 1 } else { 0 });
            }
            IndexedValue::Timestamp(v) => {
                buf.put_u8(5);
                buf.put_i64(*v);
            }
            IndexedValue::Null => {
                buf.put_u8(0);
            }
        }
        buf.freeze()
    }

    /// Deserialize from bytes
    pub fn deserialize(cursor: &mut Cursor<&[u8]>) -> Result<Self, BundlebaseError> {
        if !cursor.has_remaining() {
            return Err("Unexpected end of index data".into());
        }

        let type_tag = cursor.get_u8();
        match type_tag {
            0 => Ok(IndexedValue::Null),
            1 => Ok(IndexedValue::Int64(cursor.get_i64())),
            2 => Ok(IndexedValue::Float64(OrderedFloat(cursor.get_f64()))),
            3 => {
                let len = cursor.get_u32() as usize;
                let mut bytes = vec![0u8; len];
                cursor.copy_to_slice(&mut bytes);
                let s = String::from_utf8(bytes)
                    .map_err(|e| format!("Invalid UTF-8 in index: {}", e))?;
                Ok(IndexedValue::Utf8(s))
            }
            4 => Ok(IndexedValue::Boolean(cursor.get_u8() != 0)),
            5 => Ok(IndexedValue::Timestamp(cursor.get_i64())),
            _ => Err(format!("Unknown index value type tag: {}", type_tag).into()),
        }
    }

    /// Estimate size in bytes when serialized
    pub fn size_bytes(&self) -> usize {
        match self {
            IndexedValue::Null => 1,
            IndexedValue::Int64(_) => 9,
            IndexedValue::Float64(_) => 9,
            IndexedValue::Utf8(s) => 5 + s.len(),
            IndexedValue::Boolean(_) => 2,
            IndexedValue::Timestamp(_) => 9,
        }
    }

    /// Get the minimum possible value for this type
    pub fn min_for_type(&self) -> IndexedValue {
        match self {
            IndexedValue::Int64(_) => IndexedValue::Int64(i64::MIN),
            IndexedValue::Float64(_) => IndexedValue::Float64(OrderedFloat(f64::NEG_INFINITY)),
            IndexedValue::Utf8(_) => IndexedValue::Utf8(String::new()),
            IndexedValue::Boolean(_) => IndexedValue::Boolean(false),
            IndexedValue::Timestamp(_) => IndexedValue::Timestamp(i64::MIN),
            IndexedValue::Null => IndexedValue::Null,
        }
    }

    /// Get the maximum possible value for this type
    pub fn max_for_type(&self) -> IndexedValue {
        match self {
            IndexedValue::Int64(_) => IndexedValue::Int64(i64::MAX),
            IndexedValue::Float64(_) => IndexedValue::Float64(OrderedFloat(f64::INFINITY)),
            IndexedValue::Utf8(_) => IndexedValue::Utf8("\u{10FFFF}".repeat(1000)), // Max Unicode character repeated
            IndexedValue::Boolean(_) => IndexedValue::Boolean(true),
            IndexedValue::Timestamp(_) => IndexedValue::Timestamp(i64::MAX),
            IndexedValue::Null => IndexedValue::Null,
        }
    }
}

impl Ord for IndexedValue {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (IndexedValue::Null, IndexedValue::Null) => Ordering::Equal,
            (IndexedValue::Null, _) => Ordering::Less, // NULLs sort first
            (_, IndexedValue::Null) => Ordering::Greater,
            (IndexedValue::Int64(a), IndexedValue::Int64(b)) => a.cmp(b),
            (IndexedValue::Float64(a), IndexedValue::Float64(b)) => {
                // Handle NaN explicitly to maintain proper ordering semantics
                if a.0.is_nan() && b.0.is_nan() {
                    Ordering::Equal
                } else if a.0.is_nan() {
                    Ordering::Greater // NaN sorts last
                } else if b.0.is_nan() {
                    Ordering::Less
                } else {
                    a.partial_cmp(b).unwrap_or(Ordering::Equal)
                }
            }
            (IndexedValue::Utf8(a), IndexedValue::Utf8(b)) => a.cmp(b),
            (IndexedValue::Boolean(a), IndexedValue::Boolean(b)) => a.cmp(b),
            (IndexedValue::Timestamp(a), IndexedValue::Timestamp(b)) => a.cmp(b),
            _ => Ordering::Equal, // Mixed types (shouldn't happen)
        }
    }
}

impl PartialOrd for IndexedValue {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Entry in a sparse index block
#[derive(Debug, Clone)]
struct BlockEntryValue {
    value: IndexedValue,
    row_ids: Vec<RowId>,
}

/// A block of index entries
#[derive(Debug, Clone)]
struct IndexBlock {
    entries: Vec<BlockEntryValue>,
}

impl IndexBlock {
    fn serialize(&self) -> Bytes {
        let mut buf = BytesMut::new();

        // Entry count
        buf.put_u32(self.entries.len() as u32);

        // Entries
        for entry in &self.entries {
            // Value
            let value_bytes = entry.value.serialize();
            buf.put(value_bytes);

            // RowId count
            buf.put_u16(entry.row_ids.len() as u16);

            // RowIds
            for row_id in &entry.row_ids {
                buf.put_u64(row_id.as_u64());
            }
        }

        buf.freeze()
    }

    fn deserialize(data: &[u8]) -> Result<Self, BundlebaseError> {
        let mut cursor = Cursor::new(data);

        let entry_count = cursor.get_u32() as usize;
        let mut entries = Vec::with_capacity(entry_count);

        for _ in 0..entry_count {
            let value = IndexedValue::deserialize(&mut cursor)?;
            let row_id_count = cursor.get_u16() as usize;

            let mut row_ids = Vec::with_capacity(row_id_count);
            for _ in 0..row_id_count {
                row_ids.push(RowId::from(cursor.get_u64()));
            }

            entries.push(BlockEntryValue { value, row_ids });
        }

        Ok(IndexBlock { entries })
    }

    /// Binary search for a value within the block
    fn find_exact(&self, target: &IndexedValue) -> Option<&Vec<RowId>> {
        self.entries
            .binary_search_by(|entry| entry.value.cmp(target))
            .ok()
            .map(|idx| &self.entries[idx].row_ids)
    }
}

/// Directory entry for a block
#[derive(Debug, Clone)]
struct BlockEntry {
    min_value: IndexedValue,
    max_value: IndexedValue,
    file_offset: u64,
}

/// Block directory for binary search
#[derive(Debug, Clone)]
struct BlockDirectory {
    entries: Vec<BlockEntry>,
}

impl BlockDirectory {
    fn serialize(&self) -> Bytes {
        let mut buf = BytesMut::new();

        for entry in &self.entries {
            // Min value (fixed 8 bytes for directory)
            let min_bytes = entry.min_value.serialize();
            let min_len = min_bytes.len();
            if min_len > 8 {
                // For strings, hash to 8 bytes
                buf.put_u64(Self::hash_value(&entry.min_value));
            } else {
                buf.put(min_bytes);
                // Pad to 8 bytes
                for _ in min_len..8 {
                    buf.put_u8(0);
                }
            }

            // Max value (fixed 8 bytes)
            let max_bytes = entry.max_value.serialize();
            let max_len = max_bytes.len();
            if max_len > 8 {
                buf.put_u64(Self::hash_value(&entry.max_value));
            } else {
                buf.put(max_bytes);
                for _ in max_len..8 {
                    buf.put_u8(0);
                }
            }

            // File offset
            buf.put_u64(entry.file_offset);
        }

        buf.freeze()
    }

    fn deserialize(data: &[u8], block_count: usize) -> Result<Self, BundlebaseError> {
        let mut cursor = Cursor::new(data);
        let mut entries = Vec::with_capacity(block_count);

        for _ in 0..block_count {
            // Skip min/max hashes (we'll load actual values from blocks)
            cursor.advance(16); // 8 bytes min + 8 bytes max
            let file_offset = cursor.get_u64();

            // Placeholder values (will be updated when blocks are loaded)
            entries.push(BlockEntry {
                min_value: IndexedValue::Null,
                max_value: IndexedValue::Null,
                file_offset,
            });
        }

        Ok(BlockDirectory { entries })
    }

    fn hash_value(value: &IndexedValue) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        match value {
            IndexedValue::Utf8(s) => s.hash(&mut hasher),
            IndexedValue::Int64(v) => v.hash(&mut hasher),
            IndexedValue::Boolean(v) => v.hash(&mut hasher),
            IndexedValue::Timestamp(v) => v.hash(&mut hasher),
            IndexedValue::Float64(v) => v.hash(&mut hasher),
            IndexedValue::Null => 0u64.hash(&mut hasher),
        }
        hasher.finish()
    }
}

/// Column index structure
#[derive(Debug, Clone)]
pub struct ColumnIndex {
    column_name: String,
    data_type: DataType,
    blocks: Vec<IndexBlock>,
    directory: BlockDirectory,
    total_entries: u64, // Number of distinct values
    total_rows: u64,    // Total number of rows indexed
}

impl ColumnIndex {
    /// Build index from value -> rowids mapping
    pub fn build(
        column_name: &str,
        data_type: &DataType,
        mut value_to_rowids: HashMap<IndexedValue, Vec<RowId>>,
    ) -> Result<Self, BundlebaseError> {
        // Convert to sorted vector
        let mut sorted_entries: Vec<(IndexedValue, Vec<RowId>)> = value_to_rowids.drain().collect();
        sorted_entries.sort_by(|a, b| a.0.cmp(&b.0));

        let total_entries = sorted_entries.len() as u64;
        let total_rows = sorted_entries
            .iter()
            .map(|(_, row_ids)| row_ids.len() as u64)
            .sum();

        // Partition into blocks
        let blocks = Self::partition_into_blocks(sorted_entries)?;

        // Build directory
        let mut directory_entries = Vec::with_capacity(blocks.len());
        let mut current_offset = 0u64; // Will be updated when writing to file

        for block in &blocks {
            let (min_value, max_value) = if let (Some(first), Some(last)) =
                (block.entries.first(), block.entries.last())
            {
                (first.value.clone(), last.value.clone())
            } else {
                continue; // Empty block, skip
            };

            directory_entries.push(BlockEntry {
                min_value,
                max_value,
                file_offset: current_offset,
            });

            // Estimate block size for offset calculation
            current_offset += block.serialize().len() as u64;
        }

        Ok(ColumnIndex {
            column_name: column_name.to_string(),
            data_type: data_type.clone(),
            blocks,
            directory: BlockDirectory {
                entries: directory_entries,
            },
            total_entries,
            total_rows,
        })
    }

    /// Build index from iterator of sorted (value, rowid) entries.
    ///
    /// This method builds the index incrementally without holding all values in memory.
    /// The input iterator MUST yield entries in sorted order by (value, rowid).
    ///
    /// # Arguments
    /// * `column_name` - Name of the column being indexed
    /// * `data_type` - Arrow data type of the column
    /// * `sorted_entries` - Iterator yielding (IndexedValue, RowId) pairs in sorted order
    ///
    /// # Returns
    /// A new ColumnIndex built from the sorted entries.
    pub fn build_streaming<I>(
        column_name: &str,
        data_type: &DataType,
        sorted_entries: I,
    ) -> Result<Self, BundlebaseError>
    where
        I: Iterator<Item = Result<(IndexedValue, RowId), BundlebaseError>>,
    {
        // Track current value for grouping row_ids
        let mut current_value: Option<IndexedValue> = None;
        let mut current_row_ids: Vec<RowId> = Vec::new();

        // Block building state
        let mut blocks: Vec<IndexBlock> = Vec::new();
        let mut current_block_entries: Vec<BlockEntryValue> = Vec::new();
        let mut current_block_size: usize = 0;

        // Statistics
        let mut total_entries: u64 = 0;
        let mut total_rows: u64 = 0;

        // Helper to flush current value to block
        let flush_value_to_block = |value: IndexedValue,
                                         row_ids: Vec<RowId>,
                                         block_entries: &mut Vec<BlockEntryValue>,
                                         block_size: &mut usize,
                                         blocks: &mut Vec<IndexBlock>,
                                         total_entries: &mut u64,
                                         total_rows: &mut u64| {
            let entry_size = value.size_bytes() + 2 + (row_ids.len() * 8);

            // Check if we need to start a new block
            if *block_size + entry_size > TARGET_BLOCK_SIZE && !block_entries.is_empty() {
                blocks.push(IndexBlock {
                    entries: std::mem::take(block_entries),
                });
                *block_size = 0;
            }

            *total_entries += 1;
            *total_rows += row_ids.len() as u64;
            *block_size += entry_size;
            block_entries.push(BlockEntryValue { value, row_ids });
        };

        // Process all entries
        for entry_result in sorted_entries {
            let (value, row_id) = entry_result?;

            match &current_value {
                Some(cv) if cv == &value => {
                    // Same value, add to current row_ids
                    current_row_ids.push(row_id);
                }
                _ => {
                    // Different value - flush previous if exists
                    if let Some(prev_value) = current_value.take() {
                        let prev_row_ids = std::mem::take(&mut current_row_ids);
                        flush_value_to_block(
                            prev_value,
                            prev_row_ids,
                            &mut current_block_entries,
                            &mut current_block_size,
                            &mut blocks,
                            &mut total_entries,
                            &mut total_rows,
                        );
                    }
                    // Start new value
                    current_value = Some(value);
                    current_row_ids.push(row_id);
                }
            }
        }

        // Flush final value
        if let Some(final_value) = current_value {
            flush_value_to_block(
                final_value,
                current_row_ids,
                &mut current_block_entries,
                &mut current_block_size,
                &mut blocks,
                &mut total_entries,
                &mut total_rows,
            );
        }

        // Flush final block
        if !current_block_entries.is_empty() {
            blocks.push(IndexBlock {
                entries: current_block_entries,
            });
        }

        // Build directory
        let mut directory_entries = Vec::with_capacity(blocks.len());
        let mut current_offset = 0u64;

        for block in &blocks {
            let (min_value, max_value) = if let (Some(first), Some(last)) =
                (block.entries.first(), block.entries.last())
            {
                (first.value.clone(), last.value.clone())
            } else {
                continue; // Empty block, skip
            };

            directory_entries.push(BlockEntry {
                min_value,
                max_value,
                file_offset: current_offset,
            });

            current_offset += block.serialize().len() as u64;
        }

        Ok(ColumnIndex {
            column_name: column_name.to_string(),
            data_type: data_type.clone(),
            blocks,
            directory: BlockDirectory {
                entries: directory_entries,
            },
            total_entries,
            total_rows,
        })
    }

    fn partition_into_blocks(
        sorted_entries: Vec<(IndexedValue, Vec<RowId>)>,
    ) -> Result<Vec<IndexBlock>, BundlebaseError> {
        // Estimate number of blocks based on total entries and target block size
        // Assume average entry is ~20 bytes (type tag + value + rowid count + avg 1 rowid)
        const ESTIMATED_AVG_ENTRY_SIZE: usize = 20;
        let estimated_entries_per_block = TARGET_BLOCK_SIZE / ESTIMATED_AVG_ENTRY_SIZE;
        let estimated_block_count = (sorted_entries.len() / estimated_entries_per_block).max(1);

        let mut blocks = Vec::with_capacity(estimated_block_count);
        let mut current_block = Vec::with_capacity(estimated_entries_per_block);
        let mut current_size = 0;

        for (value, row_ids) in sorted_entries {
            let entry_size = value.size_bytes() + 2 + (row_ids.len() * 8);

            if current_size + entry_size > TARGET_BLOCK_SIZE && !current_block.is_empty() {
                blocks.push(IndexBlock {
                    entries: current_block,
                });
                current_block = Vec::with_capacity(estimated_entries_per_block);
                current_size = 0;
            }

            current_block.push(BlockEntryValue { value, row_ids });
            current_size += entry_size;
        }

        if !current_block.is_empty() {
            blocks.push(IndexBlock {
                entries: current_block,
            });
        }

        Ok(blocks)
    }

    /// Serialize index to bytes
    pub fn serialize(&self) -> Result<Bytes, BundlebaseError> {
        let mut buf = BytesMut::new();

        // Header (32 bytes)
        buf.put_slice(MAGIC); // 8 bytes
        buf.put_u8(VERSION); // 1 byte
        buf.put_u8(self.data_type_to_u8()); // 1 byte
        buf.put_u32(self.blocks.len() as u32); // 4 bytes
        buf.put_u64(self.total_entries); // 8 bytes
        buf.put_u64(self.total_rows); // 8 bytes
        buf.put_u16(0); // 2 bytes reserved

        // Update directory offsets
        let header_size = 32;
        let directory_size = self.blocks.len() * 24;
        let mut updated_directory = self.directory.clone();
        let mut current_offset = (header_size + directory_size) as u64;

        for (i, block) in self.blocks.iter().enumerate() {
            updated_directory.entries[i].file_offset = current_offset;
            current_offset += block.serialize().len() as u64;
        }

        // Directory
        buf.put(updated_directory.serialize());

        // Blocks
        for block in &self.blocks {
            buf.put(block.serialize());
        }

        Ok(buf.freeze())
    }

    /// Deserialize index from bytes
    pub fn deserialize(data: Bytes, column_name: String) -> Result<Self, BundlebaseError> {
        let mut cursor = Cursor::new(data.as_ref());

        // Verify magic
        let mut magic = [0u8; 8];
        cursor.copy_to_slice(&mut magic);
        if &magic != MAGIC {
            return Err("Invalid index file magic".into());
        }

        // Header
        let version = cursor.get_u8();
        if version != VERSION {
            return Err(format!("Unsupported index version: {}", version).into());
        }

        let data_type_tag = cursor.get_u8();
        let data_type = Self::u8_to_data_type(data_type_tag)?;
        let block_count = cursor.get_u32() as usize;
        let total_entries = cursor.get_u64();
        let total_rows = cursor.get_u64();
        cursor.advance(2); // Skip reserved

        // Directory
        let directory_size = block_count * 24;
        let directory_data = &data.as_ref()[32..32 + directory_size];
        let mut directory = BlockDirectory::deserialize(directory_data, block_count)?;

        // Blocks
        let mut blocks = Vec::with_capacity(block_count);
        for i in 0..directory.entries.len() {
            let offset = directory.entries[i].file_offset as usize;
            // Find end of block (next block offset or end of file)
            let next_offset = if i + 1 < directory.entries.len() {
                directory.entries[i + 1].file_offset as usize
            } else {
                data.len()
            };

            let block_data = &data.as_ref()[offset..next_offset];
            let block = IndexBlock::deserialize(block_data)?;

            // Update directory with actual min/max from block
            if let (Some(first), Some(last)) = (block.entries.first(), block.entries.last()) {
                directory.entries[i].min_value = first.value.clone();
                directory.entries[i].max_value = last.value.clone();
            }

            blocks.push(block);
        }

        Ok(ColumnIndex {
            column_name,
            data_type,
            blocks,
            directory,
            total_entries,
            total_rows,
        })
    }

    /// Point lookup - find all RowIds for exact value match
    pub fn lookup_exact(&self, target: &IndexedValue) -> Vec<RowId> {
        // Binary search directory for containing block
        let block_idx = self
            .directory
            .entries
            .binary_search_by(|entry| {
                if target < &entry.min_value {
                    Ordering::Greater
                } else if target > &entry.max_value {
                    Ordering::Less
                } else {
                    Ordering::Equal
                }
            })
            .ok();

        match block_idx {
            Some(idx) => self.blocks[idx]
                .find_exact(target)
                .cloned()
                .unwrap_or_default(),
            None => Vec::new(),
        }
    }

    /// Range lookup - find all RowIds where min <= value <= max
    pub fn lookup_range(&self, min: &IndexedValue, max: &IndexedValue) -> Vec<RowId> {
        let mut result = Vec::new();

        for (idx, entry) in self.directory.entries.iter().enumerate() {
            // Skip blocks entirely outside range
            if max < &entry.min_value || min > &entry.max_value {
                continue;
            }

            // Scan block for values in range
            for block_entry in &self.blocks[idx].entries {
                if &block_entry.value >= min && &block_entry.value <= max {
                    result.extend_from_slice(&block_entry.row_ids);
                }
            }
        }

        // Sort and deduplicate to ensure consistent results
        result.sort_unstable_by_key(|r| r.as_u64());
        result.dedup();
        result
    }

    /// Get metadata about this index
    pub fn column_name(&self) -> &str {
        &self.column_name
    }

    pub fn data_type(&self) -> &DataType {
        &self.data_type
    }

    pub fn cardinality(&self) -> u64 {
        self.total_entries
    }

    pub fn total_rows(&self) -> u64 {
        self.total_rows
    }

    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// Estimate selectivity for an exact match predicate
    /// Returns fraction of rows expected to match (0.0 to 1.0)
    pub fn estimate_exact_selectivity(&self, _value: &IndexedValue) -> f64 {
        if self.total_rows == 0 {
            return 0.0;
        }

        // Assume uniform distribution: 1 / distinct_values
        // This is a simple heuristic - could be improved with histograms
        1.0 / self.total_entries as f64
    }

    /// Estimate selectivity for an IN list predicate
    /// Returns fraction of rows expected to match (0.0 to 1.0)
    pub fn estimate_in_selectivity(&self, values: &[IndexedValue]) -> f64 {
        if self.total_rows == 0 || values.is_empty() {
            return 0.0;
        }

        // Estimate: (number of values) / (total distinct values)
        // Capped at 1.0 for safety
        let estimate = values.len() as f64 / self.total_entries as f64;
        estimate.min(1.0)
    }

    /// Estimate selectivity for a range predicate
    /// Returns fraction of rows expected to match (0.0 to 1.0)
    pub fn estimate_range_selectivity(&self, min: &IndexedValue, max: &IndexedValue) -> f64 {
        if self.total_rows == 0 {
            return 0.0;
        }

        // For range queries, we need to estimate based on the value distribution
        // This is a simplified estimation - could be improved with histograms

        // Count blocks that overlap with the range
        let mut overlapping_blocks = 0;
        for entry in &self.directory.entries {
            if max >= &entry.min_value && min <= &entry.max_value {
                overlapping_blocks += 1;
            }
        }

        if overlapping_blocks == 0 {
            return 0.0;
        }

        // Rough estimate: (overlapping blocks / total blocks)
        // This assumes even distribution across blocks
        let estimate = overlapping_blocks as f64 / self.directory.entries.len() as f64;
        estimate.min(1.0)
    }

    /// Estimate selectivity for any predicate type
    /// Returns fraction of rows expected to match (0.0 to 1.0)
    pub fn estimate_selectivity(&self, predicate: &crate::index::IndexPredicate) -> f64 {
        use crate::index::IndexPredicate;

        match predicate {
            IndexPredicate::Exact(value) => self.estimate_exact_selectivity(value),
            IndexPredicate::In(values) => self.estimate_in_selectivity(values),
            IndexPredicate::Range { min, max } => self.estimate_range_selectivity(min, max),
        }
    }

    fn data_type_to_u8(&self) -> u8 {
        match &self.data_type {
            DataType::Int64 | DataType::Int32 | DataType::Int16 | DataType::Int8 => 1,
            DataType::UInt64 | DataType::UInt32 | DataType::UInt16 | DataType::UInt8 => 2,
            DataType::Float64 | DataType::Float32 => 3,
            DataType::Utf8 | DataType::LargeUtf8 => 4,
            DataType::Boolean => 5,
            DataType::Timestamp(_, _) => 6,
            _ => 0,
        }
    }

    fn u8_to_data_type(tag: u8) -> Result<DataType, BundlebaseError> {
        match tag {
            1 => Ok(DataType::Int64),
            2 => Ok(DataType::UInt64),
            3 => Ok(DataType::Float64),
            4 => Ok(DataType::Utf8),
            5 => Ok(DataType::Boolean),
            6 => Ok(DataType::Timestamp(
                arrow::datatypes::TimeUnit::Nanosecond,
                None,
            )),
            _ => Err(format!("Unknown data type tag: {}", tag).into()),
        }
    }
}

impl Index for ColumnIndex {
    fn serialize(&self) -> Result<Bytes, BundlebaseError> {
        self.serialize()
    }

    fn cardinality(&self) -> u64 {
        self.total_entries
    }

    fn column_name(&self) -> &str {
        &self.column_name
    }

    fn index_type(&self) -> IndexType {
        IndexType::Column
    }

    fn total_rows(&self) -> u64 {
        self.total_rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_indexed_value_ordering() {
        assert!(IndexedValue::Null < IndexedValue::Int64(0));
        assert!(IndexedValue::Int64(1) < IndexedValue::Int64(2));
        assert!(IndexedValue::Utf8("a".to_string()) < IndexedValue::Utf8("b".to_string()));
    }

    #[test]
    fn test_indexed_value_serialize_deserialize() {
        let values = vec![
            IndexedValue::Null,
            IndexedValue::Int64(42),
            IndexedValue::Float64(OrderedFloat(3.14)),
            IndexedValue::Utf8("hello".to_string()),
            IndexedValue::Boolean(true),
            IndexedValue::Timestamp(1234567890),
        ];

        for value in values {
            let bytes = value.serialize();
            let mut cursor = Cursor::new(bytes.as_ref());
            let deserialized = IndexedValue::deserialize(&mut cursor).unwrap();
            assert_eq!(value, deserialized);
        }
    }

    #[test]
    fn test_column_index_build_and_lookup() {
        let mut value_map = HashMap::new();
        value_map.insert(IndexedValue::Int64(1), vec![RowId::from(100u64)]);
        value_map.insert(IndexedValue::Int64(2), vec![RowId::from(200u64)]);
        value_map.insert(IndexedValue::Int64(3), vec![RowId::from(300u64)]);

        let index = ColumnIndex::build("test_col", &DataType::Int64, value_map).unwrap();

        // Point lookup
        let result = index.lookup_exact(&IndexedValue::Int64(2));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_u64(), 200);

        // Range lookup
        let result = index.lookup_range(&IndexedValue::Int64(1), &IndexedValue::Int64(2));
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_column_index_serialize_deserialize() {
        let mut value_map = HashMap::new();
        value_map.insert(IndexedValue::Int64(1), vec![RowId::from(100u64)]);
        value_map.insert(IndexedValue::Int64(2), vec![RowId::from(200u64)]);

        let index = ColumnIndex::build("test_col", &DataType::Int64, value_map).unwrap();

        let bytes = index.serialize().unwrap();
        let deserialized = ColumnIndex::deserialize(bytes, "test_col".to_string()).unwrap();

        assert_eq!(deserialized.column_name(), "test_col");
        assert_eq!(deserialized.cardinality(), 2);

        let result = deserialized.lookup_exact(&IndexedValue::Int64(1));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_u64(), 100);
    }

    #[test]
    fn test_selectivity_estimation() {
        // Build index with 100 rows across 10 distinct values
        let mut value_map = HashMap::new();
        for i in 0..10 {
            let row_ids: Vec<RowId> = (i * 10..(i + 1) * 10)
                .map(|r| RowId::from(r as u64))
                .collect();
            value_map.insert(IndexedValue::Int64(i as i64), row_ids);
        }

        let index = ColumnIndex::build("test_col", &DataType::Int64, value_map).unwrap();

        // Verify stats
        assert_eq!(index.cardinality(), 10); // 10 distinct values
        assert_eq!(index.total_rows(), 100); // 100 total rows

        // Test exact selectivity: 1 / 10 distinct values = 0.1
        let exact_sel = index.estimate_exact_selectivity(&IndexedValue::Int64(5));
        assert!((exact_sel - 0.1).abs() < 0.01);

        // Test IN selectivity: 3 values / 10 distinct = 0.3
        let in_values = vec![
            IndexedValue::Int64(1),
            IndexedValue::Int64(2),
            IndexedValue::Int64(3),
        ];
        let in_sel = index.estimate_in_selectivity(&in_values);
        assert!((in_sel - 0.3).abs() < 0.01);

        // Test range selectivity (harder to verify exactly, but should be reasonable)
        let range_sel =
            index.estimate_range_selectivity(&IndexedValue::Int64(3), &IndexedValue::Int64(7));
        assert!(range_sel > 0.0 && range_sel <= 1.0);
    }

    #[test]
    fn test_selectivity_with_predicate() {
        use crate::index::IndexPredicate;

        let mut value_map = HashMap::new();
        for i in 0..20 {
            let row_ids: Vec<RowId> = (i * 5..(i + 1) * 5)
                .map(|r| RowId::from(r as u64))
                .collect();
            value_map.insert(IndexedValue::Int64(i as i64), row_ids);
        }

        let index = ColumnIndex::build("test_col", &DataType::Int64, value_map).unwrap();

        // Test with Exact predicate
        let exact_pred = IndexPredicate::Exact(IndexedValue::Int64(10));
        let sel = index.estimate_selectivity(&exact_pred);
        assert!((sel - 0.05).abs() < 0.01); // 1/20 = 0.05

        // Test with In predicate
        let in_pred = IndexPredicate::In(vec![IndexedValue::Int64(1), IndexedValue::Int64(2)]);
        let sel = index.estimate_selectivity(&in_pred);
        assert!((sel - 0.1).abs() < 0.01); // 2/20 = 0.1

        // Test with Range predicate
        let range_pred = IndexPredicate::Range {
            min: IndexedValue::Int64(5),
            max: IndexedValue::Int64(15),
        };
        let sel = index.estimate_selectivity(&range_pred);
        assert!(sel > 0.0 && sel <= 1.0);
    }

    #[test]
    fn test_build_streaming_basic() {
        // Create sorted entries (must be pre-sorted for build_streaming)
        let entries: Vec<Result<(IndexedValue, RowId), BundlebaseError>> = vec![
            Ok((IndexedValue::Int64(1), RowId::from(100u64))),
            Ok((IndexedValue::Int64(1), RowId::from(101u64))), // Same value, different rowid
            Ok((IndexedValue::Int64(2), RowId::from(200u64))),
            Ok((IndexedValue::Int64(3), RowId::from(300u64))),
        ];

        let index =
            ColumnIndex::build_streaming("test_col", &DataType::Int64, entries.into_iter())
                .unwrap();

        // Verify structure
        assert_eq!(index.column_name(), "test_col");
        assert_eq!(index.cardinality(), 3); // 3 distinct values
        assert_eq!(index.total_rows(), 4); // 4 total rows

        // Point lookup for value with multiple rows
        let result = index.lookup_exact(&IndexedValue::Int64(1));
        assert_eq!(result.len(), 2);

        // Point lookup for single-row value
        let result = index.lookup_exact(&IndexedValue::Int64(2));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_u64(), 200);
    }

    #[test]
    fn test_build_streaming_matches_build() {
        // Build index using HashMap method
        let mut value_map = HashMap::new();
        value_map.insert(IndexedValue::Int64(1), vec![RowId::from(100u64)]);
        value_map.insert(IndexedValue::Int64(2), vec![RowId::from(200u64)]);
        value_map.insert(
            IndexedValue::Int64(3),
            vec![RowId::from(300u64), RowId::from(301u64)],
        );

        let index_from_map =
            ColumnIndex::build("test_col", &DataType::Int64, value_map).unwrap();

        // Build same index using streaming method (entries must be sorted)
        let entries: Vec<Result<(IndexedValue, RowId), BundlebaseError>> = vec![
            Ok((IndexedValue::Int64(1), RowId::from(100u64))),
            Ok((IndexedValue::Int64(2), RowId::from(200u64))),
            Ok((IndexedValue::Int64(3), RowId::from(300u64))),
            Ok((IndexedValue::Int64(3), RowId::from(301u64))),
        ];

        let index_from_stream =
            ColumnIndex::build_streaming("test_col", &DataType::Int64, entries.into_iter())
                .unwrap();

        // Verify both produce same results
        assert_eq!(index_from_map.cardinality(), index_from_stream.cardinality());
        assert_eq!(index_from_map.total_rows(), index_from_stream.total_rows());

        // Verify lookups match
        for i in 1..=3 {
            let result_map = index_from_map.lookup_exact(&IndexedValue::Int64(i));
            let result_stream = index_from_stream.lookup_exact(&IndexedValue::Int64(i));
            assert_eq!(
                result_map.len(),
                result_stream.len(),
                "Mismatch for value {}",
                i
            );
        }
    }

    #[test]
    fn test_build_streaming_empty() {
        let entries: Vec<Result<(IndexedValue, RowId), BundlebaseError>> = vec![];

        let index =
            ColumnIndex::build_streaming("empty_col", &DataType::Int64, entries.into_iter())
                .unwrap();

        assert_eq!(index.cardinality(), 0);
        assert_eq!(index.total_rows(), 0);
        assert!(index.lookup_exact(&IndexedValue::Int64(1)).is_empty());
    }
}
