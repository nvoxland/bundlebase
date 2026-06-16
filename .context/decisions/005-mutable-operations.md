# ADR-005: Modification Methods Use Interior Mutability (`&self -> Result<&Self>`)

**Status:** Accepted — Amended June 2026 (implementation diverged from the originally recorded `&mut Self` pattern)

**Date:** December 2024 (documented retroactively); amended June 2026

## Context

Operations like `filter()`, `attach()`, and `drop_column()` modify the bundle. The question was how these methods should interact with Rust's ownership system and how to enable method chaining.

### Alternatives Considered

**Option 1: Return `Self` (move ownership)**
```rust
pub fn filter(mut self, expr: &str) -> Self {
    // record operation
    self
}
```
- Pros: Chainable, familiar from Iterator methods
- Cons: Moves ownership each time (can't reuse variable), incompatible with shared access

**Option 2: Return `()` (in-place mutation)**
- Pros: Clear mutation, efficient (no moves)
- Cons: **Cannot chain operations** (biggest drawback)

**Option 3: Return `&mut Self`** (originally chosen)
```rust
pub fn filter(&mut self, expr: &str) -> &mut Self { ... }
```
- Pros: Chainable, clear mutation (`&mut`), efficient
- Cons: Requires exclusive access — incompatible with `Arc`-shared state

**Option 4: Interior mutability, return `Result<&Self>`** (current implementation)
```rust
pub async fn filter(&self, query: &str, params: Vec<ScalarValue>)
    -> Result<&Self, BundlebaseError> { ... }
```
- Pros: Chainable with `?`, works through `Arc<Bundle>` / `Arc<BundleBuilder>`, safe across async tasks and Python bindings
- Cons: Mutation is not visible in the signature; correctness depends on internal locking discipline

## Decision (as amended)

**All modification methods take `&self` and return `Result<&Self, BundlebaseError>`. Mutation happens through interior mutability.**

The originally recorded decision was `&mut self -> &mut Self`. That pattern was abandoned during implementation because:

- `Bundle` and `BundleBuilder` are shared via `Arc` across async tasks, the Flight SQL server, schema providers, and the Python bindings. You cannot obtain `&mut self` through an `Arc`.
- Async methods holding `&mut self` across `.await` points would serialize all access and fight the borrow checker.
- Python bindings hold long-lived references; interior mutability lets every clone see the same state.

### Implementation Pattern (actual code)

`Bundle` (rust/bundlebase/src/bundle.rs) holds its mutable state in `Arc<RwLock<T>>` fields (`parking_lot::RwLock`) and mutates via locks:

```rust
pub struct Bundle {
    name: Arc<RwLock<Option<String>>>,
    pub operations: Arc<RwLock<Vec<AnyOperation>>>,
    packs: Arc<RwLock<HashMap<ObjectId, Arc<Pack>>>>,
    // ... ~25 Arc<RwLock<...>> fields total
}

impl Bundle {
    /// Modifies this bundle with the given operation using interior mutability.
    pub(crate) async fn apply_operation(&self, op: AnyOperation) -> Result<(), BundlebaseError> {
        op.check(self).await?;
        op.apply(self).await?;
        self.update_version_for_op(&op);
        self.operations.write().push(op);
        self.dataframe.clear(); // invalidate cached DataFrame
        Ok(())
    }
}
```

`BundleBuilder` methods (rust/bundlebase/src/bundle/builder.rs and the `BundleBuilderExt` trait in rust/bundlebase-command/src/builder_ext.rs) follow the same convention:

```rust
// builder.rs
pub async fn commit(&self, message: &str) -> Result<&Self, BundlebaseError> { ... }
pub async fn reset(&self) -> Result<&Self, BundlebaseError> { ... }

// builder_ext.rs (BundleBuilderExt trait)
async fn attach(&self, path: &str, pack: Option<&str>) -> Result<&Self, BundlebaseError>;
async fn filter(&self, query: &str, params: Vec<ScalarValue>) -> Result<&Self, BundlebaseError>;
async fn rename_column(&self, old_name: &str, new_name: &str) -> Result<&Self, BundlebaseError>;
```

### Usage

```rust
// Fluent chaining through shared references
let builder = BundleBuilder::create("memory://work", None).await?;
builder.attach("data.parquet", None).await?
    .filter("SELECT * FROM bundle WHERE age >= 18", vec![]).await?
    .drop_column("ssn").await?
    .commit("Filter adults, drop PII").await?;
```

## Consequences

### Positive

- **Method chaining**: `Result<&Self>` + `?` enables fluent pipelines
- **Shared access**: Works through `Arc<Bundle>` / `Arc<BundleBuilder>` — required by async tasks, schema providers, Flight sessions, and PyO3 bindings
- **Cheap clones with shared state**: `Bundle::clone()` shares all `Arc<RwLock<...>>` fields; mutations in one clone are visible in all clones
- **Python compatibility**: Maps naturally to Python's fluent API (bindings just clone the Arc)

### Negative

- **Hidden mutation**: `&self` signatures don't advertise mutation; readers must know the type uses interior mutability
- **Locking discipline required**: Locks must not be held across `.await` points; `builder.rs` documents a lock acquisition order (bundle lock before `in_progress_change`) to prevent deadlocks
- **Runtime instead of compile-time safety**: The borrow checker no longer prevents concurrent mutation; `parking_lot::RwLock` does

### Neutral

- **Reference lifetime**: Returned `&Self` is tied to the receiver (usually fine)
- The `&mut Self` pattern described in the original version of this ADR does not exist anywhere in the codebase

## Python Mapping

PyO3 wrappers hold an `Arc`-shared builder and return clones to Python (Python can't hold Rust references). The Python layer additionally wraps mutation methods in an `OperationChain` so calls can be chained and awaited once (see `python/src/bundlebase/__init__.py` and `chain.py`).

```python
c = await bundlebase.create() \
    .attach("data.parquet") \
    .filter("age >= 18") \
    .drop_column("ssn")
```

## Related Decisions

- [ADR-004](004-three-tier-architecture.md) - Bundle/BundleBuilder/BundleFacade split
- [ADR-006](006-lazy-evaluation.md) - Operations recorded, DataFrame rebuilt on demand
- See [python-api.md](../python-api.md) for Python API patterns

## Decision History

- **Dec 2024**: Recorded as "all mutation methods take `&mut self` and return `&mut Self`".
- **Amended June 2026**: The implemented design uses interior mutability (`Arc<RwLock<...>>` fields, `&self -> Result<&Self>` methods). The `&mut Self` pattern was incompatible with sharing bundles across async tasks, server sessions, and Python bindings. This document now records the implemented pattern; the chaining goal of the original decision is preserved.
