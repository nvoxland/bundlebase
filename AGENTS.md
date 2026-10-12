# Bundlebase Project Overview

Bundlebase is like Docker, but for data. It allows you to bundle up existing data into a standard interface which can be queries with SQL and interacted with via Python.

## Documentation Index

For detailed documentation, see the modular guides in the `.context/` directory:

**Start Here:**
- **[README.md](.context/README.md)** - Project overview, navigation hub, and documentation standards

**Core Architecture:**
- **[overview.md](.context/overview.md)** - Project purpose, structure, and development guidelines
- **[architecture.md](.context/architecture.md)** - Three-tier architecture, operations, adapters, function system, CLI modes (REPL/Flight/MCP)
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
- **[writing-style.md](.context/writing-style.md)** - Writing style for blog posts and end-user docs (voice, structure, do/don't, avoiding AI tropes)

**Architecture Decisions:**
- **[decisions/](.context/decisions/)** - Architecture Decision Records (ADRs)
  - [README.md](.context/decisions/README.md) - ADR template and index
  - [001-rust-core.md](.context/decisions/001-rust-core.md) - Rust core library decision
  - [002-datafusion-arrow.md](.context/decisions/002-datafusion-arrow.md) - DataFusion and Arrow choice
  - [003-streaming-only.md](.context/decisions/003-streaming-only.md) - Streaming execution mandate
  - [004-three-tier-architecture.md](.context/decisions/004-three-tier-architecture.md) - Three-tier design
  - [005-mutable-operations.md](.context/decisions/005-mutable-operations.md) - Interior-mutability operation pattern (`&self -> Result<&Self>`)
  - [006-lazy-evaluation.md](.context/decisions/006-lazy-evaluation.md) - Lazy evaluation and the operation lifecycle
  - [007-no-unwrap.md](.context/decisions/007-no-unwrap.md) - No `.unwrap()` allowed
  - [008-no-mod-rs.md](.context/decisions/008-no-mod-rs.md) - Named module files convention
  - [009-block-level-edit-representation.md](.context/decisions/009-block-level-edit-representation.md) - Tombstones/overlays as the sole DELETE/UPDATE representation

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
- **All Operations mutate in place** - modification methods take `&self` and return `Result<&Self>`, mutating shared state via interior mutability (`Arc<RwLock<...>>`)
- **Lazy evaluation** - queries execute on demand, not when operations are recorded
- **Streaming execution** - use `execute_stream()`, never `collect()` for memory efficiency
- **Operation lifecycle** - operations `check()` (validate), `apply()` (update bundle state), then `apply_dataframe()` (transform the lazy DataFrame)
- **Tests live in Rust** - most test coverage (including e2e) belongs in `rust/bundlebase/tests/`. Python tests exist only to verify the Python binding itself and should stay thin and high-level — they do NOT re-test underlying Rust logic.

See [.context/development.md](.context/development.md) for full development workflow.

## Build Performance

Compiling bundlebase takes 1-3 minutes per `cargo test --profile test` run.
Plan edits to minimize compile cycles — batch related changes together, fix
all known issues before rebuilding, and use targeted crate builds (`-p
bundlebase-command`) when possible to avoid full rebuilds.

## Build Tools

**CRITICAL: Always use the correct build script for Python package builds**

- ✅ **Python package**: Use `./scripts/maturin-dev.sh` (NOT `maturin develop`)
- ✅ **Rust only**: Use `cargo test --no-run --profile test`
- ❌ **Never**: Use `maturin develop` directly (causes full rebuilds)
- ❌ **Never**: Use `cargo build --all` for routine development (conflicts with maturin)

**Why?** Maturin and cargo use different feature flags (`extension-module`), causing full rebuilds when switching between them. The `maturin-dev.sh` script uses a separate target directory (`target/maturin/`) to prevent this.

**Quick Reference:**
```bash
# Python development (use this instead of maturin develop)
./scripts/maturin-dev.sh

# Rust development
cargo  test --no-run --profile test
cargo test --profile test

# Run Python tests (automatically uses same target-dir as maturin-dev.sh)
poetry run pytest python/tests/
```

**Note:** Python tests use `maturin_import_hook` for auto-rebuild. The test suite sets `CARGO_TARGET_DIR=target/maturin` in `conftest.py` to match `maturin-dev.sh`, preventing rebuilds.

See [scripts/README.md](scripts/README.md) for detailed explanation.

## Performance Guidelines

**Memory-Efficient Data Processing:**

Bundlebase uses streaming execution throughout to handle datasets larger than RAM:

- ✅ **Python**: Use `stream_batches()` for bounded-memory incremental processing
- ⚠️ **Python**: `to_pandas()` / `to_polars()` stream batches internally but accumulate them all to build the final DataFrame (~2x dataset size at peak) - fine when the result fits in RAM; use `stream_batches()` otherwise
- ✅ **Rust**: Always use `execute_stream()` for query execution
- ❌ **Rust**: Never use `collect()` - materializes entire dataset in memory (3x size)
- ❌ **Python**: Avoid `as_pyarrow()` for large datasets (it uses DataFusion `collect()`) - use `stream_batches()` instead

**See:**
- [.context/architecture.md](.context/architecture.md#streaming-execution-architecture) for architectural details
- [.context/python-api.md](.context/python-api.md#streaming-api-for-large-datasets) for Python streaming API

### Do Not
- Generate code without reading relevant context files first
- Create new architectural patterns without documentation
- Override established conventions without clear rationale
- Update old blog posts when referenced functionality changes — they are point-in-time artifacts

### When Making Changes
- Update relevant `.context/` files when making architectural decisions
- Document trade-offs and rationale in "Decision History" sections
- Keep code examples in documentation current and functional

## Non-Interactive Shell Commands

**ALWAYS use non-interactive flags** with file operations to avoid hanging on confirmation prompts.

Shell commands like `cp`, `mv`, and `rm` may be aliased to include `-i` (interactive) mode on some systems, causing the agent to hang indefinitely waiting for y/n input.

**Use these forms instead:**
```bash
# Force overwrite without prompting
cp -f source dest           # NOT: cp source dest
mv -f source dest           # NOT: mv source dest
rm -f file                  # NOT: rm file

# For recursive operations
rm -rf directory            # NOT: rm -r directory
cp -rf source dest          # NOT: cp -r source dest
```

**Other commands that may prompt:**
- `scp` - use `-o BatchMode=yes` for non-interactive
- `ssh` - use `-o BatchMode=yes` to fail instead of prompting
- `apt-get` - use `-y` flag
- `brew` - use `HOMEBREW_NO_AUTO_UPDATE=1` env var

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:7510c1e2 -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

**Architecture in one line:** issues live in a local Dolt DB; sync uses `refs/dolt/data` on your git remote; `.beads/issues.jsonl` is a passive export. See https://github.com/gastownhall/beads/blob/main/docs/SYNC_CONCEPTS.md for details and anti-patterns.

## Session Completion

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds
<!-- END BEADS INTEGRATION -->
