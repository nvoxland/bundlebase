# Progress Tracking

Bundlebase provides a pluggable progress tracking system for long-running operations. Progress tracking is always active in the code but silent by default, with optional UI implementations for Python (tqdm) and the REPL (indicatif).

## Architecture

### Core Design: Global Registry with RAII Scopes

The progress tracking system is built around three core concepts:

1. **ProgressTracker trait** - Interface for receiving progress updates
2. **Global registry** - Thread-safe storage for the active tracker
3. **ProgressScope** - RAII wrapper ensuring automatic cleanup

```rust
// Operations create a scope at the start
let _scope = ProgressScope::new("Rebuilding index on salary", Some(total_rows));

// Update progress as work progresses
_scope.update(current_row, None);

// finish() called automatically when scope drops
```

### Default Behavior

By default, a `NoOpTracker` is registered globally. This provides **zero runtime overhead** - all progress tracking calls compile to no-ops when no custom tracker is installed.

Operations **always** call progress tracking methods, but the no-op implementation ensures no performance impact.

### Pluggable Implementations

Different execution environments can register their own progress trackers:

- **Python**: Auto-detects and enables `tqdm` if already installed (via `pip install tqdm`)
- **Rust REPL**: Uses `indicatif` for terminal progress bars
- **Custom**: Users can provide their own tracker implementation

## Usage

### Python

Progress tracking is **automatic** if `tqdm` is installed:

```python
import Bundlebase

# Progress code is always included
# If you have tqdm installed, progress bars appear automatically
# pip install tqdm  # Optional - enables progress bars

# Progress bars appear automatically if tqdm is available
c = await Bundlebase.create("/path/to/container")
await c.attach("large_data.parquet")  # Progress bar shows here (if tqdm installed)
await c.rebuild_index("salary")        # Progress bar shows here (if tqdm installed)
```

#### Custom Callbacks

```python
def my_progress(event, operation, id, current, total, message):
    if event == 'start':
        print(f"Starting: {operation} (total: {total})")
    elif event == 'update':
        pct = (current / total * 100) if total else 0
        print(f"Progress: {pct:.1f}% - {message or ''}")
    elif event == 'finish':
        print(f"Finished: {operation}")

Bundlebase.progress.set_callback(my_progress)
```

#### Disabling Progress

```python
# Disable all progress tracking
Bundlebase.progress.disable()

# Re-enable tqdm
Bundlebase.progress.enable_tqdm()
```

### Rust REPL

Progress bars appear automatically in the REPL:

```bash
$ Bundlebase-server /path/to/container
Bundlebase REPL
Type 'help' for available commands, 'exit' to quit

> attach data.parquet
Attaching 'data.parquet' [=>----] 2/5 - Reading schema
```

The REPL uses `indicatif` which provides:
- **Determinate progress**: Shows `[===>  ] 75/100 (75%)`
- **Indeterminate progress**: Shows spinner for unknown totals
- **Multiple operations**: Concurrent progress bars for parallel operations

### Rust Code

#### Using ProgressScope

```rust
use bundlebase::progress::ProgressScope;

async fn rebuild_index(&self, column: &str) -> Result<(), Error> {
    // Get estimated total (if available)
    let total_rows = self.num_rows().await.ok().map(|n| n as u64);

    // Create scope - finish() called automatically on drop
    let _scope = ProgressScope::new(
        &format!("Rebuilding index on '{}'", column),
        total_rows,
    );

    let mut processed = 0;
    while let Some(batch) = stream.next().await {
        // Process batch...
        processed += batch.num_rows() as u64;

        // Update progress
        _scope.update(processed, None);
    }

    Ok(())
    // _scope drops here, calling finish() automatically
}
```

#### Built-in Trackers

**LoggingTracker** - Outputs progress to the Rust logging system:

```rust
use bundlebase::progress::{LoggingTracker, set_tracker};

// Enable logging (requires env_logger or similar)
env_logger::init();

// Register the logging tracker
set_tracker(Box::new(LoggingTracker::new()));

// Now all progress is logged at INFO/DEBUG levels
// Control with RUST_LOG environment variable:
// RUST_LOG=Bundlebase=debug cargo run
```

Log levels produced:
- **Info**: Operation start/finish
- **Debug**: Progress updates
- **Trace**: All events (requires RUST_LOG=trace)

#### Custom Tracker Implementation

```rust
use bundlebase::progress::{ProgressTracker, ProgressId, set_tracker};

struct MyTracker;

impl ProgressTracker for MyTracker {
    fn start(&self, operation: &str, total: Option<u64>) -> ProgressId {
        println!("Starting: {} (total: {:?})", operation, total);
        ProgressId::new()
    }

    fn update(&self, id: ProgressId, current: u64, message: Option<&str>) {
        println!("Progress {}: {} - {}", id.0, current, message.unwrap_or(""));
    }

    fn finish(&self, id: ProgressId) {
        println!("Finished: {}", id.0);
    }
}

// Register globally
set_tracker(Box::new(MyTracker));
```

## Tracked Operations

The following operations currently report progress:

### High Priority (Process entire datasets)

1. **`rebuild_index(column)`** - Rebuilding column index
   - **Type**: Determinate (if row count known)
   - **Unit**: Rows processed
   - **Example**: `Rebuilding index on 'salary' [=====>] 50000/100000 (50%)`

2. **`index_blocks(column, blocks)`** - Building multi-block index
   - **Type**: Determinate
   - **Unit**: Blocks processed
   - **Example**: `Indexing column 'email' - Block 2/5`

3. **`attach(source)`** - Attaching data source
   - **Type**: Indeterminate (multi-step)
   - **Steps**: Creating adapter → Reading schema → Reading statistics → Building layout
   - **Example**: `Attaching 'data.parquet' - Reading schema`

### Future Candidates

- `commit()` - Persisting operations to disk
- `load_index()` / `save_index()` - Index I/O
- Query execution - Large query processing

## Implementation Details

### Thread Safety

All components are thread-safe:

- **Global registry**: Uses `Arc<RwLock<>>` from `parking_lot`
- **ProgressTracker trait**: Requires `Send + Sync`
- **PyProgressTracker**: Uses `Python::attach()` for GIL safety

### Async Compatibility

Progress tracking works seamlessly with async code:

- Trackers can be called from async contexts
- Python bridge uses `pyo3_async_runtimes`
- No blocking calls in critical paths

### RAII Pattern

`ProgressScope` uses Rust's RAII pattern:

```rust
impl Drop for ProgressScope {
    fn drop(&mut self) {
        let tracker = get_tracker();
        tracker.finish(self.id);
    }
}
```

This ensures `finish()` is **always** called, even if:
- Operation returns early
- Error occurs
- Panic happens

### Memory Overhead

- **NoOpTracker**: Zero-sized (`std::mem::size_of::<NoOpTracker>() == 0`)
- **ProgressScope**: 16 bytes (ProgressId + AtomicU64)
- **Global registry**: Single Arc allocation
- **Active operations**: HashMap entry per in-flight operation

## Testing

### Unit Tests

```rust
#[test]
fn test_progress_scope_lifecycle() {
    let mock = MockTracker::new();
    with_tracker(Box::new(mock.clone()), || {
        {
            let _scope = ProgressScope::new("Test", Some(100));
            // start() called
        } // finish() called via Drop

        let calls = mock.calls();
        assert_eq!(calls.len(), 2); // start + finish
    });
}
```

### Integration Tests

```rust
#[test]
async fn test_rebuild_index_progress() {
    let mock = MockTracker::new();
    with_tracker(Box::new(mock.clone()), || {
        container.rebuild_index("column").await;

        let starts = mock.starts();
        assert_eq!(starts.len(), 1);
        assert!(starts[0].operation.contains("Rebuilding"));
    });
}
```

### Python E2E Tests

```python
async def test_progress_callback():
    events = []

    def callback(event, operation, id, current, total, message):
        events.append(event)

    Bundlebase.progress.set_callback(callback)

    c = await Bundlebase.create()
    await c.attach("data.parquet")

    assert 'start' in events
    assert 'finish' in events
```

## Performance

### Benchmarks

With NoOpTracker (default):
- **Overhead**: < 1% (within measurement error)
- **Compiler optimization**: Calls inlined and eliminated

With active tracker:
- **Per-operation overhead**: ~100ns (ProgressScope creation)
- **Per-update overhead**: ~50ns (AtomicU64 + HashMap lookup)
- **Impact**: Negligible for operations processing 1000+ rows

### Optimization Guidelines

1. **Update frequency**: Don't update on every row, batch updates per RecordBatch
2. **Message allocation**: Pass `None` for message when possible
3. **Tracker complexity**: Keep tracker implementations fast (<1μs per call)

## Troubleshooting

### Progress not showing in Python

**Problem**: No progress bars appear.

**Solution**:
```python
# Check if tqdm is detected
import Bundlebase.progress
print(Bundlebase.progress._has_tqdm())  # Should be True if tqdm installed

# If False, install tqdm:
# pip install tqdm

# Then manually enable
Bundlebase.progress.enable_tqdm()
```

### Progress bars interfere with output

**Problem**: Progress bars overlap with print statements.

**Solution**: tqdm writes to stderr by default, but you can disable:
```python
Bundlebase.progress.disable()
```

### Multiple progress bars in REPL

**Problem**: Concurrent operations show multiple progress bars.

**Solution**: This is expected behavior. indicatif's `MultiProgress` handles this automatically.

### Progress tracking in tests

**Problem**: Progress bars make test output messy.

**Solution**: Use MockTracker in tests:
```rust
use bundlebase::progress::{MockTracker, with_tracker};

#[test]
fn my_test() {
    let mock = MockTracker::new();
    with_tracker(Box::new(mock.clone()), || {
        // Test code here - no terminal output
    });
}
```

## Future Enhancements

### Considered but deferred:

- **Nested progress**: Sub-operation tracking (e.g., "Batch 5/10 → Row 200/500")
- **Batch-level progress**: Finer granularity (currently operation-level only)
- **Cancellation**: User-triggered stop via tracker
- **ETA estimation**: Automatic time-remaining calculation
- **Network progress**: Track remote data fetching
- **Parallel operation aggregation**: Combined progress for parallel tasks

### User feedback needed:

- Should progress bars persist after completion or auto-clear?
- What threshold for showing progress? (Always, or only if >1 second?)
- Should REPL use colored output for progress bars?
- Preference for Unicode vs ASCII progress bars?

## See Also

- [Architecture Overview](01-ARCHITECTURE.md) - Overall system design
- [Python API](02-PYTHON-API.md) - Python usage examples
- [Development Guide](06-DEVELOPMENT.md) - Building and testing
