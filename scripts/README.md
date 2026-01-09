# Development Scripts

This directory contains helper scripts for building and developing bundlebase.

## Python Package Build Scripts

### `maturin-dev.sh`

Builds the Python package in development mode using maturin with a separate target directory.

**Usage:**
```bash
./scripts/maturin-dev.sh
```

**Why?** This script uses `target/maturin` instead of `target/debug`, which prevents full rebuilds when switching between maturin and cargo builds. The two build tools use different feature flags (`extension-module`) that would normally invalidate all build artifacts.

**Benefits:**
- ✅ No full rebuilds when switching between IDE/cargo and maturin
- ✅ Fast iteration on both Rust and Python code
- ✅ Parallel build artifacts (~2x disk space, but instant switching)

### `maturin-build.sh`

Builds release wheels for the Python package using maturin with a separate target directory.

**Usage:**
```bash
./scripts/maturin-build.sh [--release]
```

**Example:**
```bash
# Development build
./scripts/maturin-build.sh

# Release build
./scripts/maturin-build.sh --release
```

## Documentation Scripts

### `build_docs.sh`

Builds the project documentation.

**Usage:**
```bash
./scripts/build_docs.sh
```

## Development Workflow

For Python development:
```bash
# Install Python package in development mode
./scripts/maturin-dev.sh

# Run Python tests (automatically uses same target-dir)
poetry run pytest python/tests/

# Make changes to Rust code, then rebuild
./scripts/maturin-dev.sh  # Fast! No full rebuild
```

**Note:** The test suite automatically uses `target/maturin/` via `CARGO_TARGET_DIR` set in `python/tests/conftest.py`. This ensures `maturin_import_hook` (used by tests for auto-rebuild) uses the same target directory as `maturin-dev.sh`, preventing unnecessary rebuilds.

For Rust development:
```bash
# Build Rust libraries only
cargo build --package bundlebase

# Run Rust tests
cargo test --package bundlebase

# Build everything (won't trigger maturin rebuild)
cargo build --all --all-targets
```

## Technical Details

The maturin scripts use `--target-dir target/maturin` to keep build artifacts separate from regular cargo builds:

- `target/debug/` - Cargo builds (without extension-module feature)
- `target/maturin/` - Maturin builds (with extension-module feature)

This separation is necessary because:
1. Maturin requires the `pyo3/extension-module` feature for Python extensions
2. Cargo test requires this feature to be **disabled**
3. Switching between these modes would normally trigger a full rebuild of 200+ crates

By using separate target directories, both build modes can coexist without conflicts.
