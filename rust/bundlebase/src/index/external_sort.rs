//! External merge sort for streaming index building.
//!
//! Provides infrastructure to sort large datasets that don't fit in memory
//! by using temporary files for intermediate storage.

use crate::data::RowId;
use crate::index::IndexedValue;
use crate::BundlebaseError;
use bytes::{BufMut, BytesMut};
use std::collections::BinaryHeap;
use std::cmp::Ordering;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::PathBuf;

/// Default memory limit for external sort buffer (64MB).
pub const DEFAULT_MEMORY_LIMIT_BYTES: usize = 64 * 1024 * 1024;

/// Configuration for external sort operations.
#[derive(Debug, Clone)]
pub struct ExternalSortConfig {
    /// Maximum bytes to accumulate before flushing a sorted run to disk.
    pub memory_limit_bytes: usize,
    /// Directory to store temporary run files.
    pub temp_dir: PathBuf,
}

impl ExternalSortConfig {
    /// Create a new config with the specified memory limit and temp directory.
    pub fn new(memory_limit_bytes: usize, temp_dir: PathBuf) -> Self {
        Self {
            memory_limit_bytes,
            temp_dir,
        }
    }

    /// Create a config with default memory limit.
    pub fn with_temp_dir(temp_dir: PathBuf) -> Self {
        Self {
            memory_limit_bytes: DEFAULT_MEMORY_LIMIT_BYTES,
            temp_dir,
        }
    }
}

/// A single entry for sorting: (value, row_id).
#[derive(Debug, Clone)]
pub struct SortEntry {
    pub value: IndexedValue,
    pub row_id: RowId,
}

impl SortEntry {
    /// Estimate memory size of this entry in bytes.
    fn size_bytes(&self) -> usize {
        // IndexedValue size + RowId (8 bytes) + Vec overhead (~24 bytes)
        self.value.size_bytes() + 8 + 24
    }

    /// Serialize to bytes for writing to a run file.
    fn serialize(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        let value_bytes = self.value.serialize();
        buf.put_u32(value_bytes.len() as u32);
        buf.put(value_bytes);
        buf.put_u64(self.row_id.as_u64());
        buf
    }

    /// Deserialize from a reader.
    fn deserialize<R: Read>(reader: &mut R) -> Result<Option<Self>, BundlebaseError> {
        // Read value length
        let mut len_buf = [0u8; 4];
        match reader.read_exact(&mut len_buf) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(format!("Failed to read entry length: {}", e).into()),
        }
        let value_len = u32::from_be_bytes(len_buf) as usize;

        // Read value bytes
        let mut value_bytes = vec![0u8; value_len];
        reader.read_exact(&mut value_bytes)
            .map_err(|e| format!("Failed to read entry value: {}", e))?;

        let mut cursor = std::io::Cursor::new(value_bytes.as_slice());
        let value = IndexedValue::deserialize(&mut cursor)?;

        // Read row_id
        let mut rowid_buf = [0u8; 8];
        reader.read_exact(&mut rowid_buf)
            .map_err(|e| format!("Failed to read row_id: {}", e))?;
        let row_id = RowId::from(u64::from_be_bytes(rowid_buf));

        Ok(Some(SortEntry { value, row_id }))
    }
}

impl PartialEq for SortEntry {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value && self.row_id.as_u64() == other.row_id.as_u64()
    }
}

impl Eq for SortEntry {}

impl PartialOrd for SortEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SortEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.value.cmp(&other.value) {
            Ordering::Equal => self.row_id.as_u64().cmp(&other.row_id.as_u64()),
            ord => ord,
        }
    }
}

/// Writer that accumulates entries, flushes sorted runs when memory limit exceeded.
pub struct ExternalSortWriter {
    config: ExternalSortConfig,
    buffer: Vec<SortEntry>,
    current_size: usize,
    run_files: Vec<PathBuf>,
    run_counter: usize,
}

impl ExternalSortWriter {
    /// Create a new external sort writer with the given configuration.
    pub fn new(config: ExternalSortConfig) -> Result<Self, BundlebaseError> {
        // Ensure temp directory exists
        std::fs::create_dir_all(&config.temp_dir)
            .map_err(|e| format!("Failed to create temp dir {:?}: {}", config.temp_dir, e))?;

        Ok(Self {
            config,
            buffer: Vec::with_capacity(1024),
            current_size: 0,
            run_files: Vec::new(),
            run_counter: 0,
        })
    }

    /// Add an entry to the sorter.
    ///
    /// If adding this entry exceeds the memory limit, the current buffer
    /// is sorted and flushed to a run file.
    pub fn add(&mut self, value: IndexedValue, row_id: RowId) -> Result<(), BundlebaseError> {
        let entry = SortEntry { value, row_id };
        let entry_size = entry.size_bytes();

        // Check if we need to flush before adding
        if self.current_size + entry_size > self.config.memory_limit_bytes && !self.buffer.is_empty() {
            self.flush_run()?;
        }

        self.current_size += entry_size;
        self.buffer.push(entry);
        Ok(())
    }

    /// Flush current buffer as a sorted run to disk.
    fn flush_run(&mut self) -> Result<(), BundlebaseError> {
        if self.buffer.is_empty() {
            return Ok(());
        }

        // Sort the buffer
        self.buffer.sort();

        // Create run file
        let run_path = self.config.temp_dir.join(format!("run_{:06}.bin", self.run_counter));
        self.run_counter += 1;

        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&run_path)
            .map_err(|e| format!("Failed to create run file {:?}: {}", run_path, e))?;

        let mut writer = BufWriter::new(file);

        // Write entry count
        writer.write_all(&(self.buffer.len() as u64).to_be_bytes())
            .map_err(|e| format!("Failed to write entry count: {}", e))?;

        // Write all entries
        for entry in &self.buffer {
            let bytes = entry.serialize();
            writer.write_all(&bytes)
                .map_err(|e| format!("Failed to write entry: {}", e))?;
        }

        writer.flush()
            .map_err(|e| format!("Failed to flush run file: {}", e))?;

        self.run_files.push(run_path);
        self.buffer.clear();
        self.current_size = 0;

        Ok(())
    }

    /// Finish writing and return an iterator over all sorted entries.
    ///
    /// This performs a k-way merge of all run files (if any) and the
    /// remaining in-memory buffer.
    pub fn finish(mut self) -> Result<SortedEntryIterator, BundlebaseError> {
        // If everything fits in memory (no run files), just sort and return
        if self.run_files.is_empty() {
            self.buffer.sort();
            return Ok(SortedEntryIterator::InMemory(InMemoryIterator {
                entries: self.buffer,
                index: 0,
            }));
        }

        // Flush any remaining entries as final run
        self.flush_run()?;

        // Create k-way merge iterator
        Ok(SortedEntryIterator::Merge(MergeIterator::new(self.run_files)?))
    }

    /// Get the number of run files created so far.
    pub fn run_count(&self) -> usize {
        self.run_files.len()
    }
}

/// Iterator over sorted entries from external sort.
pub enum SortedEntryIterator {
    /// All entries fit in memory, no disk I/O needed.
    InMemory(InMemoryIterator),
    /// K-way merge over multiple run files.
    Merge(MergeIterator),
}

impl Iterator for SortedEntryIterator {
    type Item = Result<SortEntry, BundlebaseError>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            SortedEntryIterator::InMemory(iter) => iter.next(),
            SortedEntryIterator::Merge(iter) => iter.next(),
        }
    }
}

/// In-memory iterator when all entries fit in the buffer.
pub struct InMemoryIterator {
    entries: Vec<SortEntry>,
    index: usize,
}

impl Iterator for InMemoryIterator {
    type Item = Result<SortEntry, BundlebaseError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.entries.len() {
            let entry = self.entries[self.index].clone();
            self.index += 1;
            Some(Ok(entry))
        } else {
            None
        }
    }
}

/// Entry in the merge heap, tracking which run file it came from.
struct HeapEntry {
    entry: SortEntry,
    run_index: usize,
}

impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.entry == other.entry
    }
}

impl Eq for HeapEntry {}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering for min-heap behavior (BinaryHeap is max-heap)
        other.entry.cmp(&self.entry)
    }
}

/// K-way merge iterator over multiple sorted run files.
pub struct MergeIterator {
    readers: Vec<Option<BufReader<File>>>,
    heap: BinaryHeap<HeapEntry>,
}

impl MergeIterator {
    fn new(run_files: Vec<PathBuf>) -> Result<Self, BundlebaseError> {
        let mut readers = Vec::with_capacity(run_files.len());
        let mut heap = BinaryHeap::with_capacity(run_files.len());

        for (idx, path) in run_files.iter().enumerate() {
            let file = File::open(path)
                .map_err(|e| format!("Failed to open run file {:?}: {}", path, e))?;
            let mut reader = BufReader::new(file);

            // Skip entry count (we don't need it for iteration)
            let mut count_buf = [0u8; 8];
            reader.read_exact(&mut count_buf)
                .map_err(|e| format!("Failed to read entry count: {}", e))?;

            // Read first entry from each run
            if let Some(entry) = SortEntry::deserialize(&mut reader)? {
                heap.push(HeapEntry { entry, run_index: idx });
                readers.push(Some(reader));
            } else {
                readers.push(None);
            }
        }

        Ok(Self { readers, heap })
    }
}

impl Iterator for MergeIterator {
    type Item = Result<SortEntry, BundlebaseError>;

    fn next(&mut self) -> Option<Self::Item> {
        // Pop smallest entry from heap
        let HeapEntry { entry, run_index } = self.heap.pop()?;

        // Read next entry from the same run file
        if let Some(ref mut reader) = self.readers[run_index] {
            match SortEntry::deserialize(reader) {
                Ok(Some(next_entry)) => {
                    self.heap.push(HeapEntry {
                        entry: next_entry,
                        run_index,
                    });
                }
                Ok(None) => {
                    // Run exhausted, mark reader as done
                    self.readers[run_index] = None;
                }
                Err(e) => return Some(Err(e)),
            }
        }

        Some(Ok(entry))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(value: i64, row_id: u64) -> SortEntry {
        SortEntry {
            value: IndexedValue::Int64(value),
            row_id: RowId::from(row_id),
        }
    }

    #[test]
    fn test_sort_entry_serialization() {
        let entry = make_entry(42, 100);
        let bytes = entry.serialize();

        let mut reader = std::io::Cursor::new(bytes.as_ref());
        let deserialized = SortEntry::deserialize(&mut reader).unwrap().unwrap();

        assert_eq!(deserialized.value, IndexedValue::Int64(42));
        assert_eq!(deserialized.row_id.as_u64(), 100);
    }

    #[test]
    fn test_in_memory_sort() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = ExternalSortConfig {
            memory_limit_bytes: 1024 * 1024, // 1MB - plenty for this test
            temp_dir: temp_dir.path().to_path_buf(),
        };

        let mut writer = ExternalSortWriter::new(config).unwrap();

        // Add entries in reverse order
        for i in (0..100).rev() {
            writer.add(IndexedValue::Int64(i), RowId::from(i as u64)).unwrap();
        }

        // Should have no run files (everything fits in memory)
        assert_eq!(writer.run_count(), 0);

        let iter = writer.finish().unwrap();
        let entries: Vec<_> = iter.map(|r| r.unwrap()).collect();

        assert_eq!(entries.len(), 100);

        // Verify sorted order
        for (i, entry) in entries.iter().enumerate() {
            assert_eq!(entry.value, IndexedValue::Int64(i as i64));
        }
    }

    #[test]
    fn test_external_sort_with_runs() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = ExternalSortConfig {
            memory_limit_bytes: 100, // Very small to force multiple runs (~40 bytes per entry)
            temp_dir: temp_dir.path().to_path_buf(),
        };

        let mut writer = ExternalSortWriter::new(config).unwrap();

        // Add entries in random-ish order
        let values: Vec<i64> = vec![50, 10, 90, 30, 70, 20, 80, 40, 60, 100, 5, 95];
        for (idx, &val) in values.iter().enumerate() {
            writer.add(IndexedValue::Int64(val), RowId::from(idx as u64)).unwrap();
        }

        // Should have created some run files (each entry is ~40 bytes, limit is 100 bytes)
        assert!(writer.run_count() > 0, "Expected multiple runs with 100 byte limit and 12 entries");

        let iter = writer.finish().unwrap();
        let entries: Vec<_> = iter.map(|r| r.unwrap()).collect();

        assert_eq!(entries.len(), values.len());

        // Verify sorted order
        let mut prev_val = i64::MIN;
        for entry in &entries {
            if let IndexedValue::Int64(val) = entry.value {
                assert!(val >= prev_val, "Entries should be sorted");
                prev_val = val;
            } else {
                panic!("Unexpected value type");
            }
        }
    }

    #[test]
    fn test_sort_entry_ordering() {
        let a = make_entry(1, 100);
        let b = make_entry(2, 50);
        let c = make_entry(1, 200);

        // Primary sort by value
        assert!(a < b);

        // Secondary sort by row_id when values equal
        assert!(a < c);
    }

    #[test]
    fn test_empty_writer() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = ExternalSortConfig::with_temp_dir(temp_dir.path().to_path_buf());

        let writer = ExternalSortWriter::new(config).unwrap();
        let iter = writer.finish().unwrap();
        let entries: Vec<_> = iter.collect();

        assert!(entries.is_empty());
    }
}
