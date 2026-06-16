# Template: Add New Operation

Use this template when adding a new operation to bundlebase's transformation pipeline.

## What is an Operation?

Operations are recorded transformation steps:
- **Filtering**: `FilterOp` (stored SELECT against the bundle)
- **Column manipulation**: `RenameColumnOp`, `DropColumnOp`, `AddColumnOp`, `CastColumnOp`
- **Data blocks**: `AttachBlockOp`, `DetachBlockOp`, `ReplaceBlockOp`
- **Metadata**: `SetNameOp`, `SetDescriptionOp`
- **Definitions**: `CreateViewOp`, `CreateIndexOp`, `ImportFunctionOp`, `ImportConnectorOp`, ...

All operations implement the `Operation` trait: `check()` (validate), `apply()` (bundle side effects), `apply_dataframe()` (lazy DataFrame transformation). They are registered in the `define_any_operation!` macro.

## Required Reading

Before implementing, read these files IN ORDER:

1. **[decisions/006-lazy-evaluation.md](../decisions/006-lazy-evaluation.md)** - The operation pattern and lazy evaluation
2. **[architecture.md](../architecture.md#operation-pattern-checkapplyapply_dataframe)** - How operations work
3. **[decisions/005-mutable-operations.md](../decisions/005-mutable-operations.md)** - Interior mutability / `&self -> Result<&Self>`
4. **[ai-rules.md](../ai-rules.md)** - Hard constraints (col_<id> naming, streaming, no unwrap)
5. **[anti-patterns.md](../anti-patterns.md)** - Common operation mistakes
6. **The real code**: `rust/bundlebase/src/bundle/operation.rs` (trait + macro) and a simple existing op like `operation/rename_column.rs` or `operation/filter.rs`

## Critical Constraints

Operations MUST follow these rules:

- ✅ **Implement the Operation trait** - `describe()`, `check()`, `apply()`, and (if it transforms data) `apply_dataframe()`
- ✅ **Validate in check()** - Catch errors early, before the op is recorded
- ✅ **Register in `define_any_operation!`** - One line in `operation.rs`
- ✅ **Use `col_<id>` column references** - Stored SQL/columns use stable internal names; translate user input at command-creation time
- ✅ **No `.unwrap()`** - Use proper error handling
- ✅ **Serializable** - `#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]` with `#[serde(rename_all = "camelCase")]` so it round-trips through manifests
- ✅ **No `collect()`** - `apply_dataframe()` returns a transformed (still lazy) DataFrame

## Implementation Checklist

### 1. Design Phase

- [ ] Read required documentation (listed above)
- [ ] Read 1-2 existing operations in `rust/bundlebase/src/bundle/operation/`
- [ ] Determine what the operation does (filter, transform, definition, metadata)
- [ ] Identify inputs (SQL expression, ColumnIds, parameters)
- [ ] Determine whether it needs `apply()` side effects, `apply_dataframe()`, or both
- [ ] Check if operation can be validated early in `check()`

### 2. Create Operation Struct

- [ ] Create new file: `rust/bundlebase/src/bundle/operation/your_operation.rs`
- [ ] Define operation struct with a `setup()` (or `new()`) constructor
- [ ] Derive `Debug, Clone, Serialize, Deserialize, PartialEq` and use `#[serde(rename_all = "camelCase")]`
- [ ] Reference columns by `ColumnId`, not user-visible name

**Example (real, from `rename_column.rs`):**
```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RenameColumnOp {
    pub id: ColumnId,
    pub new_name: String,
}

impl RenameColumnOp {
    pub fn setup(id: ColumnId, new_name: &str) -> Self {
        Self { id, new_name: new_name.to_string() }
    }
}
```

### 3. Implement the Operation Trait

The real trait (from `rust/bundlebase/src/bundle/operation.rs`):

```rust
pub trait Operation: Send + Sync + Clone + Serialize + Debug + Into<AnyOperation> {
    fn describe(&self) -> String;
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError>;
    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError>;
    async fn apply_dataframe(
        &self,
        df: DataFrame,
        _ctx: Arc<SessionContext>,
        _bundle_schema: &mut BundleSchema,
    ) -> Result<DataFrame, BundlebaseError> {
        Ok(df) // default: no DataFrame change
    }
    // Optional overrides: version(), allowed_on_view(), to_empty()
}
```

**Real implementation (from `rename_column.rs`):**
```rust
impl Operation for RenameColumnOp {
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        bundle.column_name(&self.id).ok_or_else(|| {
            BundlebaseError::from(format!("Column with ID '{}' not found", self.id))
        })?;
        if bundle.bundle_schema().column_id(&self.new_name).is_some() {
            return Err(format!("Column '{}' already exists in the schema", self.new_name).into());
        }
        Ok(())
    }

    async fn apply(&self, _bundle: &Bundle) -> Result<(), DataFusionError> {
        Ok(()) // no bundle side effects for this op
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        _ctx: Arc<SessionContext>,
        bundle_schema: &mut BundleSchema,
    ) -> Result<DataFrame, BundlebaseError> {
        // Metadata-only: update the name map for the final rename step.
        bundle_schema.insert(self.id, self.new_name.clone());
        Ok(df)
    }

    fn describe(&self) -> String {
        format!("RENAME COLUMN: {} to {}", self.id, self.new_name)
    }
}
```

Notes:
- `check()` validates against the live `Bundle` (no `BundleState` type exists)
- `apply()` is for bundle side effects via interior mutability (e.g. `AttachBlockOp` registers a pack); many ops leave it as `Ok(())`
- `apply_dataframe()` runs lazily when `Bundle::dataframe()` rebuilds; incoming column names are `col_<id>`

### 4. Register the Operation

All in `rust/bundlebase/src/bundle/operation.rs`:

- [ ] Add `mod your_operation;` to the module declarations at the top
- [ ] Add `pub use crate::bundle::operation::your_operation::YourOperationOp;`
- [ ] Add one line to the `define_any_operation!` macro invocation:

```rust
define_any_operation! {
    AddColumn(AddColumnOp),
    ...
    YourOperation(YourOperationOp),  // ← new line
}
```

The macro generates the `AnyOperation` enum variant, trait dispatch, and `From<YourOperationOp> for AnyOperation`.

### 5. Wire Up a Command / Builder Method

Operations are recorded via `builder.apply_operation(op.into())`, usually from a command in the `bundlebase-command` crate:

- [ ] Add a command (grammar rule + parser + `BundleBuilderCommand` impl) in `rust/bundlebase-command/` if the operation is user-invokable from SQL
- [ ] Add a convenience method to the `BundleBuilderExt` trait (`rust/bundlebase-command/src/builder_ext.rs`); methods take `&self` and return `Result<&Self, BundlebaseError>`
- [ ] If the operation stores SQL or column references, translate user-visible names to `col_<id>` with `builder.translate_sql()` / `bundle_schema().column_id()` BEFORE creating the op

**Example (pattern from builder_ext.rs):**
```rust
async fn rename_column(&self, old_name: &str, new_name: &str)
    -> Result<&Self, BundlebaseError>
{
    exec_cmd(self, RenameColumnCommand::new(old_name, new_name)).await?;
    Ok(self)
}
```

### 6. Add Python Binding

- [ ] Add an async `#[pymethods]` wrapper in `rust/bundlebase-python/src/builder.rs` (clone `slf.inner`, use `pyo3_async_runtimes::tokio::future_into_py`)
- [ ] Register in `python/src/bundlebase/__init__.py`: add to `_original_methods` and (if it mutates and returns the builder) to `mutation_methods`
- [ ] See [add-python-binding.md](add-python-binding.md)

### 7. Write Tests

- [ ] Unit tests in the operation file: `describe()`, serde round-trip, `version()` (see `rename_column.rs` tests)
- [ ] E2E tests in `rust/bundlebase/tests/` (or `bundlebase-command` tests if command-level)
- [ ] Test success case, error cases, and serialization through commit/reopen
- [ ] Thin Python test in `python/tests/` verifying the binding only
- [ ] Run: `cargo test --profile test -p bundlebase` (and `-p bundlebase-command`)

**Example Rust unit test (real, from `rename_column.rs`):**
```rust
#[test]
fn test_config_serialization() {
    let op = RenameColumnOp::setup(ColumnId::generate(), "fname");
    let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
    assert!(serialized.starts_with("id: "));
    assert!(serialized.contains("newName: fname"));
}
```

### 8. Code Review Checklist

- [ ] `check()` validates without executing data
- [ ] `apply()` only touches bundle state via the bundle's accessors/locks
- [ ] `apply_dataframe()` keeps the DataFrame lazy (no `collect()`)
- [ ] Columns referenced by `ColumnId` / `col_<id>`, never user-visible names
- [ ] Registered in `define_any_operation!`
- [ ] No `.unwrap()` calls
- [ ] Serde round-trip tested
- [ ] BundleBuilderExt method is `&self -> Result<&Self>`
- [ ] Python binding registered in `__init__.py`

## Common Pitfalls

### 1. Executing in `check()` instead of validating

**Wrong:**
```rust
async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
    // ❌ Materializing data in check()
    let df = bundle.dataframe().await?;
    (*df).clone().collect().await?;
    Ok(())
}
```

**Right:**
```rust
async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
    // ✅ Just validate inputs against bundle metadata
    if bundle.column_name(&self.id).is_none() {
        return Err(format!("Column with ID '{}' not found", self.id).into());
    }
    Ok(())
}
```

### 2. Using `collect()` in `apply_dataframe()`

**Wrong:**
```rust
let batches = df.collect().await?; // ❌ Loads entire dataset
```

**Right:**
```rust
// ✅ Return a transformed, still-lazy DataFrame
df.filter(col(&internal_name).gt(lit(self.value)))
    .map_err(|e| BundlebaseError::from(e))
```

### 3. Storing user-visible column names

**Wrong:**
```rust
// ❌ Breaks if the column is later renamed
FilterOp::new("SELECT * FROM bundle WHERE salary < 0", vec![])
```

**Right:**
```rust
// ✅ Translate at command-creation time
let translated = builder.translate_sql("SELECT * FROM bundle WHERE salary < 0");
FilterOp::new(translated, vec![]) // stores col_<id> references
```

### 4. Forgetting macro registration

If you create the op but don't add it to `define_any_operation!`, it can't be stored in the operation log or serialized. The compiler will also reject `op.into()` (`Into<AnyOperation>` is generated by the macro).

### 5. Not making the operation serializable

Operations persist in YAML manifests. Missing `Serialize`/`Deserialize` derives or non-camelCase field names break manifest round-trips. Always add `#[serde(rename_all = "camelCase")]` and a serde round-trip test.

## Success Criteria

Operation is complete when:

- ✅ Operation struct defined, documented, serializable
- ✅ Operation trait implemented (`describe`/`check`/`apply`/`apply_dataframe` as needed)
- ✅ Registered in `define_any_operation!`
- ✅ Command and/or `BundleBuilderExt` method added (`&self -> Result<&Self>`)
- ✅ Python binding added and registered
- ✅ Rust tests pass (unit + e2e), Python binding test passes
- ✅ No clippy warnings
- ✅ Code review checklist complete

## Related Templates

- [new-feature.md](new-feature.md) - If operation requires new architecture
- [add-python-binding.md](add-python-binding.md) - Focus on Python API only
- [performance-review.md](performance-review.md) - After implementing, optimize

## Related Documentation

- [decisions/006-lazy-evaluation.md](../decisions/006-lazy-evaluation.md) - Why operations are lazy
- [architecture.md](../architecture.md#operation-pipeline) - How operations fit into architecture
- [python-api.md](../python-api.md) - Python API patterns
