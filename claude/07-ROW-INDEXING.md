# Row Indexing System

## Overview

Bundlebase tracks row identity across operations to enable efficient joins and lookups.

## RowId Structure

Each row has a unique identifier combining source and offset:

```rust
pub struct RowId {
    pub block_id: BlockId,  // Identifies the data source
    pub offset: u32,           // Offset within source
}
```

## Row Indexing Workflow

### Index Creation (Lazy)

1. When data is attached: `await c.attach("data.csv")`
2. Index is created on-demand when needed (first join or explicit request)
3. Index file stored: `{block_id}-{file_version}.rowid.idx`

### Index Format

- Binary format for efficiency
- Maps file offsets to logical row IDs
- Supports CSV and JSON Lines files
- Parquet files use native row indices

## Benefits

- Fast row lookups during joins
- Enables row-level tracking through transformations
- Supports efficient distributed joins
- Minimal overhead (lazy creation)

## Example Usage

```python
c = await Bundlebase.create("memory:///test")
await c.attach("users.csv")      # BlockId: 0
await c.attach("orders.csv")     # BlockId: 1

# Row indices created on demand
await c.join(
    "orders",
    "users.id = orders.user_id",
    "inner"
)
# Indices for both users.csv and orders.csv created during join
```

## Implementation Details

### Data Adapter Integration

Row indexing is integrated with the data adapter system:

- **CsvPlugin**: Builds row indices on demand for CSV files
- **JsonLinesPlugin**: Builds row indices on demand for JSON Lines files
- **ParquetPlugin**: Uses native Parquet row indices
- **FunctionPlugin**: Row indices for generated data

### Index Storage

Indices are stored with the container data:
- Location: Container working directory or data directory
- Format: Binary (for performance)
- Caching: Reused across multiple queries

### Performance Characteristics

- **First join**: Index creation overhead (scans file once)
- **Subsequent joins**: O(1) row lookup via binary index
- **Memory**: Minimal (indices are lazy and optional)
- **I/O**: One-time overhead during first join

## Current Limitations

- Row indexing is lazy and built on first use (not pre-computed)
- Indices are not pre-computed when data is attached

## Future Improvements

- Pre-computed row indices for better join performance
- Persistent index caching across sessions
- Distributed index building for large datasets
