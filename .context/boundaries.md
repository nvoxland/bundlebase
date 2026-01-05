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
# Python side (python/bundlebase/container.py)
class AsyncContainer:
    async def filter(self, expression: str) -> "AsyncContainer":
        await self._inner.filter(expression)
        return self
```

**See:** [python-bindings.md](python-bindings.md) for details

---

### 2. Bundle ↔ DataFusion

**Boundary:** DataFrame trait and SessionContext

**Direction:** Bundle uses DataFusion for query execution

**Key Constraints:**
- DataFusion v51 API compatibility
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
- JSON (future)
- Arrow IPC (future)

**See:** [architecture.md](architecture.md#adapters)

---

### 4. Application ↔ File System

**Boundary:** `std::fs` and DataFusion I/O

**Direction:** Bidirectional (read data, write manifests)

**Key Constraints:**
- All paths are UTF-8 strings
- Relative paths resolved from data directory
- Manifest stored as JSON
- Data files immutable (read-only)

**Interface:**
```rust
// Reading data
let df = ctx.read_parquet(path, Default::default()).await?;

// Writing manifest
let manifest = Manifest::from_bundle(&bundle)?;
std::fs::write(manifest_path, serde_json::to_string_pretty(&manifest)?)?;

// Reading manifest
let json = std::fs::read_to_string(manifest_path)?;
let manifest: Manifest = serde_json::from_str(&json)?;
```

**See:** [versioning.md](versioning.md) for manifest format

---

## External Dependencies

### DataFusion (v51)

**Purpose:** SQL query engine and DataFrame API

**Boundary:** All query execution goes through DataFusion

**Usage:**
- Parsing SQL expressions
- Executing filters, projections, joins
- Streaming execution engine
- Query optimization

**Critical dependency** - bundlebase cannot function without DataFusion

---

### Apache Arrow (v57)

**Purpose:** Columnar data format and interop

**Boundary:** All in-memory data uses Arrow RecordBatches

**Usage:**
- Schema representation
- RecordBatch for data rows
- Interop with Pandas, Polars, PyArrow
- Efficient data transfer

**Critical dependency** - DataFusion requires Arrow

---

### PyO3 (v0.23)

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
- Manifest files (`.json`)
- Query results (via `to_pandas()`, etc.)

**Not supported:**
- Writing modified data back to disk (read-only transformations)
- Direct database connections (file-based only)

---

### 3. Jupyter Notebooks

**Integration:** Sync API wrapper for interactive use

**Boundary:** `asyncio.run()` wrapper in `Container` class

**Usage:**
```python
# Works in Jupyter (sync API)
import bundlebase

c = bundlebase.create()  # Sync wrapper
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

**Boundary:** JSON deserialization

**Protection:**
- Type validation via serde
- Schema validation (operations must be known types)
- No code execution in manifests

**Example:**
```json
{
  "version": 1,
  "operations": [
    {"type": "Filter", "expression": "age > 18"}
  ]
}
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
- `Container`, `AsyncContainer` classes
- Core methods: `attach()`, `filter()`, `to_pandas()`, etc.
- Method signatures

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
#[derive(Clone, Serialize, Deserialize)]
struct SortOperation {
    columns: Vec<String>,
    ascending: Vec<bool>,
}

#[async_trait]
impl Operation for SortOperation {
    fn check(&self, state: &BundleState) -> Result<()> { /* ... */ }
    fn reconfigure(&self, state: &mut BundleState) -> Result<()> { /* ... */ }
    async fn apply_dataframe(&self, df: DataFrame) -> Result<DataFrame> { /* ... */ }
}
```

**See:** [prompts/add-operation.md](prompts/add-operation.md)

---

### 3. Custom Functions (future)

**Boundary:** `FunctionRegistry` trait

**Usage:** Register custom SQL functions

**Status:** Planned, not yet implemented

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

**Constraint:** Must use DataFusion v51.x

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
