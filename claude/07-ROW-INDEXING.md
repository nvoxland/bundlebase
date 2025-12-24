# Column Indexing System

## Overview

Bundlebase provides a column indexing system for query optimization. Indexes accelerate queries with equality, IN, and range predicates by avoiding full table scans. The system includes intelligent cost-based optimization to automatically choose the best index and skip indexes when a full scan would be faster.

## Key Concepts

### RowId Structure

Each row has a unique identifier combining block and offset:

```rust
pub struct RowId {
    block_id: ObjectId,  // Identifies the data block
    offset: u64,         // Offset within block (0-based)
}
```

### IndexedValue

Values stored in indexes are normalized to support efficient lookup:

```rust
pub enum IndexedValue {
    Int64(i64),
    Float64(OrderedFloat<f64>),  // Wrapper to make f64 comparable
    Utf8(String),
    Boolean(bool),
    Timestamp(i64),
    Null,
}
```

## Architecture

The indexing system consists of several key components:

### 1. Index Definition (`index_definition.rs`)

Tracks which columns are indexed and which blocks have been indexed:

```rust
pub struct IndexDefinition {
    id: ObjectId,                              // Unique index ID
    column: String,                            // Column name
    blocks: Arc<RwLock<Vec<Arc<IndexedBlocks>>>>,  // Index files per block version
}
```

### 2. Column Index (`column_index.rs`)

The physical index structure stored on disk:

```rust
pub struct ColumnIndex {
    column_name: String,
    data_type: DataType,
    blocks: Vec<IndexBlock>,       // Value -> RowId mappings
    directory: IndexDirectory,      // Block-level min/max for range queries
    total_entries: u64,            // Total distinct values
    total_rows: u64,               // Total rows indexed
}

pub struct IndexBlock {
    value: IndexedValue,
    row_ids: Vec<RowId>,           // All rows with this value
}
```

### 3. Filter Analyzer (`filter_analyzer.rs`)

Analyzes DataFusion SQL expressions to extract indexable predicates:

```rust
pub enum IndexPredicate {
    Exact(IndexedValue),                    // column = value
    In(Vec<IndexedValue>),                 // column IN (val1, val2, ...)
    Range { min: IndexedValue, max: IndexedValue },  // column >= min AND column <= max
}

pub struct IndexableFilter {
    pub column: String,
    pub predicate: IndexPredicate,
}
```

Supported SQL patterns:
- **Equality**: `WHERE email = 'test@example.com'`
- **IN lists**: `WHERE status IN ('active', 'pending')`
- **Ranges**: `WHERE age >= 18 AND age <= 65`
- **Single bounds**: `WHERE price < 100.0`

### 4. Index Selector (`index_selector.rs`)

Finds the appropriate index for a column and block version:

```rust
pub fn select_index_from_ref(
    column: &str,
    block: &VersionedBlockId,
    indexes: &Arc<RwLock<Vec<Arc<IndexDefinition>>>>
) -> Option<Arc<IndexDefinition>>
```

### 5. Query Integration (`data_block.rs`)

Integrates indexing with DataFusion's TableProvider:

```rust
impl TableProvider for DataBlock {
    async fn scan(...) -> Result<Arc<dyn ExecutionPlan>> {
        // Phase 1: Try index optimization
        let indexable_filters = FilterAnalyzer::extract_indexable(filters);
        if let Some(best) = self.select_best_index(&indexable_filters, ...).await {
            // Use index to get RowIds
            let row_ids = self.load_and_lookup_index(...).await?;
            return Ok(optimized_scan_with_rowids);
        }

        // Phase 2: Fall back to full scan
        Ok(full_table_scan)
    }
}
```

## Index Operations

### Creating an Index

```python
import bundlebase

c = await bundlebase.create("/path/to/container")
await c.attach("users.csv")

# Define index on email column
await c.define_index("email")

# Build index for all blocks
await c.index_blocks()
```

**Rust operations applied:**
1. `DefineIndexOp` - Creates IndexDefinition, assigns unique ID
2. `IndexBlocksOp` - Scans each block, builds ColumnIndex, saves to disk

### Dropping an Index

```python
await c.drop_index("email")
```

**Rust operation:**
- `DropIndexOp` - Removes IndexDefinition, deletes index files from disk

### Query with Index

```python
# Query automatically uses index if available
df = await c.filter("email = 'test@example.com'")
```

**Execution flow:**
1. DataFusion calls `DataBlock::scan()` with filters
2. `FilterAnalyzer::extract_indexable()` identifies `email = 'test@example.com'`
3. `select_best_index()` finds email index and estimates selectivity
4. If selectivity < 20%, loads index and performs lookup
5. Returns RowIds: `[RowId { block_id: 0, offset: 42 }]`
6. Data reader fetches only matching rows

## Cost-Based Optimization

The system intelligently decides when to use indexes:

### Selectivity Estimation

```rust
impl ColumnIndex {
    pub fn estimate_selectivity(&self, predicate: &IndexPredicate) -> f64 {
        match predicate {
            IndexPredicate::Exact(_) => {
                // Assumes uniform distribution
                1.0 / self.total_entries as f64
            }
            IndexPredicate::In(values) => {
                // Based on number of values
                values.len() as f64 / self.total_entries as f64
            }
            IndexPredicate::Range { min, max } => {
                // Based on overlapping blocks
                overlapping_blocks / total_blocks
            }
        }
    }
}
```

### Index Selection Logic

```rust
async fn select_best_index<'a>(
    &self,
    indexable_filters: &'a [IndexableFilter],
    versioned_block: &VersionedBlockId,
) -> Option<IndexCandidate<'a>> {
    let mut candidates = Vec::new();

    // Evaluate each indexable filter
    for filter in indexable_filters {
        if let Some(index_def) = IndexSelector::select_index_from_ref(...) {
            // Check selectivity
            match self.check_index_selectivity(...).await {
                Ok(Some(selectivity)) => {
                    // Selectivity acceptable, add to candidates
                    candidates.push(IndexCandidate { filter, index_def, selectivity, ... });
                }
                Ok(None) => {
                    // Selectivity > 20% threshold, skip this index
                }
                Err(_) => {
                    // Error checking selectivity, skip this index
                }
            }
        }
    }

    // Choose index with lowest selectivity (most selective)
    candidates.into_iter().min_by(|a, b|
        a.selectivity.partial_cmp(&b.selectivity).unwrap_or(Equal)
    )
}
```

**20% Threshold Rule:**
- If estimated selectivity > 20%, index is skipped
- Full table scan is faster for queries returning >20% of rows
- Avoids index overhead for low-selectivity queries

### Multi-Index Evaluation

When a query has multiple indexed columns:

```sql
SELECT * FROM data WHERE email = 'test@example.com' AND status = 'active';
```

**Process:**
1. Extracts both `email = '...'` and `status = 'active'` as indexable filters
2. Checks if indexes exist for both columns
3. Estimates selectivity for each:
   - Email: 0.001% (1 in 100,000 users)
   - Status: 15% (15,000 active users)
4. Chooses email index (lower selectivity)
5. Logs: "Selected index on column 'email' with selectivity 0.001% (best among 2 candidates)"

## Index File Format

### Binary Layout

```
[Header: 32 bytes]
  - Magic: "BBIDX001" (8 bytes)
  - Version: 1 (1 byte)
  - Data Type: enum (1 byte)
  - Block Count: u32 (4 bytes)
  - Total Entries: u64 (8 bytes)
  - Total Rows: u64 (8 bytes)  # Added in v1 for selectivity
  - Reserved: (2 bytes)

[Index Directory]
  - Entry Count: u32 (4 bytes)
  - For each entry:
    - Min Value: IndexedValue
    - Max Value: IndexedValue
    - Offset: u64 (8 bytes)
    - Length: u32 (4 bytes)

[Index Blocks]
  - For each block:
    - Value: IndexedValue (serialized)
    - RowId Count: u32 (4 bytes)
    - RowIds: [RowId; count]
      - Block ID: u64 (8 bytes)
      - Offset: u64 (8 bytes)
```

### Example

For index on `email` column with 3 distinct values:

```
Header: BBIDX001, Version=1, Type=Utf8, Blocks=3, Entries=3, Rows=1000

Directory:
  Entry 0: min="alice@...", max="alice@...", offset=100, length=50
  Entry 1: min="bob@...", max="bob@...", offset=150, length=50
  Entry 2: min="carol@...", max="carol@...", offset=200, length=50

Blocks:
  Block 0: value="alice@example.com", row_ids=[RowId{block=0, offset=0}, RowId{block=0, offset=42}]
  Block 1: value="bob@example.com", row_ids=[RowId{block=0, offset=100}]
  Block 2: value="carol@example.com", row_ids=[RowId{block=0, offset=200}]
```

## Caching and Performance

### RowId Cache

Loaded indexes are cached in an LRU cache to avoid repeated disk I/O:

```rust
pub struct RowIdCache {
    cache: Arc<Mutex<LruCache<String, Vec<RowId>>>>,
    capacity: usize,  // Default: 100, configurable via BUNDLEBASE_ROWID_CACHE_SIZE
}
```

**Cache key:** `{index_path}#{column}#{predicate_hash}`

**Performance:**
- **Cache hit**: O(1) lookup, no disk I/O
- **Cache miss**: Load index from disk, deserialize, perform lookup

### Memory Efficiency

**IN Predicate Batching:**

For large IN lists, values are processed in batches:

```rust
const BATCH_SIZE: usize = 1000;
let mut unique_row_ids = HashSet::new();

for chunk in vals.chunks(BATCH_SIZE) {
    for val in chunk {
        for row_id in index.lookup_exact(val) {
            unique_row_ids.insert(row_id);  // O(1) deduplication
        }
    }
}

let mut row_ids: Vec<_> = unique_row_ids.into_iter().collect();
row_ids.sort_unstable_by_key(|r| r.as_u64());  // Consistent ordering
```

**Benefits:**
- Avoids materializing all RowIds at once
- O(1) deduplication via HashSet (vs O(n log n) sort+dedup)
- Bounded memory usage for large IN lists

## Observability

### OpenTelemetry Metrics

The index system exports metrics for monitoring:

```rust
#[cfg(feature = "metrics")]
pub fn record_index_lookup(column: &str, outcome: IndexOutcome) {
    INDEX_LOOKUPS.add(1, &[
        KeyValue::new("column", column.to_string()),
        KeyValue::new("outcome", outcome.as_str()),
    ]);
}
```

**Available metrics:**
- `index.lookups` (Counter) - Hit, Miss, Error, Fallback by column
- `index.lookup_duration` (Histogram) - Lookup latency in milliseconds
- `index.cache.operations` (Counter) - Cache hit/miss counts
- `index.cache.size` (Gauge) - Current cache entries
- `index.bytes_read` (Counter) - I/O overhead per column

**Outcomes:**
- **Hit**: Index found, selectivity acceptable, lookup succeeded
- **Miss**: No index exists for this column
- **Error**: Index loading or lookup failed
- **Fallback**: Index exists but selectivity too high (>20%)

### Logging

Simple console logging without external infrastructure:

```python
from bundlebase.index import init_logging_metrics

# Log metrics every 60 seconds to stdout
init_logging_metrics()

# Or custom interval
init_logging_metrics_with_interval(Duration::from_secs(10))
```

**Example output:**
```
[INFO] Index lookups: email=15 (hit), status=5 (fallback), age=3 (miss)
[INFO] Cache stats: hits=120, misses=8, size=23/100
[INFO] Bytes read: email=1.2MB, status=500KB
```

## Version Awareness

Indexes are version-specific to ensure correctness:

```rust
pub struct VersionedBlockId {
    block: ObjectId,
    version: String,  // Increments on any data change
}
```

**Behavior:**
- Each block tracks its current version (incremented on modifications)
- Index files include version in path: `idx_{index_id}_{uuid}.idx`
- Index lookup checks version match before using index
- Stale indexes (wrong version) are ignored, trigger Miss outcome
- `prune_stale_blocks()` removes outdated index references

**Example:**
```
Block v1: [Alice, Bob]
  -> Index file: idx_5_abc123.idx

Filter operation: WHERE age > 18
  -> Block v2: [Alice] (Bob removed)
  -> Old index invalidated (version mismatch)
  -> Query uses full scan until re-indexed
```

## Implementation Details

### Index Building

```rust
impl IndexBlocksOp {
    async fn apply(&self, bundle: &mut Bundle) -> Result<()> {
        for (block_id, version) in &self.blocks {
            // Read block data
            let df = bundle.data_table(block_id)?;

            // Build index for specified column
            let index = ColumnIndex::build_from_dataframe(
                &df,
                &self.column,
                block_id,
                version
            ).await?;

            // Serialize to disk
            let index_bytes = index.serialize()?;
            let index_file = bundle.data_dir().file(&index_path)?;
            index_file.write_bytes(index_bytes).await?;

            // Update index definition
            index_def.add_indexed_blocks(indexed_blocks);
        }
    }
}
```

### Index Lookup

```rust
impl ColumnIndex {
    pub fn lookup_exact(&self, value: &IndexedValue) -> Vec<RowId> {
        // Binary search in blocks
        match self.blocks.binary_search_by(|block| block.value.cmp(value)) {
            Ok(idx) => self.blocks[idx].row_ids.clone(),
            Err(_) => Vec::new(),
        }
    }

    pub fn lookup_range(&self, min: &IndexedValue, max: &IndexedValue) -> Vec<RowId> {
        let mut result = Vec::new();

        // Use directory for pruning
        for entry in &self.directory.entries {
            if max >= &entry.min_value && min <= &entry.max_value {
                // This block overlaps the range
                for block in &self.blocks[entry.offset..entry.offset+entry.length] {
                    if &block.value >= min && &block.value <= max {
                        result.extend_from_slice(&block.row_ids);
                    }
                }
            }
        }

        result
    }
}
```

## Performance Characteristics

### Time Complexity

| Operation | Without Index | With Index |
|-----------|--------------|------------|
| Equality predicate | O(n) full scan | O(log k + m) where k=distinct values, m=matching rows |
| IN predicate (p values) | O(n) full scan | O(p × (log k + m)) |
| Range predicate | O(n) full scan | O(log k + m × b) where b=blocks in range |

### Space Complexity

**Index file size:**
```
Size = Header (32 bytes)
     + Directory (4 + entries × ~100 bytes)
     + Blocks (k × (value_size + 4 + m × 16))
```

For a column with:
- 10,000 distinct values (k)
- Average 10 rows per value (m)
- Average value size 20 bytes

**Index size ≈ 32 + 1,000,000 + 2,000,000 = ~3MB**

### When to Use Indexes

**Good candidates:**
- High-cardinality columns with selective queries (email, user_id, SKU)
- Low-cardinality columns with very selective values (status='failed', active=true)
- Frequently queried columns in large datasets

**Poor candidates:**
- Columns with very low cardinality (boolean flags used in broad filters)
- Columns queried with high-selectivity predicates (status='active' returning 80% of rows)
- Rarely queried columns
- Small datasets (<10,000 rows) where full scan is already fast

## Error Handling

Indexing failures are gracefully handled:

```rust
match self.load_and_lookup_index(...).await {
    Ok(row_ids) => {
        // Use index results
        timer.finish(IndexOutcome::Hit);
        return Ok(optimized_scan);
    }
    Err(e) => {
        // Index failed, fall back to full scan
        log::warn!("Index lookup failed: {}. Falling back to full scan.", e);
        timer.finish(IndexOutcome::Error);
        // Continue to Phase 2: full scan
    }
}
```

**Failure scenarios:**
- Index file not found (deleted or moved)
- Corrupted index file (invalid magic bytes, version mismatch)
- Deserialization error (incompatible format)
- Version mismatch (block data changed since indexing)

**Behavior:**
- Logs warning with error details
- Records Error metric
- Falls back to full table scan (query succeeds)
- Never fails the query due to index issues

## Future Enhancements

Potential improvements not currently implemented:

### Index Intersection

For queries with multiple predicates:
```rust
let row_ids_1 = index_1.lookup_exact(&val1);  // email = '...'
let row_ids_2 = index_2.lookup_exact(&val2);  // status = 'active'

// Intersect (both must match)
let intersection = row_ids_1.into_iter()
    .filter(|rid| row_ids_2.binary_search(rid).is_ok())
    .collect();
```

### Compression

- Use roaring bitmaps for RowId lists (low-cardinality columns)
- Delta encoding for sorted RowIds
- Bloom filters for negative lookups

### Advanced Statistics

- Histograms for better selectivity estimation
- Track actual value distributions (not just uniform)
- Per-block statistics for partition pruning

### Incremental Indexing

- Update indexes on append operations (avoid full rebuild)
- Track which blocks are indexed vs not indexed
- Partial index usage (indexed blocks + full scan for new data)
