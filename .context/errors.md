# Error Handling

This document describes bundlebase's error handling patterns, error types, and best practices for reporting and recovering from errors.

## Error Handling Philosophy

Bundlebase follows Rust's error handling model:

1. **No panics** - Library code never panics (except in truly impossible cases)
2. **Explicit errors** - Use `Result<T>` for all fallible operations
3. **Rich context** - Errors include actionable information
4. **Early validation** - Catch errors in `check()` phase, not during execution
5. **Typed errors** - Use custom error types with semantic meaning

**See:** [decisions/007-no-unwrap.md](decisions/007-no-unwrap.md) for rationale

---

## Error Type Hierarchy

### Core Error Type

```rust
pub struct BundlebaseError {
    kind: ErrorKind,
    context: String,
    source: Option<Box<dyn std::error::Error>>,
}

pub enum ErrorKind {
    InvalidOperation,
    ValidationError,
    DataSourceError,
    SchemaError,
    IoError,
    DataFusionError,
}
```

### Usage Pattern

```rust
// Creating errors with context
return Err(BundlebaseError::validation(
    format!("Column '{}' not found in schema", column_name)
));

// Adding context to errors
operation.apply(df)
    .map_err(|e| BundlebaseError::context(
        e,
        format!("Failed to apply {} operation", op_name)
    ))?;
```

---

## Error Handling Patterns

### 1. The `?` Operator (Propagation)

**Use when:** Error should be handled by caller

```rust
pub fn filter(&mut self, expr: &str) -> Result<&mut Self> {
    // Propagate errors up the call stack
    let op = FilterBlock::new(expr)?;
    self.push_operation(op)?;
    Ok(self)
}
```

**Why:** Most common pattern - let caller decide how to handle errors

---

### 2. Pattern Matching (Specific Handling)

**Use when:** Different error types need different handling

```rust
match load_manifest(&path) {
    Ok(manifest) => apply_manifest(manifest),
    Err(BundlebaseError { kind: ErrorKind::IoError, .. }) => {
        // File doesn't exist, create new bundle
        create_new_bundle()
    }
    Err(e) => {
        // Other errors are fatal
        return Err(e);
    }
}
```

**Why:** Allows recovery from specific error conditions

---

### 3. Converting Option to Result

**Use when:** `None` is an error condition

```rust
// ❌ WRONG: Unwrap panics
let column = schema.column_with_name(&name).unwrap();

// ✅ CORRECT: Convert to Result
let column = schema
    .column_with_name(&name)
    .ok_or_else(|| BundlebaseError::validation(
        format!("Column '{}' not found", name)
    ))?;
```

**Why:** Provides meaningful error instead of panic

---

### 4. Early Return for Validation

**Use when:** Validating inputs before processing

```rust
fn check(&self, state: &BundleState) -> Result<()> {
    // Early return on validation errors
    if self.columns.is_empty() {
        return Err(BundlebaseError::validation(
            "Cannot select zero columns"
        ));
    }

    for col in &self.columns {
        if !state.schema().column_with_name(col).is_some() {
            return Err(BundlebaseError::validation(
                format!("Column '{}' not found", col)
            ));
        }
    }

    Ok(())
}
```

**Why:** Validates without executing, catches errors early

---

### 5. Wrapping External Errors

**Use when:** Converting errors from external libraries

```rust
// DataFusion error
let df = ctx.read_parquet(&path, Default::default())
    .await
    .map_err(|e| BundlebaseError::data_source(
        format!("Failed to read Parquet file '{}': {}", path, e)
    ))?;

// I/O error
let contents = std::fs::read_to_string(&path)
    .map_err(|e| BundlebaseError::io(
        format!("Cannot read manifest '{}': {}", path, e)
    ))?;
```

**Why:** Provides context about what was being attempted

---

## Error Categories

### Validation Errors

**When:** User input is invalid

**Examples:**
- Invalid SQL syntax
- Column doesn't exist
- Type mismatch
- Empty required field

**Pattern:**
```rust
fn validate_filter(&self, schema: &Schema) -> Result<()> {
    // Parse SQL expression
    let expr = parse_sql(&self.expression)
        .map_err(|e| BundlebaseError::validation(
            format!("Invalid SQL expression '{}': {}", self.expression, e)
        ))?;

    // Validate columns exist
    for col in expr.referenced_columns() {
        if !schema.column_with_name(&col).is_some() {
            return Err(BundlebaseError::validation(
                format!("Column '{}' not found in schema", col)
            ));
        }
    }

    Ok(())
}
```

**User impact:** Should be caught in `check()` phase, before execution

---

### Data Source Errors

**When:** Cannot read from data source

**Examples:**
- File not found
- Corrupt Parquet file
- Permission denied
- Network error (future)

**Pattern:**
```rust
async fn load_parquet(&self, path: &str) -> Result<DataFrame> {
    ctx.read_parquet(path, Default::default())
        .await
        .map_err(|e| BundlebaseError::data_source(
            format!("Cannot read Parquet file '{}': {}. \
                     Check that file exists and is valid Parquet format.",
                     path, e)
        ))
}
```

**User impact:** Cannot recover automatically, user must fix path/file

---

### Schema Errors

**When:** Schema doesn't match expectations

**Examples:**
- Column type mismatch
- Missing required column
- Schema version incompatibility

**Pattern:**
```rust
fn validate_schema(&self, expected: &Schema, actual: &Schema) -> Result<()> {
    for field in expected.fields() {
        match actual.field_with_name(field.name()) {
            Some(actual_field) if actual_field.data_type() == field.data_type() => {
                // OK
            }
            Some(actual_field) => {
                return Err(BundlebaseError::schema(
                    format!(
                        "Column '{}' has type {} but expected {}",
                        field.name(),
                        actual_field.data_type(),
                        field.data_type()
                    )
                ));
            }
            None => {
                return Err(BundlebaseError::schema(
                    format!("Required column '{}' not found", field.name())
                ));
            }
        }
    }
    Ok(())
}
```

**User impact:** User must modify data or operation to match schema

---

### Execution Errors

**When:** Error during query execution

**Examples:**
- Division by zero
- Numeric overflow
- Out of memory (if streaming violated)

**Pattern:**
```rust
async fn apply_dataframe(&self, df: DataFrame) -> Result<DataFrame> {
    df.filter(self.predicate.clone())
        .map_err(|e| BundlebaseError::execution(
            format!("Filter operation failed on expression '{}': {}",
                    self.expression, e)
        ))
}
```

**User impact:** May indicate bug in operation logic

---

## Python Error Mapping

### Rust to Python Conversion

All Rust errors must be mapped to Python exceptions:

```rust
#[pymethods]
impl PyBundleBuilder {
    fn filter(&mut self, expression: String) -> PyResult<Self> {
        self.inner.filter(&expression)
            .map_err(|e| match e.kind() {
                ErrorKind::ValidationError => {
                    PyErr::new::<PyValueError, _>(e.to_string())
                }
                ErrorKind::DataSourceError => {
                    PyErr::new::<PyFileNotFoundError, _>(e.to_string())
                }
                _ => {
                    PyErr::new::<PyRuntimeError, _>(e.to_string())
                }
            })?;
        Ok(self.clone())
    }
}
```

### Python Exception Types

| Rust ErrorKind | Python Exception | When to Use |
|----------------|------------------|-------------|
| `ValidationError` | `ValueError` | Invalid input |
| `DataSourceError` | `FileNotFoundError` or `IOError` | File issues |
| `SchemaError` | `TypeError` | Type mismatch |
| `IoError` | `IOError` | I/O operations |
| Other | `RuntimeError` | General errors |

---

## Common Error Scenarios

### 1. Column Not Found

**Scenario:** User references non-existent column

**Rust:**
```rust
fn check(&self, state: &BundleState) -> Result<()> {
    if !state.schema().column_with_name(&self.column).is_some() {
        return Err(BundlebaseError::validation(
            format!(
                "Column '{}' not found. Available columns: {}",
                self.column,
                state.schema().field_names().join(", ")
            )
        ));
    }
    Ok(())
}
```

**Python:**
```python
try:
    c.filter("invalid_column > 10")
except ValueError as e:
    print(f"Error: {e}")
    # Error: Column 'invalid_column' not found. Available columns: name, age, status
```

---

### 2. Invalid SQL Expression

**Scenario:** User provides malformed SQL

**Rust:**
```rust
fn check(&self, state: &BundleState) -> Result<()> {
    parse_sql(&self.expression)
        .map_err(|e| BundlebaseError::validation(
            format!(
                "Invalid SQL expression '{}': {}. \
                 Example: \"age > 18 AND status = 'active'\"",
                self.expression, e
            )
        ))?;
    Ok(())
}
```

**Python:**
```python
try:
    c.filter("age > > 18")  # Invalid syntax
except ValueError as e:
    print(f"Error: {e}")
    # Error: Invalid SQL expression 'age > > 18': unexpected token '>'
    # Example: "age > 18 AND status = 'active'"
```

---

### 3. File Not Found

**Scenario:** User attaches non-existent file

**Rust:**
```rust
async fn attach(&mut self, path: &str) -> Result<&mut Self> {
    // Check file exists
    if !std::path::Path::new(path).exists() {
        return Err(BundlebaseError::data_source(
            format!(
                "File '{}' not found. Check path and try again.",
                path
            )
        ));
    }

    // Attempt to read
    let df = ctx.read_parquet(path, Default::default())
        .await
        .map_err(|e| BundlebaseError::data_source(
            format!("Cannot read '{}': {}", path, e)
        ))?;

    Ok(self)
}
```

**Python:**
```python
try:
    c.attach("nonexistent.parquet")
except FileNotFoundError as e:
    print(f"Error: {e}")
    # Error: File 'nonexistent.parquet' not found. Check path and try again.
```

---

### 4. Type Mismatch

**Scenario:** Operation incompatible with column type

**Rust:**
```rust
fn check(&self, state: &BundleState) -> Result<()> {
    let field = state.schema()
        .column_with_name(&self.column)
        .ok_or_else(|| BundlebaseError::validation(
            format!("Column '{}' not found", self.column)
        ))?;

    // Ensure numeric type
    if !matches!(field.data_type(), DataType::Int64 | DataType::Float64) {
        return Err(BundlebaseError::schema(
            format!(
                "Column '{}' has type {} but operation requires numeric type",
                self.column, field.data_type()
            )
        ));
    }

    Ok(())
}
```

**Python:**
```python
try:
    c.filter("name > 100")  # name is string, not number
except TypeError as e:
    print(f"Error: {e}")
    # Error: Column 'name' has type Utf8 but operation requires numeric type
```

---

## Error Handling Anti-Patterns

### ❌ Using `.unwrap()`

**Wrong:**
```rust
let column = schema.column_with_name(&name).unwrap();  // Panics!
```

**Right:**
```rust
let column = schema.column_with_name(&name)
    .ok_or_else(|| BundlebaseError::validation(
        format!("Column '{}' not found", name)
    ))?;
```

**See:** [anti-patterns.md](anti-patterns.md#section-2-error-handling)

---

### ❌ Generic Error Messages

**Wrong:**
```rust
return Err("Operation failed".into());  // Useless error
```

**Right:**
```rust
return Err(BundlebaseError::execution(
    format!(
        "Filter operation failed on column '{}' with expression '{}': {}",
        column, expression, cause
    )
));
```

---

### ❌ Silently Ignoring Errors

**Wrong:**
```rust
let _ = operation.check(state);  // Ignores validation errors!
```

**Right:**
```rust
operation.check(state)?;  // Propagate errors
```

---

### ❌ Catching Errors Too Early

**Wrong:**
```rust
pub fn filter(&mut self, expr: &str) -> &mut Self {
    match self.try_filter(expr) {
        Ok(_) => self,
        Err(_) => self,  // ❌ Swallows error
    }
}
```

**Right:**
```rust
pub fn filter(&mut self, expr: &str) -> Result<&mut Self> {
    self.try_filter(expr)?;  // ✅ Propagate to caller
    Ok(self)
}
```

---

## Error Recovery Strategies

### 1. Retry (Not Common in Bundlebase)

```rust
// Retry on transient errors (network, etc.)
for attempt in 0..3 {
    match load_remote_file(&url).await {
        Ok(data) => return Ok(data),
        Err(e) if e.is_transient() => {
            log::warn!("Attempt {} failed: {}, retrying...", attempt, e);
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        Err(e) => return Err(e),  // Fatal error
    }
}
```

**Use case:** Network operations (future)

---

### 2. Fallback Values

```rust
// Use default if optional config missing
let batch_size = config.batch_size
    .unwrap_or(DEFAULT_BATCH_SIZE);
```

**Use case:** Optional configuration

---

### 3. Partial Success (Not Implemented)

```rust
// Process as much as possible, collect errors
let mut errors = Vec::new();
for item in items {
    if let Err(e) = process(item) {
        errors.push(e);
    }
}

if !errors.is_empty() {
    log::warn!("Processed with {} errors", errors.len());
}
```

**Use case:** Batch processing (future)

---

## Testing Error Conditions

### Rust Tests

```rust
#[test]
fn test_filter_invalid_column() {
    let mut builder = BundleBuilder::new();
    let result = builder.filter("invalid_col > 10");

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("Column 'invalid_col' not found"));
}
```

### Python Tests

```python
def test_filter_error():
    c = bundlebase.create()
    c.attach("tests/data/sample.parquet")

    with pytest.raises(ValueError, match="Column .* not found"):
        c.filter("invalid_column > 10")
```

**See:** [testing.md](testing.md) for more patterns

---

## Error Message Guidelines

### 1. Be Specific

**Wrong:** "Invalid input"
**Right:** "Column 'age' not found in schema. Available columns: name, status"

### 2. Include Context

**Wrong:** "Parse error"
**Right:** "Invalid SQL expression 'age > > 18': unexpected token '>'"

### 3. Suggest Solutions

**Wrong:** "File not found"
**Right:** "File 'data.parquet' not found. Check that path is correct and file exists."

### 4. Show Values

**Wrong:** "Type mismatch"
**Right:** "Column 'age' has type Utf8 but expected Int64"

### 5. Multi-line for Clarity

```rust
format!(
    "Operation 'filter' failed:\n\
     Expression: {}\n\
     Column: {}\n\
     Error: {}\n\
     Hint: Ensure column exists and expression is valid SQL",
    expr, column, cause
)
```

---

## Summary

| Pattern | When to Use | Example |
|---------|-------------|---------|
| `?` operator | Propagate to caller | `operation.check()?` |
| Pattern match | Specific handling | `match err.kind()` |
| `ok_or_else()` | Option → Result | `option.ok_or_else(|| error)?` |
| `map_err()` | Add context | `.map_err(|e| format!("Context: {}", e))` |
| Early return | Validation | `if invalid { return Err(...) }` |

**Key principles:**
- ✅ Never use `.unwrap()` in library code
- ✅ Provide rich context in error messages
- ✅ Validate early (in `check()` phase)
- ✅ Map Rust errors to Python exceptions
- ✅ Make errors actionable (user knows what to fix)

**See also:**
- [decisions/007-no-unwrap.md](decisions/007-no-unwrap.md) - No unwrap enforcement
- [anti-patterns.md](anti-patterns.md#section-2-error-handling) - Error anti-patterns
- [prompts/fix-bug.md](prompts/fix-bug.md) - Debugging error conditions
