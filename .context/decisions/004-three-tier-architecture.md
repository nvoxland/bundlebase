# ADR-004: Three-Tier Architecture

**Status:** Accepted

**Date:** December 2024 (documented retroactively)

## Context

Bundlebase needed clear separation between:
- **Immutable snapshots** (committed containers) that can be safely shared and cloned
- **Mutable builders** (uncommitted containers) that accumulate operations before committing
- **Common interface** shared by both for reading/querying operations

### The Problem

Without clear boundaries:
- Risk of accidentally mutating committed containers
- Confusion about when operations are recorded vs executed
- Unclear ownership and sharing semantics
- Type system doesn't enforce immutability

### Alternatives Considered

**Option 1: Single mutable type** (like Pandas DataFrame)
- Pros: Simple, familiar API, easy to understand
- Cons: Can accidentally mutate committed data, unclear clone semantics

**Option 2: Builder pattern** (separate Builder type)
- Pros: Clear separation, familiar pattern
- Cons: Awkward to convert between types, duplication of read methods

**Option 3: Three-tier with shared trait** (chosen)
- Pros: Type-enforced immutability, shared interface, clear semantics
- Cons: More complex type system, learning curve

## Decision

**Implement three-tier architecture: Bundle trait, Bundle (read-only), BundleBuilder (mutable).**

### Architecture

```
┌─────────────────────────────────────┐
│         Bundlebase Trait            │
│  (Common read/query interface)      │
└─────────────────────────────────────┘
              ▲         ▲
              │         │
     ┌────────┴──┐  ┌──┴──────────┐
     │  Bundle   │  │ BundleBuilder │
     │(immutable)│  │   (mutable)   │
     └───────────┘  └───────────────┘
```

**Bundlebase Trait**:
- Common interface for schema(), query(), to_pandas(), etc.
- Implemented by both Bundle and BundleBuilder
- Contains all read/query operations

**Bundle** (read-only):
- Represents committed snapshot
- Loaded from manifest files
- Cannot be modified (`&self` methods only)
- Cheap to clone (Arc-based sharing)
- Can be extended via `.extend()` → creates new BundleBuilder

**BundleBuilder** (mutable):
- Contains uncommitted operations
- All mutation methods take `&mut self` and return `&mut Self`
- Methods: `attach()`, `filter()`, `remove_column()`, `commit()`
- Wraps a base Bundle + list of new operations

### Implementation Details

```rust
// Trait (common interface)
pub trait Bundlebase {
    fn schema(&self) -> &Schema;
    async fn to_pandas(&self) -> Result<DataFrame>;
    // ... read operations
}

// Read-only (immutable)
pub struct Bundle {
    state: Arc<BundleState>, // Shared immutable state
    // No mutation methods
}

// Mutable (builder)
pub struct BundleBuilder {
    base: Bundle,              // Base committed state
    operations: Vec<Operation>, // New operations
}

impl BundleBuilder {
    pub fn filter(&mut self, expr: &str) -> &mut Self {
        self.operations.push(Operation::Filter(expr.to_string()));
        self // Chainable
    }
}
```

## Consequences

### Positive

- **Type safety**: Rust type system enforces immutability (can't call `.filter()` on Bundle)
- **Clear semantics**: Obvious when data can/cannot be modified
- **Safe sharing**: Bundle can be cloned and shared across threads without locks
- **Clone efficiency**: Arc-based state sharing makes cloning cheap
- **Familiar pattern**: Similar to Rust's `String` vs `&str`, `Vec<T>` vs `&[T]`
- **Explicit conversions**: `.extend()` creates mutable from immutable with clear intent

### Negative

- **Learning curve**: Users must understand three types and when to use each
- **API complexity**: More types in the API surface than single-type approach
- **Conversion overhead**: Need to explicitly convert between types
- **Documentation burden**: Must explain the architecture to new users
- **Python mapping**: Harder to map to Python (Python doesn't have ownership)

### Neutral

- **Code duplication**: Some methods appear on both types (via trait)
- **Method availability**: Read methods on both types, write methods only on BundleBuilder
- **Generic programming**: Can write functions generic over `T: Bundlebase`

## Usage Patterns

### Loading and Querying (immutable)

```rust
// Load committed container (immutable)
let bundle = Bundle::open("/path").await?;

// Read operations work
let schema = bundle.schema();
let df = bundle.to_pandas().await?;

// Mutation doesn't compile
bundle.filter("age > 18"); // ❌ Compile error
```

### Building and Modifying (mutable)

```rust
// Create mutable container
let mut builder = BundleBuilder::create("/path").await?;

// Chain mutations
builder
    .attach("data.parquet")?
    .filter("active = true")?
    .remove_column("temp")?;

// Commit creates immutable snapshot
builder.commit("Initial load").await?;
```

### Extending (immutable → mutable)

```rust
// Load committed
let base = Bundle::open("/base").await?;

// Extend to new mutable container
let mut extended = base.extend("/extended").await?;
extended.attach("more_data.parquet")?;
extended.commit("Added data").await?;
```

## Related Decisions

- [ADR-005](005-mutable-operations.md) - Mutable operations return `&mut Self` for chaining
- [ADR-006](006-lazy-evaluation.md) - Operations recorded on BundleBuilder, executed later
- See [architecture.md](../architecture.md) for detailed implementation

## Python Mapping

Python doesn't have ownership, so we map:
- `Bundle` (Rust) → `Bundle` (Python read-only wrapper)
- `BundleBuilder` (Rust) → `BundleBuilder` (Python mutable wrapper)

Python users see two classes but don't need to understand Rust ownership:

```python
# Immutable (loaded)
bundle = await bundlebase.open("/path")
df = await bundle.to_pandas() # OK
# bundle.filter(...) # Would error

# Mutable (created)
builder = await bundlebase.create("/path")
builder.attach("data.parquet") # OK
await builder.commit("Done")
```

## References

- [architecture.md](../architecture.md#three-tier-architecture)
- [python-bindings.md](../python-bindings.md) - Python FFI implementation
