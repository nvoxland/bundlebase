# Bundlebase Substrate

**Documentation as Code as Context**

This `.context/` directory contains comprehensive documentation designed for both human developers and AI tools. The documentation lives in Git alongside code, updating through normal PR workflows.

## Project Overview

**Bundlebase** is like Docker, but for data. It's a high-performance data processing library written in Rust with Python bindings, providing a flexible framework for loading, transforming, and querying data from various sources.

**Current Version**: 0.2.1 (Alpha)
**License**: Apache 2.0
**Homepage**: https://github.com/nvoxland/bundlebase

### Key Features

- **Multiple Formats**: Support for Parquet, CSV, JSON, and custom data sources
- **Version Control**: Built-in commit system for data pipeline versioning
- **Python Native**: Seamless async/sync Python API with type hints
- **High Performance**: Rust-powered core with streaming execution for datasets larger than RAM
- **Fluent API**: Chain operations with intuitive, readable syntax
- **SQL Integration**: Built on DataFusion for full SQL query support

### Quick Example

```python
import bundlebase

# Async API - chain operations fluently
c = await (bundlebase.create()
    .attach("data.parquet")
    .filter("age >= 18")
    .remove_column("ssn")
    .rename_column("fname", "first_name"))

# Stream to pandas (constant memory, even for huge files)
df = await c.to_pandas()

# Version control for data pipelines
await c.commit("Cleaned customer data")
```

## Documentation Navigation

### Start Here

- **This file (README.md)** - Project overview and navigation hub

### Core Architecture & Concepts

- **[overview.md](overview.md)** - Project purpose, structure, development guidelines, and quick start
- **[architecture.md](architecture.md)** - Three-tier architecture (trait, read-only, builder), operation pipeline, adapters, function system
- **[versioning.md](versioning.md)** - Commit-based versioning, manifest system, path handling, and 'from' chains

### Python API & Bindings

- **[python-api.md](python-api.md)** - Complete Python API reference with examples, mutable operations pattern
- **[python-bindings.md](python-bindings.md)** - PyO3 integration details, async bridge between Rust and Python
- **[sync-api.md](sync-api.md)** - Synchronous wrapper API for scripts and Jupyter notebooks

### Infrastructure & Systems

- **[indexing.md](indexing.md)** - Row indexing system for fast lookups and efficient data access
- **[progress.md](progress.md)** - Progress tracking system for long-running operations
- **[logging.md](logging.md)** - Logging configuration, levels, and integration
- **[views.md](views.md)** - Views system, named forks, and view inheritance

### Development & Testing

- **[testing.md](testing.md)** - Testing strategy, organization, and execution (Rust unit → integration → Python E2E)
- **[development.md](development.md)** - Setup instructions, build workflow, and development process
- **[code-review.md](code-review.md)** - Code review guidelines, checklists, and standards
- **[debt.md](debt.md)** - Technical debt tracking, prioritization, and resolution

### AI & Project Rules

- **[ai-rules.md](ai-rules.md)** - Hard constraints for AI code generation (CRITICAL: streaming, no unwrap, etc.)
- **[anti-patterns.md](anti-patterns.md)** - What NOT to do, with concrete examples
- **[glossary.md](glossary.md)** - Domain terminology and definitions
- **[workflows.md](workflows.md)** - Step-by-step procedures for common development tasks

### Architecture Decisions & Guidelines

- **[decisions/](decisions/)** - Architecture Decision Records (ADRs) documenting major architectural choices
  - [README.md](decisions/README.md) - ADR template, guidelines, and index
  - [001-rust-core.md](decisions/001-rust-core.md) - Why Rust core with Python bindings
  - [002-datafusion-arrow.md](decisions/002-datafusion-arrow.md) - DataFusion v51 and Apache Arrow v57
  - [003-streaming-only.md](decisions/003-streaming-only.md) - Mandate for streaming execution
  - [004-three-tier-architecture.md](decisions/004-three-tier-architecture.md) - Bundle trait, Bundle, BundleBuilder design
  - [005-mutable-operations.md](decisions/005-mutable-operations.md) - Why operations return `&mut Self`
  - [006-lazy-evaluation.md](decisions/006-lazy-evaluation.md) - Three-phase operation pattern (check → reconfigure → apply)
  - [007-no-unwrap.md](decisions/007-no-unwrap.md) - Compiler-enforced ban on `.unwrap()`
  - [008-no-mod-rs.md](decisions/008-no-mod-rs.md) - Named module files convention

### AI Prompt Templates

- **[prompts/](prompts/)** - Task-specific templates for AI-assisted development
  - [README.md](prompts/README.md) - How to use prompt templates
  - [new-feature.md](prompts/new-feature.md) - Template for adding completely new features
  - [add-operation.md](prompts/add-operation.md) - Template for adding transformation operations (most common)
  - [add-python-binding.md](prompts/add-python-binding.md) - Template for wrapping Rust code for Python
  - [fix-bug.md](prompts/fix-bug.md) - Template for systematic bug investigation and fixing
  - [performance-review.md](prompts/performance-review.md) - Template for performance optimization

### System Boundaries & Constraints

- **[boundaries.md](boundaries.md)** - System boundaries, integration points, and trust boundaries
- **[errors.md](errors.md)** - Error handling patterns, error types, and Python error mapping
- **[performance.md](performance.md)** - Performance characteristics, benchmarks, and optimization patterns
- **[dependencies.md](dependencies.md)** - External dependencies, versions, and dependency management

## Technology Stack

### Core Technologies

| Technology | Version | Purpose |
|------------|---------|---------|
| **Rust** | 2021 Edition | Core library implementation, performance-critical code |
| **DataFusion** | v51 | SQL query engine and execution framework |
| **Apache Arrow** | v57 | Columnar data format, zero-copy data sharing |
| **PyO3** | v0.23 | Python bindings and FFI layer |
| **Tokio** | v1 | Async runtime for Rust operations |

### Python Ecosystem

| Dependency | Version | Purpose |
|------------|---------|---------|
| **Python** | 3.13+ | Target runtime environment |
| **PyArrow** | 14.0.0+ | Arrow FFI for Python-Rust data transfer |
| **Pandas** | 2.0.0+ | DataFrame conversion target |
| **Polars** | 0.20.0+ | Alternative DataFrame library (optional) |

### Build & Development

- **Maturin** (1.10.2+) - Python wheel builder for Rust extensions
- **Poetry** - Python dependency management
- **pytest** - Python testing framework
- **cargo** - Rust build tool and package manager

## Architectural Foundations

### Three-Tier Design

Bundlebase uses a **three-tier architecture** to manage mutability and data safety:

1. **Bundlebase Trait** - Common interface shared by all implementations
2. **BundlebaseBuilder** - Mutable container for building and transforming data
3. **Bundlebase (Read-Only)** - Immutable snapshot loaded from commits

**Key Principle**: Operations mutate in place (`&mut self`), enabling fluent chaining while maintaining clear ownership semantics.

### Streaming Execution

**CRITICAL**: Bundlebase is designed for **streaming execution** to handle datasets larger than RAM:

- ✅ **Always** use `execute_stream()` - processes data in batches (constant memory)
- ❌ **Never** use `collect()` - materializes entire dataset (10GB file → 30GB RAM)
- ✅ Python's `to_pandas()` and `to_polars()` stream internally
- ❌ Avoid `as_pyarrow()` for large datasets

This is a foundational constraint documented in detail in [ai-rules.md](ai-rules.md) and [anti-patterns.md](anti-patterns.md).

### Lazy Evaluation

Operations are **recorded** when called but only **executed** during query time:

```python
c = c.filter("age >= 18")    # Records FilterBlock operation
c = c.select(["name"])       # Records SelectColumns operation
df = await c.to_pandas()     # NOW executes both operations
```

This enables query optimization and deferred execution.

## Project Maturity

**Current State**: Alpha (v0.2.1)

**Major Features Implemented**:
- ✅ Three-tier architecture with proper immutability semantics
- ✅ Commit-based versioning with manifest history
- ✅ Streaming execution with constant memory usage
- ✅ Python async/await and synchronous APIs
- ✅ Multi-format support (CSV, JSON, Parquet, custom functions)
- ✅ Row indexing for fast lookups
- ✅ SQL query integration via DataFusion

**Known Limitations**:
- ⚠️ Limited input validation (some operations don't validate preconditions)
- ⚠️ Schema mismatch handling in multi-source UNIONs could be improved
- ⚠️ Row indexing is lazy (built on first use, not pre-computed)

**Development Philosophy**:
- Project is **not launched** - no backward compatibility constraints
- Break freely when improvements warrant it
- Prioritize simplicity over premature flexibility
- Trust internal code, validate at system boundaries only

## Documentation Standards

### File Structure

Each domain documentation file follows a consistent structure:

1. **Purpose & Overview** - What this domain covers and why it matters
2. **Core Concepts** - Key abstractions and how they relate
3. **Patterns & Examples** - Concrete code examples demonstrating proper usage
4. **Decision History** - Why choices were made, trade-offs considered
5. **References** - Links to related documentation and external resources

### Content Guidelines

- **Code Examples**: All examples should be **runnable** and **up-to-date** with current API
- **Decision Rationale**: Document **why**, not just **what** - capture trade-offs and alternatives considered
- **Cross-References**: Link to related documents using relative paths (e.g., `[Architecture](01-ARCHITECTURE.md)`)
- **Diagrams**: Use Mermaid for complex relationships and flows
- **Updates**: When making architectural changes, update relevant `.context/` files in the same PR

### Markdown Conventions

- Use `**bold**` for emphasis and important terms
- Use `code` for inline code, file names, and technical terms
- Use ` ```language ` fenced blocks for multi-line code examples
- Use `- ✅` for recommended patterns, `- ❌` for anti-patterns
- Use `>` blockquotes for important warnings or notes
- Use tables for structured comparison data

## Using This Documentation

### For Human Developers

**New to the project?**
1. Start with [overview.md](overview.md) for project structure
2. Read [architecture.md](architecture.md) to understand core abstractions
3. Follow [development.md](development.md) for setup
4. Review [ai-rules.md](ai-rules.md) for critical constraints

**Adding a feature?**
1. Check [prompts/](prompts/) for task-specific templates (new-feature, add-operation, etc.)
2. Consult [workflows.md](workflows.md) for step-by-step process
3. Review relevant [decisions/](decisions/) to understand architectural rationale
4. Check [glossary.md](glossary.md) for terminology consistency
5. Avoid patterns in [anti-patterns.md](anti-patterns.md)
6. Update relevant documentation files when adding new patterns

### For AI Tools

**Before generating code**:
1. Read [ai-rules.md](ai-rules.md) - contains hard constraints (streaming, no unwrap, etc.)
2. Use [prompts/](prompts/) templates for common tasks (add-operation, new-feature, etc.)
3. Review relevant [decisions/](decisions/) to understand architectural choices
4. Consult relevant domain files (architecture, API, testing)
5. Check [glossary.md](glossary.md) for consistent terminology
6. Review [anti-patterns.md](anti-patterns.md) to avoid common mistakes

**When uncertain**:
- Reference existing patterns in codebase
- Use the appropriate prompt template as a guide
- Prefer simplicity over premature abstraction
- Ask for clarification rather than guessing

## Extending This Documentation

### Adding a New Domain File

1. Create file in `.context/` directory with descriptive name
2. Follow the standard file structure (see Documentation Standards above)
3. Add entry to this README.md navigation section
4. Update CLAUDE.md with reference to new file
5. Include practical code examples adapted to bundlebase's tech stack

### Maintaining Documentation Quality

- **Keep it current**: Update docs in same PR as code changes
- **Be specific**: "Use `execute_stream()`" not "use streaming"
- **Show, don't tell**: Include code examples for complex patterns
- **Explain decisions**: Document why a choice was made, not just what it is
- **Cross-reference**: Link related documents to create a navigable web

## Quick Access Commands

### Common Development Tasks

```bash
# Setup environment
poetry install && ./scripts/maturin-dev.sh

# Run tests
cargo test              # Rust tests
poetry run pytest       # Python tests

# Build for development
./scripts/maturin-dev.sh         # Build and install in dev mode

# Check code
cargo clippy            # Rust linting
mypy python/            # Python type checking
```

### Reading Documentation

```bash
# View README (this file)
cat .context/README.md

# View AI rules (critical constraints)
cat .context/ai-rules.md

# View all .context files
ls -1 .context/
```

### Searching Documentation

```bash
# Find mentions of streaming execution
grep -r "streaming" .context/

# Find all examples of a specific operation
grep -r "execute_stream" .context/

# Search for specific term in glossary
grep -i "bundle" .context/glossary.md
```

## Support

- **Documentation**: https://nvoxland.github.io/bundlebase/
- **Issues**: https://github.com/nvoxland/bundlebase/issues
- **Repository**: https://github.com/nvoxland/bundlebase

---

**Last Updated**: January 2026
**Methodology**: [.context approach](https://github.com/andrefigueira/.context/)
