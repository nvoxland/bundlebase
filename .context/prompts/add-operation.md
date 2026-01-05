# Template: Add New Operation

Use this template when adding a new operation to bundlebase's transformation pipeline.

## What is an Operation?

Operations are transformations applied to data:
- **Filtering**: `filter("age > 18")`
- **Selection**: `select(["name", "age"])`
- **Column manipulation**: `rename_column()`, `remove_column()`
- **Joining**: `join()` (future)
- **Aggregation**: `aggregate()` (future)

All operations follow the **three-phase pattern**: check → reconfigure → apply.

## Required Reading

Before implementing, read these files IN ORDER:

1. **[decisions/006-lazy-evaluation.md](../decisions/006-lazy-evaluation.md)** - Three-phase operation pattern
2. **[architecture.md](../architecture.md#three-phase-operation-pattern)** - How operations work
3. **[decisions/005-mutable-operations.md](../decisions/005-mutable-operations.md)** - Why operations return `&mut Self`
4. **[ai-rules.md](../ai-rules.md#section-3-operations-development)** - Operation constraints
5. **[anti-patterns.md](../anti-patterns.md#section-3-operations)** - Common operation mistakes

## Critical Constraints

Operations MUST follow these rules:

- ✅ **Implement Operation trait** - All three methods: `check()`, `reconfigure()`, `apply_dataframe()`
- ✅ **Validate in check()** - Catch errors early, before execution
- ✅ **Update schema in reconfigure()** - So schema is known without executing
- ✅ **Stream in apply_dataframe()** - Use `execute_stream()`, never `collect()`
- ✅ **Return `&mut Self`** - Enable method chaining
- ✅ **No `.unwrap()`** - Use proper error handling
- ✅ **Serializable** - Must work with manifest save/load

## Implementation Checklist

### 1. Design Phase

- [ ] Read required documentation (listed above)
- [ ] Understand how existing operations work (read `src/bundle/operations/*.rs`)
- [ ] Determine what the operation does (filter, transform, aggregate, etc.)
- [ ] Identify inputs (SQL expression, column names, parameters)
- [ ] Determine schema impact (does schema change?)
- [ ] Check if operation can be validated early (before execution)

### 2. Create Operation Struct

- [ ] Create new file: `rust/bundlebase/src/bundle/operations/your_operation.rs`
- [ ] Define operation struct with fields
- [ ] Add `#[derive(Clone, Serialize, Deserialize)]` for manifest support
- [ ] Add descriptive doc comments (`///`)

**Example:**
```rust
/// Sorts data by specified columns
#[derive(Clone, Serialize, Deserialize)]
pub struct SortOperation {
    /// Columns to sort by (e.g., ["age", "name"])
    columns: Vec<String>,
    /// Sort order for each column
    ascending: Vec<bool>,
}
```

### 3. Implement Operation Trait

- [ ] Implement all three required methods
- [ ] `check()` - Validate inputs without executing
- [ ] `reconfigure()` - Update BundleState (schema, row count, etc.)
- [ ] `apply_dataframe()` - Execute on DataFrame with streaming

**Template:**
```rust
#[async_trait]
impl Operation for YourOperation {
    fn check(&self, state: &BundleState) -> Result<()> {
        // 1. Validate inputs (columns exist, SQL parses, etc.)
        // 2. Return error if invalid
        // 3. Do NOT execute operation here
        Ok(())
    }

    fn reconfigure(&self, state: &mut BundleState) -> Result<()> {
        // 1. Update schema if operation changes it
        // 2. Update row count if operation filters/samples
        // 3. Update any other metadata
        Ok(())
    }

    async fn apply_dataframe(&self, df: DataFrame) -> Result<DataFrame> {
        // 1. Transform DataFrame using DataFusion API
        // 2. Use streaming execution (no collect())
        // 3. Return modified DataFrame
        df.your_transformation()
            .map_err(|e| format!("Operation failed: {}", e).into())
    }
}
```

### 4. Add to BundleBuilder

- [ ] Open `rust/bundlebase/src/bundle/builder.rs`
- [ ] Add public method for operation
- [ ] Method should return `Result<&mut Self>`
- [ ] Method should call `self.push_operation()`
- [ ] Add doc comments with example

**Example:**
```rust
impl BundleBuilder {
    /// Sort data by specified columns
    ///
    /// # Example
    /// ```no_run
    /// container.sort(&["age", "name"], &[true, false])?;
    /// ```
    pub fn sort(&mut self, columns: &[&str], ascending: &[bool]) -> Result<&mut Self> {
        let op = SortOperation {
            columns: columns.iter().map(|s| s.to_string()).collect(),
            ascending: ascending.to_vec(),
        };
        self.push_operation(op)?;
        Ok(self)
    }
}
```

### 5. Add Python Binding

- [ ] Open `python/bundlebase/src/container.py` (async version)
- [ ] Add method to `AsyncContainer` class
- [ ] Match Rust method signature (but Python-friendly)
- [ ] Add type hints
- [ ] Add docstring with example
- [ ] Open `python/bundlebase/src/container_sync.py`
- [ ] Add same method to `Container` class (sync wrapper)

**Example:**
```python
# AsyncContainer
async def sort(self, columns: list[str], ascending: list[bool] | None = None) -> "AsyncContainer":
    """Sort data by specified columns.

    Args:
        columns: Column names to sort by
        ascending: Sort order for each column (default: all True)

    Returns:
        Self for method chaining

    Example:
        >>> c.sort(["age", "name"], [True, False])
    """
    if ascending is None:
        ascending = [True] * len(columns)
    await self._inner.sort(columns, ascending)
    return self

# Container (sync wrapper)
def sort(self, columns: list[str], ascending: list[bool] | None = None) -> "Container":
    """Sort data by specified columns (synchronous version)."""
    return sync(self._async_impl.sort(columns, ascending))
```

### 6. Write Tests

- [ ] Write Rust unit test in the operation file
- [ ] Write Python E2E test in `python/tests/`
- [ ] Test success case (operation works correctly)
- [ ] Test error cases (invalid columns, bad input, etc.)
- [ ] Test schema changes (if applicable)
- [ ] Test with sample data files
- [ ] Run tests: `cargo test` and `poetry run pytest`

**Example Rust test:**
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sort_operation() {
        let ctx = SessionContext::new();
        let df = ctx.read_parquet("tests/data/sample.parquet", Default::default())
            .await
            .unwrap();

        let op = SortOperation {
            columns: vec!["age".to_string()],
            ascending: vec![true],
        };

        let result = op.apply_dataframe(df).await.unwrap();
        // Verify result (check schema, sample data, etc.)
    }
}
```

**Example Python test:**
```python
def test_sort():
    c = bundlebase.create()
    c.attach("tests/data/sample.parquet")
    c.sort(["age"], [True])

    df = c.to_pandas()

    # Verify sorted
    assert df["age"].is_monotonic_increasing
```

### 7. Documentation

- [ ] Add Rust doc comments to operation struct
- [ ] Add Rust doc comments to BundleBuilder method
- [ ] Add Python docstrings with examples
- [ ] Update [python-api.md](../python-api.md) with new method (if significant)
- [ ] Consider adding to [workflows.md](../workflows.md) if common use case

### 8. Code Review Checklist

- [ ] Operation implements all three trait methods
- [ ] `check()` validates without executing
- [ ] `reconfigure()` updates schema correctly
- [ ] `apply_dataframe()` uses streaming (no `collect()`)
- [ ] No `.unwrap()` calls
- [ ] Returns `&mut Self` for chaining
- [ ] Serializable (derives `Serialize`, `Deserialize`)
- [ ] Python binding returns `self` for chaining
- [ ] Tests cover success and error cases
- [ ] Documentation complete

## Common Pitfalls

### 1. Executing in `check()` instead of validating

**Wrong:**
```rust
fn check(&self, state: &BundleState) -> Result<()> {
    // ❌ Actually executing operation in check()
    let df = state.to_dataframe().await?;
    df.filter(col(&self.column).gt(lit(0))).collect().await?;
    Ok(())
}
```

**Right:**
```rust
fn check(&self, state: &BundleState) -> Result<()> {
    // ✅ Just validate inputs
    if !state.schema().column_with_name(&self.column).is_some() {
        return Err(format!("Column not found: {}", self.column).into());
    }
    Ok(())
}
```

### 2. Using `collect()` in `apply_dataframe()`

**Wrong:**
```rust
async fn apply_dataframe(&self, df: DataFrame) -> Result<DataFrame> {
    let batches = df.collect().await?; // ❌ Loads entire dataset
    // Process batches...
}
```

**Right:**
```rust
async fn apply_dataframe(&self, df: DataFrame) -> Result<DataFrame> {
    // ✅ Return transformed DataFrame (streaming)
    df.filter(col(&self.column).gt(lit(self.value)))
        .map_err(|e| format!("Filter failed: {}", e).into())
}
```

### 3. Not updating schema in `reconfigure()`

**Wrong:**
```rust
fn reconfigure(&self, state: &mut BundleState) -> Result<()> {
    // ❌ Schema changed but not updated
    Ok(())
}
```

**Right:**
```rust
fn reconfigure(&self, state: &mut BundleState) -> Result<()> {
    // ✅ Update schema to reflect column selection
    let new_schema = state.schema().project(&self.columns)?;
    state.set_schema(new_schema);
    Ok(())
}
```

### 4. Forgetting to return `&mut self` in BundleBuilder method

**Wrong:**
```rust
pub fn operation(&mut self, param: &str) -> Result<()> {
    self.push_operation(op)?;
    Ok(()) // ❌ Can't chain
}
```

**Right:**
```rust
pub fn operation(&mut self, param: &str) -> Result<&mut Self> {
    self.push_operation(op)?;
    Ok(self) // ✅ Enables chaining
}
```

### 5. Not making operation serializable

**Wrong:**
```rust
pub struct MyOperation {
    // ❌ Missing derives
    data: Vec<String>,
}
```

**Right:**
```rust
#[derive(Clone, Serialize, Deserialize)]
pub struct MyOperation {
    // ✅ Can be saved in manifest
    data: Vec<String>,
}
```

## Example: Filter Operation

Reference implementation from the codebase:

```rust
// src/bundle/operations/filter.rs

/// Filters rows based on SQL WHERE expression
#[derive(Clone, Serialize, Deserialize)]
pub struct FilterBlock {
    expression: String,
}

#[async_trait]
impl Operation for FilterBlock {
    fn check(&self, state: &BundleState) -> Result<()> {
        // Validate SQL syntax
        parse_sql_expression(&self.expression)?;
        // Validate columns exist
        validate_columns(&self.expression, state.schema())?;
        Ok(())
    }

    fn reconfigure(&self, state: &mut BundleState) -> Result<()> {
        // Schema unchanged, but row count may decrease
        state.set_row_count_approximate();
        Ok(())
    }

    async fn apply_dataframe(&self, df: DataFrame) -> Result<DataFrame> {
        // Execute filter (streaming)
        df.filter(parse_sql_to_expr(&self.expression)?)
            .map_err(|e| format!("Filter failed: {}", e).into())
    }
}
```

## Success Criteria

Operation is complete when:

- ✅ Operation struct defined and documented
- ✅ All three Operation trait methods implemented correctly
- ✅ BundleBuilder method added with `&mut Self` return
- ✅ Python async binding added
- ✅ Python sync binding added
- ✅ Rust tests pass
- ✅ Python E2E tests pass
- ✅ No clippy warnings
- ✅ Streaming execution verified
- ✅ Code review checklist complete

## Related Templates

- [new-feature.md](new-feature.md) - If operation requires new architecture
- [add-python-binding.md](add-python-binding.md) - Focus on Python API only
- [performance-review.md](performance-review.md) - After implementing, optimize

## Related Documentation

- [decisions/006-lazy-evaluation.md](../decisions/006-lazy-evaluation.md) - Why three phases
- [architecture.md](../architecture.md#operations) - How operations fit into architecture
- [python-api.md](../python-api.md) - Python API patterns
