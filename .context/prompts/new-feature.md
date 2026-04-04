# Template: Add New Feature

Use this template when adding a completely new feature to bundlebase that doesn't fit into the existing operation pipeline.

## Examples of Features

- New data source adapter (JSON, Avro, etc.)
- New query optimization pass
- New indexing strategy
- New Python API surface area
- New storage backend

## Required Reading

Before implementing, read:

1. **[architecture.md](../architecture.md)** - Understand three-tier architecture
2. **[ai-rules.md](../ai-rules.md)** - Critical constraints (streaming, no unwrap, etc.)
3. **[anti-patterns.md](../anti-patterns.md)** - What NOT to do
4. **Relevant ADRs** in [decisions/](../decisions/) - Especially ADR-002 (DataFusion), ADR-003 (Streaming)

## Critical Constraints

Must follow these rules:

- ✅ **Streaming execution only** - Use `execute_stream()`, never `collect()`
- ✅ **No `.unwrap()`** - Code will not compile if you use it
- ✅ **Proper error handling** - Use `Result<T>` and `?` operator
- ✅ **Async where needed** - I/O operations must be async
- ✅ **Type safety** - Leverage Rust's type system
- ✅ **No `mod.rs` files** - Use named module files (e.g., `feature.rs`)

See [ai-rules.md](../ai-rules.md) for full list.

## Implementation Checklist

### 1. Planning Phase

- [ ] Read all required documentation files
- [ ] Understand how feature fits into architecture
- [ ] Identify which tier(s) the feature touches (Bundle trait, Bundle, BundleBuilder)
- [ ] Check if feature requires new dependencies
- [ ] Sketch out API design (Rust and Python)
- [ ] Identify potential performance impacts

### 2. Rust Implementation

- [ ] Create new module file (e.g., `src/new_feature.rs`, NOT `src/new_feature/mod.rs`)
- [ ] Define Rust types and traits
- [ ] Implement core logic with streaming execution
- [ ] Add proper error handling (no `.unwrap()`)
- [ ] Add logging at appropriate levels (see [logging.md](../logging.md))
- [ ] Write Rust unit tests
- [ ] Run `cargo clippy` and fix all warnings
- [ ] Run `cargo test` and verify all tests pass

### 3. Python Bindings (if applicable)

- [ ] Read [python-bindings.md](../python-bindings.md)
- [ ] Create PyO3 wrapper in `python/bundlebase/src/`
- [ ] Handle async/sync bridge if needed (see [sync-api.md](../sync-api.md))
- [ ] Map Rust errors to Python exceptions
- [ ] Add type hints (use `typing` module)
- [ ] Clone `Arc<T>` wrappers for Python return values

### 4. Documentation

- [ ] Add docstrings to all public Rust items (`///` comments)
- [ ] Add Python docstrings with examples
- [ ] Update [CLAUDE.md](../../CLAUDE.md) if feature changes development workflow
- [ ] Consider creating new ADR in [decisions/](../decisions/) if architectural
- [ ] Update [README.md](../README.md) navigation if new major section

### 5. Testing

- [ ] Write Rust unit tests (`#[test]` functions)
- [ ] Write Rust integration tests (if needed, in `tests/` directory)
- [ ] Write Python E2E tests in `python/tests/` (see [testing.md](../testing.md))
- [ ] Test with sample data files
- [ ] Test error conditions (invalid input, missing files, etc.)
- [ ] Run full test suite: `poetry run pytest`
- [ ] Verify streaming behavior (check memory usage doesn't grow with dataset size)

### 6. Performance Validation

- [ ] Verify streaming execution is used (no `collect()` calls)
- [ ] Test with dataset larger than RAM
- [ ] Profile memory usage (should be constant, not proportional to data size)
- [ ] Check for unnecessary clones or allocations
- [ ] Run `cargo build --release` and test release performance

### 7. Code Review Checklist

- [ ] No `.unwrap()` or `.expect()` (except in tests)
- [ ] No `collect()` calls on DataFrames
- [ ] All public items have documentation
- [ ] Error messages are descriptive and include context
- [ ] Follows naming conventions (snake_case for functions, PascalCase for types)
- [ ] No `mod.rs` files
- [ ] Python bindings return cloned Arc wrappers
- [ ] Tests cover success and error paths

## Common Pitfalls

### 1. Using `collect()` instead of streaming

**Wrong:**
```rust
let data = df.collect().await?; // ❌ Loads entire dataset into memory
```

**Right:**
```rust
let stream = df.execute_stream().await?; // ✅ Streaming execution
while let Some(batch) = stream.next().await {
    // Process batch incrementally
}
```

### 2. Using `.unwrap()` for error handling

**Wrong:**
```rust
let value = option.unwrap(); // ❌ Will not compile
```

**Right:**
```rust
let value = option.ok_or_else(|| BundlebaseError::from("value not found"))?; // ✅
```

### 3. Creating `mod.rs` files

**Wrong:**
```
src/
└── feature/
    └── mod.rs  ❌
```

**Right:**
```
src/
└── feature.rs  ✅
```

### 4. Forgetting async/sync bridge in Python

**Wrong:**
```python
# Python binding exposes async only
async def new_feature(self):  # ❌ Hard to use in scripts
    ...
```

**Right:**
```python
# Provide both async and sync wrappers
async def new_feature(self):  # ✅ For async contexts
    ...

class Container:  # Sync wrapper
    def new_feature(self):  # ✅ For scripts/Jupyter
        return sync(self._async_impl.new_feature())
```

### 5. Not testing with large datasets

**Wrong:**
```python
# Test with tiny dataset
def test_feature():
    c = bundlebase.create()
    c.attach("10_row_file.parquet")  # ❌ Doesn't test streaming
```

**Right:**
```python
# Test with dataset larger than RAM
def test_feature_streaming():
    c = bundlebase.create()
    c.attach("10gb_file.parquet")  # ✅ Verifies streaming works
    result = c.to_pandas()  # Should use constant memory
```

## Example: Adding a Reader Plugin

Here's a reference implementation using the existing JSON array reader as a model. This is the pattern for adding a new file format or a new way of reading an existing format via reader-level options.

### Architecture: Reader Plugins

New formats live in `rust/bundlebase-data/src/plugin/` as `ReaderPlugin` implementations. Plugins are registered in `DataReaderFactory` and activated by checking file extension and/or `read_options`.

**Key pattern — `read_options` for format configuration:**

Reader-level options (like `json_record_path`, `json_sep`, `json_meta`) are passed through any connector without connector-specific code changes. Connectors only need to allow them through `validate_connector_args` (prefix with `json_` or `_`). The `ReaderPlugin::reader()` method receives them as `read_options: Option<&HashMap<String, String>>`.

### 1. Planning
- Feature: New file format or variant reader
- Architecture: Implement `ReaderPlugin` trait in `bundlebase-data`
- Dependencies: Add to `bundlebase-data/Cargo.toml` (not `bundlebase`)
- Activation: Decide whether the plugin activates by extension, by `read_options` key, or both

### 2. Create the Plugin

```rust
// rust/bundlebase-data/src/plugin/my_format_reader.rs
use crate::plugin::ReaderPlugin;
use crate::{DataReader, DataContext};
use bundlebase_common::object_id::BlockId;
use bundlebase_common::BundlebaseError;
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

pub const MY_OPTION: &str = "my_option";  // read_options key (use prefix like "my_format_")

#[derive(Default)]
pub struct MyFormatPlugin;

#[async_trait]
impl ReaderPlugin for MyFormatPlugin {
    async fn reader(
        &self,
        source: &str,
        block_id: &BlockId,
        bundle: &dyn DataContext,
        schema: Option<SchemaRef>,
        layout: Option<String>,
        expected_version: Option<String>,
        read_options: Option<&HashMap<String, String>>,
    ) -> Result<Option<Arc<dyn DataReader>>, BundlebaseError> {
        // Check if this plugin applies
        let Some(opts) = read_options else { return Ok(None) };
        let Some(my_opt) = opts.get(MY_OPTION) else { return Ok(None) };
        // ... return Some(reader) or None
        Ok(None)
    }
}
```

### 3. Register the Plugin

```rust
// rust/bundlebase-data/src/plugin.rs
pub mod my_format_reader;
pub use my_format_reader::MyFormatPlugin;

// rust/bundlebase-data/src/reader_factory.rs
plugins: vec![
    Arc::new(MyFormatPlugin::default()),  // ← add before existing plugins
    Arc::new(CsvPlugin::default()),
    // ...
],
```

### 4. Allow Options Through Connectors

If using a new `prefix_` option key, update `validate_connector_args` in `bundlebase-common/src/connector.rs`:

```rust
if !key.starts_with('_') && !key.starts_with("json_") && !key.starts_with("my_format_") && !valid_names.contains(key.as_str()) {
```

### 5. Testing

```rust
// In my_format_reader.rs
#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn test_no_option_returns_none() { ... }

    #[tokio::test]
    async fn test_reads_data_correctly() { ... }

    #[tokio::test]
    async fn test_read_options_round_trip() {
        // Verify read_options() returns the keys used, so they persist in AttachBlockOp
    }
}
```

### 6. Usage (no Python binding changes needed)

Reader options flow through any connector automatically. The connector fetches the file and the reader plugin transforms it, copying the result into the bundle as Parquet. Direct `ATTACH` is for pass-through formats only (CSV, TSV, JSONL, Parquet).

```sql
-- Reader options work with any connector
CREATE SOURCE USING http WITH (url = '...', my_format_option = 'value')
CREATE SOURCE USING remote_dir WITH (url = '...', my_format_option = 'value')
```

## Success Criteria

Feature is complete when:

- ✅ Rust code compiles with no warnings (`cargo clippy`)
- ✅ All Rust tests pass (`cargo test`)
- ✅ Python bindings work correctly
- ✅ All Python E2E tests pass (`poetry run pytest`)
- ✅ Documentation is complete (Rust docs, Python docstrings)
- ✅ Streaming execution verified (constant memory usage)
- ✅ No critical constraints violated
- ✅ Code review checklist complete

## Related Templates

- [add-operation.md](add-operation.md) - If feature is a transformation operation
- [add-python-binding.md](add-python-binding.md) - If only adding Python API
- [performance-review.md](performance-review.md) - For performance-critical features

## Related Documentation

- [architecture.md](../architecture.md) - How features fit into architecture
- [decisions/003-streaming-only.md](../decisions/003-streaming-only.md) - Why streaming matters
- [testing.md](../testing.md) - Testing strategy
