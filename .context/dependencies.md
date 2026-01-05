# Dependencies

This document catalogs bundlebase's external dependencies, version constraints, and dependency management policies.

## Dependency Philosophy

1. **Minimize dependencies** - Only add when necessary
2. **Prefer stable, mature libraries** - Avoid experimental crates
3. **Version lock critical dependencies** - Pin DataFusion/Arrow versions
4. **Monitor security** - Regular `cargo audit`
5. **Document rationale** - Explain why each dependency exists

---

## Critical Dependencies

These dependencies are **essential** - bundlebase cannot function without them.

### DataFusion (v51.x)

**Purpose:** SQL query engine and DataFrame API

**Version:** `51.0.0`

**Why this version:**
- Stable API (v51 released December 2024)
- Streaming execution support
- Predicate and projection pushdown
- Rich DataFrame API

**Version policy:** LOCKED - upgrade requires testing

**Rationale:** DataFusion is the core query engine. All query execution goes through DataFusion. Version lock ensures API stability.

**See:** [decisions/002-datafusion-arrow.md](decisions/002-datafusion-arrow.md)

**License:** Apache 2.0

---

### Apache Arrow (v57.x)

**Purpose:** Columnar data format and interoperability

**Version:** `57.0.0`

**Why this version:**
- Matches DataFusion v51 requirement
- Stable schema and RecordBatch APIs
- Interop with Pandas, Polars, PyArrow

**Version policy:** LOCKED - must match DataFusion's Arrow version

**Rationale:** Arrow is DataFusion's data format. All data in bundlebase uses Arrow RecordBatches. Version must match DataFusion to avoid ABI issues.

**License:** Apache 2.0

---

### PyO3 (v0.23.x)

**Purpose:** Rust ↔ Python bindings

**Version:** `0.23`

**Why this version:**
- Stable API (v0.23 released 2024)
- Support for async Python
- Arc-based cloning patterns
- Good GIL management

**Version policy:** Can upgrade minor versions (0.23.x)

**Rationale:** PyO3 is the standard for Rust/Python interop. Enables exposing Rust API to Python.

**License:** Apache 2.0 / MIT

---

### Tokio (v1.x)

**Purpose:** Async runtime

**Version:** `1.x` (latest stable)

**Why this version:**
- Industry standard async runtime
- DataFusion requires Tokio
- Stable v1.x API

**Version policy:** Can use latest v1.x

**Rationale:** DataFusion is async and requires an async runtime. Tokio is the de-facto standard.

**License:** MIT

---

## Important Dependencies

These dependencies provide significant functionality but could theoretically be replaced.

### Serde (v1.x)

**Purpose:** Serialization/deserialization (manifest files)

**Version:** `1.x`

**Usage:**
- Serializing operations to manifest JSON
- Deserializing manifests back to operations
- `#[derive(Serialize, Deserialize)]` on operation types

**Version policy:** Use latest v1.x

**License:** Apache 2.0 / MIT

---

### Serde JSON (v1.x)

**Purpose:** JSON format for manifests

**Version:** `1.x`

**Usage:** Reading/writing `.json` manifest files

**Alternatives:** Could use TOML, YAML, but JSON is ubiquitous

**License:** Apache 2.0 / MIT

---

### Async-trait (v0.1.x)

**Purpose:** Async methods in traits

**Version:** `0.1.x`

**Usage:** `Operation` trait has async `apply_dataframe()` method

**Rationale:** Rust doesn't have native async trait support yet

**Note:** May be removed when Rust supports async trait natively

**License:** Apache 2.0 / MIT

---

### Log (v0.4.x)

**Purpose:** Logging facade

**Version:** `0.4.x`

**Usage:** `log::debug!()`, `log::info!()`, etc.

**Rationale:** Standard Rust logging interface

**See:** [logging.md](logging.md)

**License:** Apache 2.0 / MIT

---

### Env_logger (v0.11.x)

**Purpose:** Simple logger implementation

**Version:** `0.11.x`

**Usage:** Default logger for bundlebase

**Rationale:** Simple, widely used logger

**Alternatives:** tracing, flexi_logger

**License:** Apache 2.0 / MIT

---

## Development Dependencies

These dependencies are only used during development/testing.

### Tokio (test runtime)

**Purpose:** Run async tests

**Usage:**
```rust
#[tokio::test]
async fn test_filter() {
    // Test async code
}
```

**License:** MIT

---

### Tempfile (v3.x)

**Purpose:** Create temporary directories for tests

**Usage:**
```rust
let tmpdir = tempfile::tempdir()?;
let bundle = BundleBuilder::create(tmpdir.path()).await?;
```

**License:** Apache 2.0 / MIT

---

## Python Dependencies

### Core Python Dependencies

Managed via `pyproject.toml`:

```toml
[tool.poetry.dependencies]
python = "^3.8"
pyarrow = "^18.0.0"  # Must match Arrow v57
pandas = "^2.0.0"
polars = "^1.0.0"
```

---

### PyArrow (v18.x)

**Purpose:** Python Arrow library (interop)

**Version:** `18.0.0` (matches Arrow v57)

**Usage:** Converting Arrow RecordBatches to Python objects

**Version policy:** LOCKED - must match Rust Arrow version

**License:** Apache 2.0

---

### Pandas (v2.x)

**Purpose:** DataFrame library for Python

**Version:** `^2.0.0`

**Usage:** `to_pandas()` returns `pandas.DataFrame`

**Version policy:** Compatible with v2.x

**License:** BSD-3-Clause

---

### Polars (v1.x)

**Purpose:** Alternative DataFrame library

**Version:** `^1.0.0`

**Usage:** `to_polars()` returns `polars.DataFrame`

**Version policy:** Compatible with v1.x

**License:** MIT

---

## Build Dependencies

### Maturin (v1.x)

**Purpose:** Build Python wheels from Rust code

**Version:** `^1.0`

**Usage:**
```bash
poetry run maturin develop  # Dev build
poetry run maturin build    # Release build
```

**Rationale:** Standard tool for PyO3 projects

**License:** Apache 2.0 / MIT

---

## Version Constraints

### Hard Locks (MUST NOT CHANGE without testing)

```toml
datafusion = "51.0.0"
arrow = "57.0.0"
pyarrow = "18.0.0"  # In Python
```

**Reason:** API compatibility, ABI compatibility

**Upgrade process:**
1. Read DataFusion/Arrow release notes
2. Check for breaking API changes
3. Update code for API changes
4. Run full test suite
5. Update version in `Cargo.toml` and `pyproject.toml`

---

### Flexible Versions (can upgrade minor)

```toml
pyo3 = "0.23"      # 0.23.x OK
tokio = "1"        # 1.x OK
serde = "1"        # 1.x OK
```

**Reason:** Stable APIs, minor updates safe

---

## Dependency Tree

```
bundlebase
├── datafusion = 51.0.0 (CRITICAL)
│   ├── arrow = 57.0.0 (CRITICAL)
│   ├── tokio = 1.x
│   └── [many DataFusion deps]
├── pyo3 = 0.23
│   └── [PyO3 proc macros]
├── serde = 1.x
├── serde_json = 1.x
├── async-trait = 0.1
├── log = 0.4
└── env_logger = 0.11
```

**Check dependency tree:**
```bash
cargo tree
```

---

## Security

### Cargo Audit

Run regularly to check for known vulnerabilities:

```bash
cargo install cargo-audit
cargo audit
```

**Policy:** Address all high/critical vulnerabilities immediately

---

### Dependabot

GitHub Dependabot monitors dependencies and creates PRs for:
- Security updates
- Version updates

**Policy:**
- Security updates: merge immediately
- Minor updates: review and merge
- Major updates: thorough testing required

---

## Dependency Update Process

### Minor Updates (safe)

```bash
# Update Cargo.lock
cargo update

# Run tests
cargo test
poetry run pytest

# If tests pass, commit
git commit -am "chore: update dependencies"
```

---

### Major Updates (requires testing)

1. **Read release notes** - understand breaking changes
2. **Update Cargo.toml** - change version constraint
3. **Fix compilation errors** - adapt to API changes
4. **Run full test suite** - verify behavior unchanged
5. **Update documentation** - note version requirements
6. **Test with real data** - not just unit tests

**Example: DataFusion upgrade**

```bash
# Check what would change
cargo update --dry-run -p datafusion

# Read release notes
open https://github.com/apache/datafusion/releases

# Update version in Cargo.toml
# datafusion = "52.0.0"

# Fix breaking changes
cargo build
# ... fix errors ...

# Test thoroughly
cargo test
poetry run pytest
# Test with large real datasets

# Update docs
# Update .context/dependencies.md with new version
```

---

## Adding New Dependencies

Before adding a new dependency, ask:

1. **Is it necessary?** Can we implement this ourselves simply?
2. **Is it maintained?** Check recent commits, open issues
3. **Is it stable?** Prefer v1.0+ crates
4. **What's the license?** Must be compatible (Apache 2.0, MIT, BSD)
5. **What's the size?** Does it pull in many transitive deps?
6. **Is it well-documented?** Good docs = good quality
7. **Is there an alternative?** Could we use a different crate?

**Approval process:**
1. Justify need in PR description
2. Document in this file (dependencies.md)
3. Add to relevant ADR if architectural impact

---

## Dependency Licenses

All dependencies must use permissive licenses compatible with Apache 2.0:

**Approved licenses:**
- Apache 2.0
- MIT
- BSD (2-clause, 3-clause)
- ISC

**Not allowed:**
- GPL (copyleft)
- AGPL (copyleft)
- Proprietary licenses

**Check licenses:**
```bash
cargo install cargo-license
cargo license
```

---

## Transitive Dependencies

Bundlebase has many transitive dependencies via DataFusion:

```bash
# Count dependencies
cargo tree | wc -l
# ~200+ total dependencies
```

**Why so many?**
- DataFusion is a complex query engine
- Arrow format has many features
- Each feature may require a crate

**Policy:** Accept transitive deps from DataFusion (vetted by Apache)

**Monitor:** Watch for duplicate versions (`cargo tree -d`)

---

## Platform-Specific Dependencies

### Linux

No platform-specific dependencies

### macOS

No platform-specific dependencies

### Windows

No platform-specific dependencies

**Note:** Bundlebase is platform-agnostic thanks to Rust's portability.

---

## Minimal Dependency Set

For users who only need Rust API (no Python):

```toml
[dependencies]
datafusion = "51.0.0"
arrow = "57.0.0"
tokio = "1"
serde = "1"
serde_json = "1"
async-trait = "0.1"
log = "0.4"
```

For Python bindings, add:
```toml
pyo3 = "0.23"
```

---

## Dependency Rationale Summary

| Dependency | Why | Could Replace? |
|------------|-----|----------------|
| DataFusion | Query engine | No - core functionality |
| Arrow | Data format | No - DataFusion requires it |
| PyO3 | Python bindings | No - only option for Rust/Python |
| Tokio | Async runtime | Technically yes (async-std), but Tokio is standard |
| Serde | Serialization | Yes - but ubiquitous in Rust |
| Log | Logging | Yes - but standard facade |

**Conclusion:** All dependencies are well-justified.

---

## Future Dependency Considerations

### Potential Additions

1. **HTTP client (reqwest)** - for remote data sources
2. **Compression libs** - for additional formats (gzip, zstd)
3. **Database drivers** - for SQL database sources
4. **Cloud SDKs** - for S3, GCS, Azure integration

**Policy:** Only add when feature implemented

---

### Potential Removals

1. **async-trait** - if Rust adds native async trait support
2. **env_logger** - could switch to tracing

**Policy:** Wait for compelling reason to change

---

## Dependency Health Metrics

### DataFusion

- **GitHub stars:** 6k+
- **Release cadence:** Monthly
- **Maintenance:** Active (Apache project)
- **Breaking changes:** Occasional (major versions)
- **Status:** ✅ Healthy

### Arrow

- **GitHub stars:** 15k+
- **Release cadence:** Monthly
- **Maintenance:** Very active (Apache project)
- **Status:** ✅ Healthy

### PyO3

- **GitHub stars:** 12k+
- **Release cadence:** Regular
- **Maintenance:** Very active
- **Status:** ✅ Healthy

### Tokio

- **GitHub stars:** 27k+
- **Release cadence:** Regular
- **Maintenance:** Very active
- **Status:** ✅ Healthy

---

## Troubleshooting Dependency Issues

### Version Conflicts

**Error:** `multiple versions of arrow`

**Solution:**
```bash
# Find duplicate versions
cargo tree -d

# Usually fixed by updating Cargo.lock
cargo update
```

---

### PyArrow Mismatch

**Error:** `Arrow schema incompatibility`

**Solution:** Ensure PyArrow version matches Rust Arrow version

```bash
# Check Rust Arrow version
cargo tree | grep arrow

# Update Python PyArrow
poetry update pyarrow
```

---

### Build Failures

**Error:** Compilation errors after dependency update

**Solution:**
1. Read dependency release notes
2. Check for API breaking changes
3. Update code to match new API
4. Consult dependency documentation

---

## References

- **Cargo Book:** https://doc.rust-lang.org/cargo/
- **DataFusion:** https://github.com/apache/datafusion
- **Arrow:** https://github.com/apache/arrow-rs
- **PyO3:** https://pyo3.rs/
- **Tokio:** https://tokio.rs/

---

## Summary

**Critical dependencies:**
- DataFusion v51 (query engine) - LOCKED
- Arrow v57 (data format) - LOCKED
- PyO3 v0.23 (Python bindings) - minor updates OK
- PyArrow v18 (Python interop) - LOCKED

**Version policy:**
- DataFusion/Arrow: Hard lock, upgrade with caution
- PyO3/Tokio/Serde: Minor updates safe
- Regular `cargo audit` for security
- Dependabot for automated updates

**Adding dependencies:**
- Justify necessity
- Check license compatibility
- Document rationale
- Prefer stable, maintained crates

**See also:**
- [decisions/002-datafusion-arrow.md](decisions/002-datafusion-arrow.md) - Why DataFusion
- [boundaries.md](boundaries.md#external-dependencies) - Dependency boundaries
