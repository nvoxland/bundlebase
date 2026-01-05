# ADR-006: Lazy Operation Evaluation

**Status:** Accepted

**Date:** December 2024 (documented retroactively)

## Context

When users call operations like `filter()` or `select()`, should the operation execute immediately or be deferred until query time?

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
```rust
container.filter("age > 18"); // Executes filter immediately
container.select(["name"]); // Executes select immediately
df = container.to_pandas(); // Just returns already-filtered data
```
- Pros: Simple, immediate feedback on errors
- Cons: Can't optimize query, forces data load even if not queried

**Option 2: Fully lazy** (like Spark RDDs)
```rust
container.filter("age > 18"); // Records operation
container.select(["name"]); // Records operation
df = container.to_pandas(); // NOW executes entire pipeline
```
- Pros: Maximum optimization potential, deferred execution
- Cons: Late error feedback, hard to debug

**Option 3: Hybrid lazy** (chosen - like DataFusion)
```rust
container.filter("age > 18")?; // Records + validates
container.select(["name"])?; // Records + validates schema
df = container.to_pandas().await?; // Executes pipeline
```
- Pros: Early validation, optimization potential, clear execution point
- Cons: Complexity in implementation (validate without executing)

## Decision

**Record operations when called, execute lazily during query with early validation.**

### Three-Phase Pattern

Each operation implements:

1. **Validation phase** (`check()`): Validate operation is legal (immediate)
2. **Schema update** (`reconfigure()`): Update schema/metadata (immediate)
3. **DataFrame transformation** (`apply_dataframe()`): Execute on data (lazy)

```rust
pub trait Operation {
    // Phase 1: Validate (immediate)
    fn check(&self, state: &BundleState) -> Result<()>;

    // Phase 2: Update state (immediate)
    fn reconfigure(&self, state: &mut BundleState) -> Result<()>;

    // Phase 3: Execute (lazy - only during query)
    async fn apply_dataframe(&self, df: DataFrame) -> Result<DataFrame>;
}
```

### Example Flow

```rust
// User code
container.filter("age >= 18")?; // Phase 1+2: validate, update schema
container.select(["name"])?; // Phase 1+2: validate, update schema

// Later... (maybe much later)
df = container.to_pandas().await?; // Phase 3: execute all operations
```

## Consequences

### Positive

- **Early validation**: Catch errors (bad SQL, missing columns) immediately
- **Schema tracking**: Know output schema before execution
- **Query optimization**: Can reorder/combine operations before executing
- **Deferred execution**: Only execute when data actually needed
- **Composable**: Build up transformation pipeline incrementally
- **Efficient**: Avoid redundant execution if query never happens

### Negative

- **Complex implementation**: Three-phase pattern more code than simple execution
- **Mental model**: Users must understand when operations execute
- **Testing complexity**: Must test both validation and execution phases
- **Partial errors**: Operation may validate but fail during execution
- **Debug difficulty**: Error might occur far from where operation was added

### Neutral

- **Operation serialization**: Recorded operations can be saved/loaded from manifests
- **Inspection**: Can query operation list without executing them
- **Row count approximation**: Can track approximate row counts before execution

## Implementation Examples

### Filter Operation

```rust
impl Operation for FilterBlock {
    fn check(&self, state: &BundleState) -> Result<()> {
        // Validate SQL syntax
        parse_sql(&self.expression)?;
        // Validate referenced columns exist
        validate_columns(&self.expression, state.schema())?;
        Ok(())
    }

    fn reconfigure(&self, state: &mut BundleState) -> Result<()> {
        // Schema unchanged, but row count may decrease
        state.set_row_count_approximate();
        Ok(())
    }

    async fn apply_dataframe(&self, df: DataFrame) -> Result<DataFrame> {
        // NOW actually filter data
        df.filter(col(&self.column).gt(lit(18)))
    }
}
```

### Select Operation

```rust
impl Operation for SelectColumns {
    fn check(&self, state: &BundleState) -> Result<()> {
        // Validate all columns exist
        for col in &self.columns {
            if !state.schema().column_with_name(col).is_some() {
                return Err(format!("Column not found: {}", col).into());
            }
        }
        Ok(())
    }

    fn reconfigure(&self, state: &mut BundleState) -> Result<()> {
        // Update schema to only selected columns
        let new_schema = state.schema().project(&self.columns)?;
        state.set_schema(new_schema);
        Ok(())
    }

    async fn apply_dataframe(&self, df: DataFrame) -> Result<DataFrame> {
        // Execute selection
        df.select_columns(&self.columns)
    }
}
```

## Benefits for Query Optimization

Lazy evaluation enables optimizations:

```rust
// User code
container
    .filter("age >= 18")?
    .select(["name", "age"])?
    .filter("age < 65")?;

// Optimizer can:
// 1. Combine filters: "age >= 18 AND age < 65"
// 2. Push select before filter: select(filter(data))
// 3. Push filters into data source (predicate pushdown)
```

## User Mental Model

**What users see**:
```python
c.filter("age >= 18") # "Records filter operation"
c.select(["name"]) # "Records select operation"
df = await c.to_pandas() # "NOW executes pipeline"
```

**What happens internally**:
1. `filter()` - validate SQL, update schema, record operation
2. `select()` - validate columns exist, update schema, record operation
3. `to_pandas()` - execute all recorded operations via DataFusion

## Related Decisions

- [ADR-002](002-datafusion-arrow.md) - DataFusion provides lazy DataFrame evaluation
- [ADR-004](004-three-tier-architecture.md) - Operations recorded on BundleBuilder
- [ADR-005](005-mutable-operations.md) - Operations chainable via `&mut Self`
- See [architecture.md](../architecture.md#three-phase-operation-pattern)

## Trade-offs

| Approach | Validation | Optimization | Simplicity | User Model |
|----------|------------|--------------|------------|------------|
| Eager | ✅ Immediate | ❌ None | ✅ Simple | ✅ Clear |
| Fully Lazy | ❌ Deferred | ✅ Maximum | ⚠️ Medium | ⚠️ Complex |
| Hybrid ✓ | ✅ Early | ✅ Good | ⚠️ Medium | ✅ Clear |

**Conclusion**: Hybrid approach balances optimization potential with usability.
