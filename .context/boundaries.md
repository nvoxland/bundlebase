# System Boundaries

This document defines bundlebase's system boundaries, integration points, and external interfaces. Understanding these boundaries is critical for maintaining architectural integrity.

## Core Boundaries

### 1. Rust Core ↔ Python Bindings

**Boundary:** PyO3 FFI layer

**Direction:** Rust exposes API to Python via PyO3

**Key Constraints:**
- Python cannot hold Rust references (must clone Arc wrappers)
- Async Rust → Async Python → Sync Python (via `sync()` wrapper)
- Errors must be mapped to Python exceptions with context
- All public Rust items need Python wrappers

**Interface:**
```rust
// Rust side (python/bundlebase/src/)
#[pyclass]
pub struct PyBundleBuilder {
    inner: Arc<BundleBuilder>,  // Arc allows cheap cloning
}

#[pymethods]
impl PyBundleBuilder {
    fn filter(&mut self, expr: String) -> PyResult<Self> {
        self.inner.filter(&expr)
            .map_err(|e| PyErr::new::<PyRuntimeError, _>(format!("Filter failed: {}", e)))?;
        Ok(self.clone())  // Clone Arc for Python
    }
}
```

```python
# Python side (python/src/bundlebase/sync.py wraps the async bindings)
class SyncBundleBuilder(SyncBundle):
    def filter(self, expression: str) -> "SyncBundleBuilder":
        _call_original_method(self._async_bundle, "filter", expression)
        return self
```

**See:** [python-bindings.md](python-bindings.md) for details

---

### 2. Bundle ↔ DataFusion

**Boundary:** DataFrame trait and SessionContext

**Direction:** Bundle uses DataFusion for query execution

**Key Constraints:**
- DataFusion v53 API compatibility
- Streaming execution only (`execute_stream()`, never `collect()`)
- Operations map to DataFusion logical plan
- Schema compatibility with Arrow

**Interface:**
```rust
use datafusion::prelude::*;

impl Bundle {
    pub async fn to_dataframe(&self) -> Result<DataFrame> {
        let ctx = SessionContext::new();
        let df = self.adapter.to_dataframe(&ctx).await?;

        // Apply all operations via DataFusion
        for op in &self.operations {
            df = op.apply_dataframe(df).await?;
        }

        Ok(df)
    }
}
```

**See:** [decisions/002-datafusion-arrow.md](decisions/002-datafusion-arrow.md)

---

### 3. Bundle ↔ Data Sources

**Boundary:** Adapter trait

**Direction:** Adapters read external data into DataFrames

**Key Constraints:**
- Must return DataFusion DataFrame
- Must support streaming reads
- Must preserve schema accurately
- Must handle file format specifics (Parquet, CSV, etc.)

**Interface:**
```rust
#[async_trait]
pub trait DataAdapter {
    async fn to_dataframe(&self, ctx: &SessionContext) -> Result<DataFrame>;
    fn schema(&self) -> Result<SchemaRef>;
}
```

**Supported formats:**
- Parquet (primary format, columnar, compressed)
- CSV (via DataFusion CSV reader)
- TSV (tab-separated, via CSV reader with tab delimiter)
- JSON Lines (via DataFusion JSON reader)
- Arrow IPC (future)

**See:** [architecture.md](architecture.md#adapters)

---

### 4. Application ↔ File System

**Boundary:** `std::fs` and DataFusion I/O

**Direction:** Bidirectional (read data, write manifests)

**Key Constraints:**
- All paths are UTF-8 strings
- Relative paths resolved from data directory
- Manifests/commits stored as YAML in `_bundlebase/`
- Data files immutable; UPDATE/DELETE are recorded as overlay operations, not in-place edits

**Interface:**
```rust
// Reading data
let df = ctx.read_parquet(path, Default::default()).await?;

// Writing a commit (YAML via serde_yaml_ng)
let yaml = serde_yaml_ng::to_string(&commit)?;

// Reading a commit
let commit: BundleCommit = serde_yaml_ng::from_str(&yaml)?;
```

**See:** [versioning.md](versioning.md) for manifest format

---

## External Dependencies

### DataFusion (v53)

**Purpose:** SQL query engine and DataFrame API

**Boundary:** All query execution goes through DataFusion

**Usage:**
- Parsing SQL expressions
- Executing filters, projections, joins
- Streaming execution engine
- Query optimization

**Critical dependency** - bundlebase cannot function without DataFusion

---

### Apache Arrow (v58)

**Purpose:** Columnar data format and interop

**Boundary:** All in-memory data uses Arrow RecordBatches

**Usage:**
- Schema representation
- RecordBatch for data rows
- Interop with Pandas, Polars, PyArrow
- Efficient data transfer

**Critical dependency** - DataFusion requires Arrow

---

### PyO3 (v0.28)

**Purpose:** Rust ↔ Python bindings

**Boundary:** All Python API exposed via PyO3

**Usage:**
- `#[pyclass]` for Rust types
- `#[pymethods]` for Rust methods
- Python exception mapping
- GIL management

**Critical dependency** - only for Python users (Rust API works standalone)

---

### Tokio

**Purpose:** Async runtime

**Boundary:** All async operations run on Tokio runtime

**Usage:**
- DataFusion is async (requires runtime)
- File I/O async
- Python async bridge uses Tokio

**Critical dependency** - DataFusion requires async runtime

---

## Integration Points

### 1. Python Data Science Ecosystem

**Integrations:**
- **Pandas**: `to_pandas()` → `pandas.DataFrame`
- **Polars**: `to_polars()` → `polars.DataFrame`
- **PyArrow**: `stream_batches()` → `pyarrow.RecordBatch` iterator
- **NumPy**: Via Pandas conversion

**Boundary:** Arrow → Python object conversion

**Example:**
```python
# Bundlebase → Pandas
df = container.to_pandas()  # Returns pandas.DataFrame

# Bundlebase → Polars
df = container.to_polars()  # Returns polars.DataFrame

# Bundlebase → PyArrow (streaming)
for batch in container.stream_batches():
    # batch is pyarrow.RecordBatch
    process(batch)
```

---

### 2. File Formats

**Supported inputs:**
- Parquet files (`.parquet`)
- CSV files (`.csv`)
- Directories containing data files

**Supported outputs:**
- Manifest/commit files (`.yaml`)
- Query results (via `to_pandas()`, etc.)
- Tar export (`export_tar()`), `EXPORT DATA` / `EXPORT EMPTY` commands

**Notes:**
- Source data files are never modified in place; `UPDATE`/`DELETE` are recorded as overlay operations within the bundle
- Non-file sources are supported through the connector system (`IMPORT CONNECTOR`, e.g. IPC subprocess connectors)

---

### 3. Jupyter Notebooks

**Integration:** Sync API wrapper for interactive use

**Boundary:** `asyncio.run()` wrapper in `Container` class

**Usage:**
```python
# Works in Jupyter (sync API)
import bundlebase.sync as bb

c = bb.create()  # Sync wrapper
c.attach("data.parquet")
df = c.to_pandas()  # Blocks until complete
```

**See:** [sync-api.md](sync-api.md)

---

## Trust Boundaries

### 1. User-Provided SQL

**Trust level:** UNTRUSTED

**Boundary:** SQL expression parser

**Protection:**
- DataFusion parses and validates SQL
- No arbitrary code execution (SQL only)
- Type checking at parse time
- No database write operations (read-only)

**Example:**
```python
# Safe: DataFusion validates expression
c.filter("age > 18")  # ✅ Validated

# Blocked: Invalid SQL
c.filter("'; DROP TABLE users; --")  # ❌ Parse error
```

---

### 2. File Paths

**Trust level:** TRUSTED (must be controlled by application)

**Boundary:** File system access

**Protection:**
- Paths resolved relative to data directory
- No path traversal protection (assumes trusted input)
- Application must validate paths before passing to bundlebase

**Example:**
```python
# Application must validate paths
user_input = "../../../etc/passwd"  # ⚠️ App must reject

# Bundlebase assumes valid paths
c.attach("data.parquet")  # ✅ Assumes application validated
```

**Warning:** Bundlebase does NOT prevent path traversal. Applications must validate paths.

---

### 3. Manifest Files

**Trust level:** TRUSTED (must be generated by bundlebase)

**Boundary:** YAML deserialization

**Protection:**
- Type validation via serde
- Schema validation (operations must be known types)
- No code execution in manifests

**Example:**
```yaml
author: nathan
message: Initial data load
timestamp: 2024-01-15T10:30:00Z
changes:
  - id: 12345678-...
    description: FILTER age > 18
    operations:
      - type: filter
        expression: age > 18
```

**Warning:** Do not load manifests from untrusted sources. Malicious manifests could specify arbitrary file paths.

---

## API Stability Boundaries

### Public Rust API

**Stable (SemVer guarantees):**
- `Bundle`, `BundleBuilder` types
- `Operation` trait
- Public methods on `BundleBuilder`
- Error types

**Unstable (may change):**
- Internal operation implementations
- Manifest format (versioned separately)
- Adapter trait (may add methods)

---

### Public Python API

**Stable:**
- `SyncBundle` / `SyncBundleBuilder` (sync) and async bundle classes
- Core methods: `attach()`, `filter()`, `to_pandas()`, etc.
- Method signatures

(Note: the project is pre-launch — "stable" means "change deliberately," not a SemVer guarantee.)

**Unstable:**
- Internal PyO3 wrappers
- Error message formats
- Performance characteristics

---

## Extension Points

### 1. Custom Adapters

**Boundary:** `DataAdapter` trait

**Usage:** Implement adapter for new data sources

**Example:**
```rust
struct JsonAdapter { path: String }

#[async_trait]
impl DataAdapter for JsonAdapter {
    async fn to_dataframe(&self, ctx: &SessionContext) -> Result<DataFrame> {
        ctx.read_json(&self.path, Default::default()).await
            .map_err(|e| e.into())
    }

    fn schema(&self) -> Result<SchemaRef> {
        // Infer schema from JSON
    }
}
```

**See:** [architecture.md](architecture.md#adapters)

---

### 2. Custom Operations

**Boundary:** `Operation` trait

**Usage:** Implement new transformation operations

**Example:**
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SortOp {
    columns: Vec<String>,
    ascending: Vec<bool>,
}

impl Operation for SortOp {
    fn describe(&self) -> String { /* ... */ }
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> { /* ... */ }
    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> { /* ... */ }
    async fn apply_dataframe(
        &self,
        df: DataFrame,
        ctx: Arc<SessionContext>,
        bundle_schema: &mut BundleSchema,
    ) -> Result<DataFrame, BundlebaseError> { /* ... */ }
}
```

**See:** [prompts/add-operation.md](prompts/add-operation.md)

---

### 3. Custom Functions

**Boundary:** `IMPORT FUNCTION` command / `bundlebase-udf` crate

**Usage:** Register custom SQL scalar functions, with multi-runtime support (e.g., shared libraries, subprocess runtimes)

**Status:** Implemented

---

## Interoperability Constraints

### Arrow Compatibility

**Requirement:** All data must be Arrow-compatible

**Supported types:**
- Primitives: Int32, Int64, Float32, Float64, Boolean, String
- Complex: List, Struct
- Temporal: Date32, Timestamp

**Not supported:**
- Custom types without Arrow representation
- Non-UTF-8 strings

---

### DataFusion Version Lock

**Constraint:** Must use DataFusion v53.x

**Reason:** API stability, feature compatibility

**Impact:** Cannot upgrade DataFusion without code changes

**See:** [dependencies.md](dependencies.md)

---

## Performance Boundaries

### Memory

**Constraint:** Streaming execution, constant memory usage

**Boundary:** `execute_stream()` vs `collect()`

**Guarantee:** Memory usage independent of dataset size

**See:** [decisions/003-streaming-only.md](decisions/003-streaming-only.md)

---

### Concurrency

**Constraint:** Single-threaded query execution (per container)

**Boundary:** DataFusion parallelism within queries

**Note:** Multiple containers can query in parallel

---

## Summary

| Boundary | Type | Direction | Critical Constraint |
|----------|------|-----------|---------------------|
| Rust ↔ Python | FFI | Rust → Python | Arc cloning required |
| Bundle ↔ DataFusion | API | Bundle uses DF | Streaming only |
| Bundle ↔ Data | Adapter | Read-only | Schema accuracy |
| App ↔ File System | I/O | Bidirectional | Path validation |
| User ↔ SQL | Parser | Untrusted input | SQL validation |

**Key Insight:** Most boundaries enforce **streaming execution** and **type safety** to maintain performance and correctness.
