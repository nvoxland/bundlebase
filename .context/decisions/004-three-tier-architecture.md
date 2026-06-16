# ADR-004: Three-Tier Architecture

**Status:** Accepted — Amended June 2026 (type names and mutability mechanism updated to match implementation; crate layout documented)

**Date:** December 2024 (documented retroactively); amended June 2026

## Context

Bundlebase needed clear separation between:
- **Committed bundles** (loaded from manifests) that can be safely shared
- **Working bundles** (uncommitted) that accumulate operations before committing
- **Common interface** shared by both for reading/querying operations

### The Problem

Without clear boundaries:
- Risk of accidentally recording operations on committed bundles
- Confusion about when operations are recorded vs executed
- Unclear ownership and sharing semantics

### Alternatives Considered

**Option 1: Single mutable type** (like Pandas DataFrame)
- Pros: Simple, familiar API, easy to understand
- Cons: Can accidentally mutate committed data, unclear clone semantics

**Option 2: Builder pattern** (separate Builder type)
- Pros: Clear separation, familiar pattern
- Cons: Awkward to convert between types, duplication of read methods

**Option 3: Three-tier with shared trait** (chosen)
- Pros: Shared read/query interface, clear semantics for committed vs working state
- Cons: More complex type system, learning curve

## Decision (as amended)

**Implement three tiers: the `BundleFacade` trait (common interface), `Bundle` (committed/persistent), and `BundleBuilder` (working copy with change tracking).**

### Architecture

```
┌─────────────────────────────────────┐
│         BundleFacade Trait          │
│  (Common read/query interface)      │
└─────────────────────────────────────┘
              ▲         ▲
              │         │
     ┌────────┴──┐  ┌──┴────────────┐
     │  Bundle   │  │ BundleBuilder │
     │(committed)│  │ (working copy)│
     └───────────┘  └───────────────┘
```

**BundleFacade trait** (rust/bundlebase/src/bundle/facade.rs):
- Common interface: `schema()`, `dataframe()`, `query()`, `num_rows()`, `operations()`, `views()`, `extend()`, `export_tar()`, etc.
- Implemented by both `Bundle` and `BundleBuilder`
- Used by the catalog/schema providers and CLI so they work with either type

**Bundle** (rust/bundlebase/src/bundle.rs):
- Represents a bundle loaded from persistent storage (committed manifests)
- **Not a frozen immutable struct**: all mutable state lives in `Arc<RwLock<T>>` fields (~25 of them), enabling thread-safe interior mutability
- Mutation only happens through the `pub(crate) apply_operation(&self, op)` path — external users cannot record operations on a bare `Bundle`
- `Clone` shares all `Arc` state: clones see each other's mutations (intentional, used by schema providers)
- Extended via `.extend()` → creates a `BundleBuilder`

**BundleBuilder** (rust/bundlebase/src/bundle/builder.rs):
- Wraps an `Arc<Bundle>` plus change tracking: `in_progress_change`, `status` (uncommitted `BundleChange`s), pending deletes/updates
- Modification methods take `&self` and return `Result<&Self>` (interior mutability — see [ADR-005](005-mutable-operations.md))
- Convenience methods (`attach()`, `filter()`, `drop_column()`, ...) live on the `BundleBuilderExt` trait in the `bundlebase-command` crate (builder_ext.rs); they run commands through `run_command`/`do_change` for change tracking and rollback
- `commit(message)` persists accumulated changes as a new manifest version
- `reset()` / `undo()` discard or roll back uncommitted changes

### Crate Layout

The "tiers" sit inside a 12-crate workspace (`rust/`):

| Crate | Role |
|-------|------|
| `bundlebase` | Core: Bundle, BundleBuilder, operations, packs, catalog glue |
| `bundlebase-command` | Command layer: SQL-ish command parsing (pest grammar), `BundleBuilderExt` |
| `bundlebase-cli` | CLI binary: repl/query/extend/server/mcp subcommands |
| `bundlebase-python` | PyO3 bindings (the only crate with PyO3 code) |
| `bundlebase-common` | Shared types: errors, RowId, ObjectId, Platform, arrow type parsing |
| `bundlebase-data` | Reader plugins (CSV/TSV/JSONL/Parquet), DataReaderFactory, page maps |
| `bundlebase-io` | IO abstraction over local/object storage |
| `bundlebase-catalog` | DataFusion catalog/schema providers (`bundle_info.*`) |
| `bundlebase-connector` | Connector definitions and validation |
| `bundlebase-index` | BTree row indexes |
| `bundlebase-udf` | User-defined function runtimes (python/lib/java/docker/ipc) and bridges |
| `bundlebase-report` | Stored report generation |

## Consequences

### Positive

- **Clear semantics**: `Bundle` = committed state; `BundleBuilder` = uncommitted changes with status/undo/commit
- **Safe sharing**: Both types are `Arc`-shareable across threads, async tasks, server sessions, and Python
- **Clone efficiency**: Arc-based state sharing makes cloning cheap
- **Generic programming**: Functions can take `&dyn BundleFacade` to work with either type
- **Explicit conversions**: `.extend()` creates a working copy from a committed bundle with clear intent

### Negative

- **Mutation not compiler-enforced**: Because both types use interior mutability, immutability of `Bundle` is enforced by API visibility (`pub(crate) apply_operation`) and the operation/commit flow, not by `&self` vs `&mut self`
- **Learning curve**: Users must understand the trait + two types
- **API complexity**: More types in the API surface than single-type approach

### Neutral

- **Code duplication**: Some methods appear on both types (via the trait)
- **Method availability**: Read methods on both types; change-tracked write methods only on `BundleBuilder`

## Usage Patterns

### Loading and Querying (committed)

```rust
// Load committed bundle
let bundle: Arc<Bundle> = Bundle::open("file:///path/to/bundle", None).await?;

// Read operations work
let schema = bundle.schema().await?;
let results = bundle.query("SELECT * FROM bundle", vec![], None).await?;

// There is no `bundle.filter(...)` — recording operations requires a BundleBuilder
```

### Building and Modifying (working copy)

```rust
use bundlebase_command::BundleBuilderExt;

let builder = BundleBuilder::create("file:///path/to/bundle", None).await?;
builder.attach("data.parquet", None).await?
    .filter("SELECT * FROM bundle WHERE active = true", vec![]).await?
    .drop_column("temp").await?
    .commit("Initial load").await?;
```

### Extending (committed → working copy)

```rust
let base = Bundle::open("file:///base", None).await?;
let extended = base.extend(Some("/extended")).await?; // Arc<BundleBuilder>
extended.attach("more_data.parquet", None).await?;
extended.commit("Added data").await?;
```

## Related Decisions

- [ADR-005](005-mutable-operations.md) - Interior mutability (`&self -> Result<&Self>`)
- [ADR-006](006-lazy-evaluation.md) - Operations recorded, DataFrame rebuilt lazily
- See [architecture.md](../architecture.md) for detailed implementation

## Python Mapping

Python doesn't have ownership, so we map:
- `Bundle` (Rust) → `PyBundle` / `Bundle` (Python, read/query only)
- `BundleBuilder` (Rust) → `PyBundleBuilder` / `BundleBuilder` (Python, mutation methods)

```python
# Committed (loaded)
bundle = await bundlebase.open("/path")
df = await bundle.to_pandas() # OK
# bundle.filter(...) # AttributeError — not a builder

# Working copy (created or extended)
builder = await bundlebase.create("/path").attach("data.parquet")
await builder.commit("Done")
```

## Decision History

- **Dec 2024**: Recorded as "Bundlebase trait / Bundle (immutable, `&self` only) / BundleBuilder (`&mut self`)" with a shared `BundleState` struct.
- **Amended June 2026**: The trait is named `BundleFacade`; there is no `BundleState` type; both `Bundle` and `BundleBuilder` use interior mutability and `&self` methods (see ADR-005). The three-tier separation itself (trait + committed type + working type) stands and is implemented.

## References

- [architecture.md](../architecture.md#three-tier-architecture)
- [python-bindings.md](../python-bindings.md) - Python FFI implementation
