# Bundlebase Project Overview

Bundlebase is like Docker, but for data. It allows you to bundle up existing data into a standard interface which can be queries with SQL and interacted with via Python.

## Documentation Index

For detailed documentation, see the modular guides in the `.context/` directory:

**Start Here:**
- **[README.md](.context/README.md)** - Project overview, navigation hub, and documentation standards

**Core Architecture:**
- **[overview.md](.context/overview.md)** - Project purpose, structure, and development guidelines
- **[architecture.md](.context/architecture.md)** - Three-tier architecture, operations, adapters, function system
- **[versioning.md](.context/versioning.md)** - Commit-based versioning and path handling

**Python API:**
- **[python-api.md](.context/python-api.md)** - Complete Python API reference with examples
- **[python-bindings.md](.context/python-bindings.md)** - PyO3 integration and async bridge
- **[sync-api.md](.context/sync-api.md)** - Synchronous wrapper API for scripts and Jupyter notebooks

**Infrastructure:**
- **[indexing.md](.context/indexing.md)** - Row indexing system
- **[progress.md](.context/progress.md)** - Progress tracking for long-running operations
- **[logging.md](.context/logging.md)** - Logging configuration
- **[views.md](.context/views.md)** - Views system

**Development:**
- **[testing.md](.context/testing.md)** - Testing strategy and execution
- **[development.md](.context/development.md)** - Setup, build, and development workflow
- **[code-review.md](.context/code-review.md)** - Code review guidelines and checklists
- **[debt.md](.context/debt.md)** - Technical debt tracking and prioritization

**AI & Project Rules:**
- **[ai-rules.md](.context/ai-rules.md)** - Hard constraints for AI code generation (streaming, no unwrap, etc.)
- **[anti-patterns.md](.context/anti-patterns.md)** - What NOT to do with concrete examples
- **[glossary.md](.context/glossary.md)** - Domain terminology and definitions
- **[workflows.md](.context/workflows.md)** - Step-by-step procedures for common development tasks

**Architecture Decisions:**
- **[decisions/](.context/decisions/)** - Architecture Decision Records (ADRs)
  - [README.md](.context/decisions/README.md) - ADR template and index
  - [001-rust-core.md](.context/decisions/001-rust-core.md) - Rust core library decision
  - [002-datafusion-arrow.md](.context/decisions/002-datafusion-arrow.md) - DataFusion and Arrow choice
  - [003-streaming-only.md](.context/decisions/003-streaming-only.md) - Streaming execution mandate
  - [004-three-tier-architecture.md](.context/decisions/004-three-tier-architecture.md) - Three-tier design
  - [005-mutable-operations.md](.context/decisions/005-mutable-operations.md) - `&mut Self` return pattern
  - [006-lazy-evaluation.md](.context/decisions/006-lazy-evaluation.md) - Three-phase operation pattern
  - [007-no-unwrap.md](.context/decisions/007-no-unwrap.md) - No `.unwrap()` allowed
  - [008-no-mod-rs.md](.context/decisions/008-no-mod-rs.md) - Named module files convention

**AI Prompt Templates:**
- **[prompts/](.context/prompts/)** - Task-specific templates for AI-assisted development
  - [new-feature.md](.context/prompts/new-feature.md) - Template for adding new features
  - [add-operation.md](.context/prompts/add-operation.md) - Template for adding operations (most common)
  - [add-python-binding.md](.context/prompts/add-python-binding.md) - Template for Python bindings
  - [fix-bug.md](.context/prompts/fix-bug.md) - Template for bug fixing
  - [performance-review.md](.context/prompts/performance-review.md) - Template for performance optimization

**System Boundaries & Constraints:**
- **[boundaries.md](.context/boundaries.md)** - System boundaries and integration points
- **[errors.md](.context/errors.md)** - Error handling patterns and Python error mapping
- **[performance.md](.context/performance.md)** - Performance characteristics and benchmarks
- **[dependencies.md](.context/dependencies.md)** - External dependencies and version management

## End User Documentation

The published end-user facing documentation is manged in the `docs` directory.

## Key Principles

- **Always start with Rust**, then write Python bindings
- **All Operations mutate in place** - modification methods return `&mut self`
- **Lazy evaluation** - queries execute on demand, not when operations are recorded
- **Streaming execution** - use `execute_stream()`, never `collect()` for memory efficiency
- **Three phases** - operations validate, reconfigure state, then apply to DataFrames
- **E2E Python tests** - test the Python binding, not underlying Rust logic

See [.context/development.md](.context/development.md) for full development workflow.

## Performance Guidelines

**Memory-Efficient Data Processing:**

Bundlebase uses streaming execution throughout to handle datasets larger than RAM:

- ✅ **Python**: Use `to_pandas()` / `to_polars()` - they stream internally (constant memory)
- ✅ **Python**: Use `stream_batches()` for custom incremental processing
- ✅ **Rust**: Always use `execute_stream()` for query execution
- ❌ **Rust**: Never use `collect()` - materializes entire dataset in memory (3x size)
- ❌ **Python**: Avoid `as_pyarrow()` for large datasets - use `stream_batches()` instead

**See:**
- [.context/architecture.md](.context/architecture.md#streaming-execution-architecture) for architectural details
- [.context/python-api.md](.context/python-api.md#streaming-api-for-large-datasets) for Python streaming API

### Do Not
- Generate code without reading relevant context files first
- Create new architectural patterns without documentation
- Override established conventions without clear rationale

### When Making Changes
- Update relevant `.context/` files when making architectural decisions
- Document trade-offs and rationale in "Decision History" sections
- Keep code examples in documentation current and functional