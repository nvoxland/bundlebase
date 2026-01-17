use crate::data::RowId;
use crate::io::plugin::object_store::ObjectStoreFile;
use crate::io::IOReadWriteDir;
use crate::BundlebaseError;
use bytes::Bytes;
use crate::object_id::ObjectId;
use futures::stream;
use futures::stream::StreamExt;

const MAGIC_BYTES: &[u8; 8] = b"ROWIDIDX";

/// # Index File Format
/// - Magic bytes: 8 bytes
/// - Version: 1 (1 byte)
/// - Row count: u64 in little-endian (8 bytes)
/// - RowId array: Each as u64 in little-endian
pub struct RowIdIndex {}

impl RowIdIndex {
    pub(crate) fn new() -> Self {
        Self {}
    }

    pub(crate) async fn build(
        &self,
        datafile: &ObjectStoreFile,
        data_dir: &dyn IOReadWriteDir,
        block_id: &ObjectId,
        skip_first_line: bool,
    ) -> Result<Box<dyn crate::io::IOReadFile>, BundlebaseError> {
        // Read stream and collect all bytes
        let mut file_stream = datafile.read_existing().await?;
        let mut buffer = Vec::new();
        while let Some(chunk_result) = file_stream.next().await {
            let chunk = chunk_result?;
            buffer.extend_from_slice(&chunk);
        }
        let data = self.build_row_index(&buffer, block_id, skip_first_line);

        // Serialize index to bytes and write using content-addressed storage
        let index_bytes = self.serialize_index(&data);
        let data_stream = Box::pin(stream::once(async { Ok::<_, std::io::Error>(index_bytes) }));

        let result = data_dir.write_stream(data_stream, "rowid.idx").await?;
        Ok(result.file)
    }

    /// Scan file bytes and build an index of row offsets
    fn build_row_index(
        &self,
        bytes: &[u8],
        block_id: &ObjectId,
        skip_first_line: bool,
    ) -> Vec<RowId> {
        let mut row_ids = Vec::new();
        let mut row_start = 0u64;
        let mut skip_first = skip_first_line;

        for (i, byte) in bytes.iter().enumerate() {
            if *byte == b'\n' {
                let row_end = i as u64;
                let row_size = (row_end - row_start + 1) as usize; // Include newline

                if skip_first {
                    skip_first = false;
                } else {
                    row_ids.push(RowId::new(block_id, row_start, row_size));
                }
                row_start = row_end + 1;
            }
        }

        row_ids
    }

    /// Serialize an index to bytes
    fn serialize_index(&self, data: &[RowId]) -> Bytes {
        let mut buffer = Vec::new();

        // Magic bytes
        buffer.extend_from_slice(MAGIC_BYTES);

        // Version
        buffer.push(1u8);

        // Row count
        buffer.extend_from_slice(&(data.len() as u64).to_le_bytes());

        // RowId array
        for row_id in data {
            buffer.extend_from_slice(&row_id.as_u64().to_le_bytes());
        }

        Bytes::from(buffer)
    }

    /// Load an index from disk with specified magic bytes
    ///
    /// Load an index from disk
    ///
    /// # Arguments
    /// * `file` - IOFile pointing to the layout file
    ///
    /// # Returns
    /// A Vec of RowId values loaded from the index file
    ///
    /// # Errors
    /// Returns an error if the file doesn't exist, is corrupted, has wrong magic bytes, or has an invalid version
    pub async fn load_index(&self, file: &ObjectStoreFile) -> Result<Vec<RowId>, BundlebaseError> {
        let mut stream = file.read_existing().await?;
        let mut buffer = Vec::new();
        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result?;
            buffer.extend_from_slice(&chunk);
        }
        let bytes = bytes::Bytes::from(buffer);

        // Verify magic bytes
        if bytes.len() < 17 || &bytes[0..8] != MAGIC_BYTES {
            return Err("Invalid index file: bad magic bytes".into());
        }

        // Check version
        let version = bytes[8];
        if version != 1 {
            return Err(format!("Unsupported index version: {}", version).into());
        }

        // Read row count
        let row_count_bytes: [u8; 8] = bytes[9..17].try_into()?;
        let row_count = u64::from_le_bytes(row_count_bytes) as usize;

        // Read RowIds
        let mut row_ids = Vec::with_capacity(row_count);
        for i in 0..row_count {
            let offset = 17 + i * 8;
            if offset + 8 > bytes.len() {
                return Err("Index file truncated".into());
            }
            let row_id_bytes: [u8; 8] = bytes[offset..offset + 8].try_into()?;
            let value = u64::from_le_bytes(row_id_bytes);
            row_ids.push(RowId::from(value));
        }

        Ok(row_ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::IOReadWriteFile;
    use crate::test_utils::random_memory_dir_concrete;

    #[test]
    fn test_build_row_index_empty() {
        let block_id = ObjectId::from(1);
        let bytes = b"";
        let result = RowIdIndex::new().build_row_index(bytes, &block_id, false);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_build_row_index_single_line_no_newline() {
        let block_id = ObjectId::from(1u8);
        let bytes = b"data";
        let result = RowIdIndex::new().build_row_index(bytes, &block_id, false);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_build_row_index_single_line_with_newline() {
        let block_id = ObjectId::from(1u8);
        let bytes = b"data\n";
        let result = RowIdIndex::new().build_row_index(bytes, &block_id, false);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].offset(), 0);
        assert_eq!(result[0].block_id(), block_id);
    }

    #[test]
    fn test_build_row_index_multiple_lines() {
        let block_id = ObjectId::from(5u8);
        let bytes = b"line1\nline2\nline3\n";
        let result = RowIdIndex::new().build_row_index(bytes, &block_id, false);
        assert_eq!(result.len(), 3);

        assert_eq!(result[0].offset(), 0);
        assert_eq!(result[0].block_id(), block_id);

        assert_eq!(result[1].offset(), 6);
        assert_eq!(result[1].block_id(), block_id);

        assert_eq!(result[2].offset(), 12);
        assert_eq!(result[2].block_id(), block_id);
    }

    #[test]
    fn test_build_row_index_skip_first_line() {
        let block_id = ObjectId::from(10u8);
        let bytes = b"header\nrow1\nrow2\n";
        let result = RowIdIndex::new().build_row_index(bytes, &block_id, true);
        assert_eq!(result.len(), 2);

        // First row (skip header) should start at offset 7
        assert_eq!(result[0].offset(), 7);
        assert_eq!(result[0].block_id(), block_id);

        // Second row should start at offset 12
        assert_eq!(result[1].offset(), 12);
        assert_eq!(result[1].block_id(), block_id);
    }

    #[test]
    fn test_build_row_index_skip_first_line_only_header() {
        let block_id = ObjectId::from(15u8);
        let bytes = b"header\n";
        let result = RowIdIndex::new().build_row_index(bytes, &block_id, true);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_build_row_index_row_sizes() {
        let block_id = ObjectId::from(35u8);
        let bytes = b"short\nmedium_line\nveryverylongline\n";
        let result = RowIdIndex::new().build_row_index(bytes, &block_id, false);
        assert_eq!(result.len(), 3);

        // Row sizes include the newline character
        assert_eq!(result[0].offset(), 0);
        assert_eq!(result[1].offset(), 6);
        assert_eq!(result[2].offset(), 18);
    }

    #[test]
    fn test_build_row_index_csv_with_header() {
        let block_id = ObjectId::from(40u8);
        let bytes = b"name,age,city\nAlice,30,NYC\nBob,25,LA\n";
        let result = RowIdIndex::new().build_row_index(bytes, &block_id, true);
        assert_eq!(result.len(), 2);

        // Header skipped, first data row is "Alice..."
        assert_eq!(result[0].offset(), 14);
        assert_eq!(result[1].offset(), 27);
    }

    #[tokio::test]
    async fn test_serialize_and_load_index_single_row() {
        let dir = random_memory_dir_concrete();
        let file = dir.io_file("single_row.idx").unwrap();
        let block_id = ObjectId::from(50u8);

        let index = RowIdIndex::new();

        let data = vec![RowId::new(&block_id, 100, 50)];
        let bytes = index.serialize_index(&data);
        file.write(bytes).await.unwrap();

        let loaded = index.load_index(&file).await.unwrap();

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0], data[0]);
        assert_eq!(loaded[0].offset(), 100);
        assert_eq!(loaded[0].block_id(), block_id);
    }

    #[tokio::test]
    async fn test_serialize_and_load_index_multiple_rows() {
        let dir = random_memory_dir_concrete();
        let file = dir.io_file("multi_row.idx").unwrap();
        let block_id = ObjectId::from(60u8);

        let index = RowIdIndex::new();

        let data = vec![
            RowId::new(&block_id, 0, 100),
            RowId::new(&block_id, 100, 200),
            RowId::new(&block_id, 300, 150),
            RowId::new(&block_id, 450, 75),
        ];

        let bytes = index.serialize_index(&data);
        file.write(bytes).await.unwrap();

        let loaded = index.load_index(&file).await.unwrap();

        assert_eq!(loaded.len(), 4);
        for (i, row_id) in data.iter().enumerate() {
            assert_eq!(loaded[i], *row_id);
        }
    }

    #[tokio::test]
    async fn test_index_format_binary_layout() {
        // Verify the exact binary format of serialized index
        let block_id = ObjectId::from(110u8);

        let index = RowIdIndex::new();

        let data = vec![RowId::new(&block_id, 42, 100)];
        let bytes = index.serialize_index(&data);

        // Verify structure
        assert_eq!(&bytes[0..8], MAGIC_BYTES.as_slice()); // Magic bytes
        assert_eq!(bytes[8], 1u8); // Version
        assert_eq!(u64::from_le_bytes(bytes[9..17].try_into().unwrap()), 1u64); // Row count
        assert_eq!(bytes.len(), 25); // 8 + 1 + 8 + 8 for one RowId
    }
}
