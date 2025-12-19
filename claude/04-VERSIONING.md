# Commit-Based Versioning

## Manifest System

Bundlebase uses a versioned manifest system for persistence:

**Manifest storage:**
- Location: `{data_dir}/_manifest/` directory
- Format: YAML files with 5-digit version + 12-character hash
- Example: `00001-a1b2c3d4e5f6.yaml`

**Manifest structure:**
```yaml
# Container metadata
name: "my_container"
description: "Container description"
num_rows: 1000
schema:
  - name: "id"
    type: "Int64"
  - name: "name"
    type: "Utf8"

# Base container reference (for from)
from: null  # or path to base container

# Operations applied
operations:
  - type: "AttachBlock"
    source: "data.parquet"  # Relative path
    version: "a1b2c3d4e5f6"
  - type: "RemoveColumns"
    names: ["temp_col"]

# Commit metadata
version: 1
hash: "a1b2c3d4e5f6"
created_at: "2024-01-15T10:30:00Z"
message: "Initial data load"
```

## Commit Workflow

```python
# Create mutable container
c = await Bundlebase.create("/my/container")

# Make modifications
await c.attach("data.parquet")
await c.remove_column("sensitive")

# Commit creates versioned snapshot
await c.commit("Cleaned sensitive data")
# Creates: /my/container/_manifest/{version}-{hash}.yaml

# Later, open the container
c = await Bundlebase.open("/my/container")
# Loads the latest manifest and reconstructs container state
```

## From Chain

Containers can extend other containers to build on previous versions:

```python
# Load committed container
base = await Bundlebase.open("/base/container")

# Extend to new directory with new modifications
extended = await base.extend("/extended/container")
await extended.attach("new_data.parquet")
await extended.commit("Added new data")

# Results in manifest:
# {
#   "from": "/base/container",
#   "operations": [...]
# }
```

**Benefits:**
- **Version history**: All commits are preserved
- **Branching**: Can extend from any previous commit
- **Relative paths**: Manifests support relative paths for portability
- **Circular dependency detection**: Prevents invalid from chains

## Path Handling

### Path Resolution

The `attach()` method handles paths flexibly:

```rust
pub async fn attach(&mut self, path: &str) -> Result<&mut Self> {
    let url = if path.contains(":") {
        Url::parse(path)?  // Already a URL (file://, function://)
    } else {
        // Convert relative/absolute file path to file:// URL
        Url::from_file_path(std::fs::canonicalize(path)?).unwrap()
    };
    // ...
    Ok(self)
}
```

**Supported formats:**
- Relative paths: `"test_data/file.parquet"` → resolves to absolute file:// URL
- Absolute paths: `"/full/path/to/file.csv"` → converts to file:// URL
- File URLs: `"file:///full/path/to/file.json"` → used as-is
- Function URLs: `"function://my_func"` → handled by FunctionPlugin

**Path resolution:**
- Relative paths resolved using `std::fs::canonicalize()` (resolves from current working directory)
- Result converted to absolute file:// URL
- DataFusion handles file:// URLs natively

**Test data location:**
- All test data in `test_data/` directory at project root
- Rust tests: `"test_data/userdata.parquet"`
- Python tests: `"test_data/userdata.parquet"` (pytest runs from project root)

## Error Handling

**Current implementation:**
- Python bindings use `.map_err()` to convert Rust errors to Python exceptions
- Operation validation happens in the `check()` phase (immediate feedback)
- State modification in `reconfigure()` phase validates preconditions
- DataFrame transformation errors surface during `apply_dataframe()`

**Error flow:**

```python
c = await Bundlebase.create("memory:///test_container")
await c.attach("data.parquet")

# Validation happens immediately (check phase)
try:
    await c.remove_column("nonexistent")  # May raise error here
except Exception as e:
    print(f"Validation error: {e}")

# If validation passes, operation is recorded
await c.filter("invalid_column > 5")  # May pass check, fail on query

# Execution errors surface during query
try:
    results = await c.to_dict()  # DataFrame transformation error
except Exception as e:
    print(f"Execution error: {e}")
```

**Error types:**
- **Validation errors**: Column not found, invalid schema, type mismatches
- **I/O errors**: File not found, permission denied, network errors
- **Serialization errors**: Invalid manifest format, version mismatch
- **Execution errors**: DataFusion query errors, missing data

**Best practices:**
1. Check container schema before operations: `c.schema.field("column_name")`
2. Validate data types before operations
3. Handle errors from async operations with try/except
4. Use descriptive error messages for debugging
