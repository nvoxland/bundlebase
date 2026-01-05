# ADR-005: Mutable Operations Return &mut Self

**Status:** Accepted

**Date:** December 2024 (documented retroactively)

## Context

Operations like `filter()`, `attach()`, and `remove_column()` modify the container. The question was how these methods should interact with Rust's ownership system and how to enable method chaining.

### Alternatives Considered

**Option 1: Return `Self` (move ownership)**
```rust
pub fn filter(mut self, expr: &str) -> Self {
    self.operations.push(Operation::Filter(expr.to_string()));
    self
}
```
- Pros: Chainable, familiar from Iterator methods
- Cons: Moves ownership each time (can't reuse variable), incompatible with `&mut self` pattern

**Option 2: Return `()` (in-place mutation)**
```rust
pub fn filter(&mut self, expr: &str) {
    self.operations.push(Operation::Filter(expr.to_string()));
}
```
- Pros: Clear mutation, efficient (no moves)
- Cons: **Cannot chain operations** (biggest drawback)

**Option 3: Return `&mut Self`** (chosen)
```rust
pub fn filter(&mut self, expr: &str) -> &mut Self {
    self.operations.push(Operation::Filter(expr.to_string()));
    self
}
```
- Pros: Chainable, clear mutation (`&mut`), efficient, matches Python behavior
- Cons: Less common in Rust ecosystem, requires mutable reference throughout chain

## Decision

**All mutation operations on BundleBuilder return `&mut Self` for method chaining.**

### Implementation Pattern

```rust
impl BundleBuilder {
    pub fn attach(&mut self, path: &str) -> Result<&mut Self> {
        // Validation
        // Add operation
        self.operations.push(Operation::Attach(path.to_string()));
        Ok(self) // Return mutable reference for chaining
    }

    pub fn filter(&mut self, expr: &str) -> Result<&mut Self> {
        self.operations.push(Operation::Filter(expr.to_string()));
        Ok(self)
    }

    pub fn remove_column(&mut self, name: &str) -> Result<&mut Self> {
        self.operations.push(Operation::RemoveColumn(name.to_string()));
        Ok(self)
    }
}
```

### Usage

```rust
// Fluent chaining with &mut
let mut container = BundleBuilder::create("/path").await?;
container
    .attach("data.parquet")?
    .filter("age >= 18")?
    .remove_column("ssn")?
    .rename_column("name", "full_name")?;
```

## Consequences

### Positive

- **Method chaining**: Enables fluent API matching Python/Pandas style
- **Clear mutability**: The `&mut` makes it obvious that mutation is happening
- **Efficiency**: No ownership moves, references only
- **Python compatibility**: Maps naturally to Python's fluent API
- **Type safety**: Compiler enforces that you have mutable access
- **Consistent pattern**: All mutation methods follow same signature

### Negative

- **Non-idiomatic Rust**: Most Rust APIs use `self` ownership moves for builders
- **Mutable borrow rules**: Must maintain single mutable borrow throughout chain
- **Cannot branch**: Can't split the chain and apply different operations
  ```rust
  let filtered = container.filter("a")?; // Borrows container
  container.filter("b")?; // ❌ Already borrowed
  ```
- **Learning curve**: Developers used to standard Rust builder pattern need to adjust
- **Error handling**: With `?` operator, chain breaks on first error

### Neutral

- **Reference lifetime**: Returned reference tied to container lifetime (usually fine)
- **Clone requirement**: If need to branch, must clone first (Arc-based, cheap)

## Comparison with Standard Rust Patterns

### Iterator (move ownership)
```rust
vec.into_iter()
   .filter(|x| x > 0)
   .map(|x| x * 2)
   .collect()  // Ownership moved through chain
```

### Builder pattern (move ownership)
```rust
Builder::new()
    .width(800)
    .height(600)
    .build() // Ownership moved, consumes builder
```

### Bundlebase (mutable reference)
```rust
container
    .filter("a")?
    .select(["col"])? // &mut reference through chain
```

## Python Mapping

Python doesn't distinguish `&mut` from ownership, so the mapping is straightforward:

```python
# Python sees fluent chaining
c = await bundlebase.create()
c.attach("data.parquet")\
  .filter("age >= 18")\
  .remove_column("ssn")
```

Implementation: PyO3 methods return `clone()` of `self` to Python (Python can't hold Rust references):

```rust
#[pymethods]
impl PyBundleBuilder {
    fn filter(&mut self, expr: String) -> PyResult<Self> {
        self.inner.filter(&expr)?;
        Ok(self.clone()) // Clone for Python (Arc-based, cheap)
    }
}
```

## Related Decisions

- [ADR-004](004-three-tier-architecture.md) - Only BundleBuilder has mutation methods
- [ADR-006](006-lazy-evaluation.md) - Operations recorded, executed later
- See [python-api.md](../python-api.md) for Python API patterns

## Trade-off Analysis

| Pattern | Chaining | Efficiency | Idiomatic | Python Match |
|---------|----------|------------|-----------|--------------|
| `Self` | ✅ Yes | ⚠️ Moves | ✅ Yes | ✅ Yes |
| `()` | ❌ No | ✅ Best | ✅ Yes | ❌ No |
| `&mut Self` ✓ | ✅ Yes | ✅ Good | ⚠️ Less | ✅ Yes |

**Choice rationale**: Chaining + Python compatibility outweigh non-idiomatic Rust.

## Future Considerations

If branching becomes important, we could add non-chainable mutations:

```rust
impl BundleBuilder {
    // Chainable (current)
    pub fn filter(&mut self, expr: &str) -> &mut Self;

    // Non-chainable (for branching)
    pub fn filter_in_place(&mut self, expr: &str) -> Result<()>;
}
```

Currently not needed; users can `.clone()` (cheap) if branching required.
