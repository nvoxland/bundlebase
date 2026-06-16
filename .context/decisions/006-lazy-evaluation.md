# ADR-006: Lazy Operation Evaluation

**Status:** Accepted — Amended June 2026 (mechanism description updated to match the implemented `Operation` trait; there is no `BundleState` type or `reconfigure()` method)

**Date:** December 2024 (documented retroactively); amended June 2026

## Context

When users call operations like `filter()` or `drop_column()`, should the operation execute immediately or be deferred until query time?

### The Problem

**Immediate execution**:
- Simple mental model (operation happens when called)
- Can validate inputs immediately
- But: Forces eager evaluation (can't optimize query)
- But: Must load data even if not queried yet

**Lazy evaluation**:
- Can optimize query plan before execution
- Only executes when data actually needed
- But: Errors deferred (may not see them immediately)
- But: More complex mental model

### Alternatives Considered

**Option 1: Eager/immediate execution** (like Pandas)
- Pros: Simple, immediate feedback on errors
- Cons: Can't optimize query, forces data load even if not queried

**Option 2: Fully lazy** (like Spark RDDs)
- Pros: Maximum optimization potential, deferred execution
- Cons: Late error feedback, hard to debug

**Option 3: Hybrid lazy** (chosen - like DataFusion)
- Pros: Early validation, optimization potential, clear execution point
- Cons: Complexity in implementation (validate without executing)

## Decision

**Record operations when called, validate immediately, and rebuild the DataFrame lazily on demand.**

### The Real Operation Trait

Operations implement the `Operation` trait (rust/bundlebase/src/bundle/operation.rs):

```rust
pub trait Operation: Send + Sync + Clone + Serialize + Debug + Into<AnyOperation> {
    /// Human-readable description of this operation
    fn describe(&self) -> String;

    /// Validate this operation against the current bundle (immediate, fail fast)
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError>;

    /// Apply side effects to the bundle via interior mutability (immediate) —
    /// e.g. register a pack, set the name, record a view. Default for many ops: no-op.
    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError>;

    /// Transform the DataFrame (LAZY — called when the dataframe is rebuilt).
    /// Default implementation returns the DataFrame unchanged.
    async fn apply_dataframe(
        &self,
        df: DataFrame,
        ctx: Arc<SessionContext>,
        bundle_schema: &mut BundleSchema,
    ) -> Result<DataFrame, BundlebaseError>;

    // Plus: version(), allowed_on_view(), to_empty() — see operation.rs
}
```

There is **no `BundleState` type and no `reconfigure()` method** (an earlier design that was never implemented this way). Schema/name tracking is done by `BundleSchema` (a `ColumnId -> name` map threaded through `apply_dataframe`).

### How Lazy Evaluation Works

1. **Recording** — `Bundle::apply_operation(&self, op)` runs `check()` and `apply()` immediately, pushes the op onto `Bundle::operations`, and **clears the cached DataFrame** (`self.dataframe.clear()`).

2. **Rebuilding on demand** — `Bundle::dataframe()` (in `bundle.rs`) returns the cached DataFrame if present; otherwise it rebuilds:
   - Start from the base pack table
   - Compute `BundleSchema::initial(&ops)` and `resolve_cast_ops(&ops)` (cancels reverted casts)
   - Apply each active operation's `apply_dataframe()` in order (joins are executed inline when a `CreateJoin` op is reached)
   - Finally rename internal `col_<id>` columns to user-visible names and cache the result

### Example Flow

```rust
// User code (via BundleBuilderExt)
builder.filter("SELECT * FROM bundle WHERE age >= 18", vec![]).await?; // check + record
builder.drop_column("ssn").await?;                                     // check + record

// Later... (maybe much later)
let df = builder.dataframe().await?; // NOW apply_dataframe runs for every recorded op
```

## Consequences

### Positive

- **Early validation**: `check()` catches errors (missing columns, bad references) when the operation is recorded
- **Deferred execution**: `apply_dataframe()` only runs when data is actually needed
- **Query optimization**: The rebuilt DataFrame is a DataFusion logical plan — optimization and predicate pushdown happen at execution
- **Composable**: Build up the transformation pipeline incrementally
- **Serializable**: Recorded operations are saved/loaded via manifests (serde with `AnyOperation`)

### Negative

- **Complex implementation**: Validation/side-effect/DataFrame phases are more code than simple execution
- **Mental model**: Users must understand when operations execute
- **Partial errors**: An operation may pass `check()` but fail later in `apply_dataframe()`
- **Cache invalidation**: Every recorded operation clears the cached DataFrame and `BundleSchema`

### Neutral

- **Inspection**: Operation list can be queried without executing (`describe()`, `bundle_info` tables)
- **Versioning**: Each operation contributes a content hash to the bundle version (`version()`)

## Implementation Examples (from the codebase)

### RenameColumnOp (metadata-only)

```rust
// rust/bundlebase/src/bundle/operation/rename_column.rs
impl Operation for RenameColumnOp {
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        bundle.column_name(&self.id)
            .ok_or_else(|| BundlebaseError::from(format!("Column with ID '{}' not found", self.id)))?;
        if bundle.bundle_schema().column_id(&self.new_name).is_some() {
            return Err(format!("Column '{}' already exists in the schema", self.new_name).into());
        }
        Ok(())
    }

    async fn apply(&self, _bundle: &Bundle) -> Result<(), DataFusionError> {
        Ok(()) // no bundle side effects
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        _ctx: Arc<SessionContext>,
        bundle_schema: &mut BundleSchema,
    ) -> Result<DataFrame, BundlebaseError> {
        // Metadata-only: update the name map for the final rename step.
        bundle_schema.insert(self.id, self.new_name.clone());
        Ok(df)
    }

    fn describe(&self) -> String {
        format!("RENAME COLUMN: {} to {}", self.id, self.new_name)
    }
}
```

### FilterOp (DataFrame transformation)

```rust
// rust/bundlebase/src/bundle/operation/filter.rs (simplified)
impl Operation for FilterOp {
    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        Ok(())
    }

    async fn apply(&self, _bundle: &Bundle) -> Result<(), DataFusionError> {
        Ok(())
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        ctx: Arc<SessionContext>,
        bundle_schema: &mut BundleSchema,
    ) -> Result<DataFrame, BundlebaseError> {
        // Translate user-visible column names to stable col_<id> names,
        // register `df` as the `bundle` table in an ephemeral context,
        // and run the stored SELECT against it.
        let translated_query = bundle_schema.translate_sql(&self.query);
        // ... create plan with parameters, execute_logical_plan ...
    }
}
```

## User Mental Model

**What users see**:
```python
c = c.filter("age >= 18")        # "Records filter operation"
c = c.drop_column("ssn")         # "Records drop operation"
df = await c.to_pandas()         # "NOW executes pipeline"
```

**What happens internally**:
1. `filter()` - `check()` validates, op recorded, cached DataFrame cleared
2. `drop_column()` - same
3. `to_pandas()` - `dataframe()` rebuilds the DataFrame through every recorded op, then streams results

## Related Decisions

- [ADR-002](002-datafusion-arrow.md) - DataFusion provides lazy DataFrame evaluation
- [ADR-004](004-three-tier-architecture.md) - Operations recorded through BundleBuilder
- [ADR-005](005-mutable-operations.md) - Interior mutability for shared access
- See [architecture.md](../architecture.md#operation-pattern-checkapplyapply_dataframe)

## Trade-offs

| Approach | Validation | Optimization | Simplicity | User Model |
|----------|------------|--------------|------------|------------|
| Eager | ✅ Immediate | ❌ None | ✅ Simple | ✅ Clear |
| Fully Lazy | ❌ Deferred | ✅ Maximum | ⚠️ Medium | ⚠️ Complex |
| Hybrid ✓ | ✅ Early | ✅ Good | ⚠️ Medium | ✅ Clear |

**Conclusion**: Hybrid approach balances optimization potential with usability.

## Decision History

- **Dec 2024**: Recorded with a three-phase `check()/reconfigure()/apply_dataframe()` trait operating on a `BundleState` type.
- **Amended June 2026**: The implemented trait is `check(&Bundle)` / `apply(&Bundle)` / `apply_dataframe(df, ctx, &mut BundleSchema)`; `BundleState` was never built. The lazy-evaluation decision itself stands — only the mechanism description changed.
