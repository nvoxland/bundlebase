# Operation Chains

Fluent method chaining for Bundlebase operations.

## Overview

Bundlebase uses operation chains to enable fluent method chaining while maintaining clean async/await syntax. This allows you to queue multiple operations before execution.

## Chain Classes

### OperationChain

::: bundlebase.chain.OperationChain
    options:
      show_root_heading: true
      show_root_full_path: false

### CreateChain

::: bundlebase.chain.CreateChain
    options:
      show_root_heading: true
      show_root_full_path: false

### ExtendChain

::: bundlebase.chain.ExtendChain
    options:
      show_root_heading: true
      show_root_full_path: false

## How It Works

When you call a mutation method on a bundle, it returns an `OperationChain` that queues the operation:

```python
import bundlebase

# Each method call returns a chain
c = await bundlebase.create()  # CreateChain
c = await c.attach("data.parquet")  # OperationChain
c = await c.filter("age >= 18")  # OperationChain
c = await c.remove_column("ssn")  # OperationChain

# Final await executes all queued operations
df = await c.to_pandas()
```

## Examples

### Basic Chaining

```python
import bundlebase

# Chain operations before execution
c = await (bundlebase.create()
    .attach("data.parquet")
    .filter("active = true")
    .remove_column("temp"))

# Execute when needed
df = await c.to_pandas()
```

### Create Chain

The `create()` function returns a `CreateChain` that can queue operations before the bundle is created:

```python
import bundlebase

# All operations are queued
chain = (bundlebase.create()
    .attach("data.parquet")
    .filter("age >= 18")
    .rename_column("fname", "first_name"))

# Single await executes creation + all operations
c = await chain
df = await c.to_pandas()
```

### Extend Chain

When extending a bundle, you get an `ExtendChain`:

```python
import bundlebase

# Open existing bundle
original = await bundlebase.open("/path/to/bundle")

# Extend with chained operations
extended = await (original.extend("/path/to/new/bundle")
    .attach("new_data.parquet")
    .filter("year >= 2020"))

# Commit the extended bundle
await extended.commit("Added 2020+ data")
```

### Mixed Chaining and Direct Calls

You can mix chaining with direct method calls:

```python
import bundlebase

# Start with a chain
c = await (bundlebase.create()
    .attach("data.parquet")
    .filter("age >= 18"))

# Continue with direct calls
c = await c.remove_column("ssn")
c = await c.rename_column("fname", "first_name")

# Or resume chaining
c = await (c
    .select(["id", "first_name", "last_name"])
    .filter("active = true"))

df = await c.to_pandas()
```

## Benefits of Operation Chains

### 1. Clean Syntax

Chaining enables clean, readable code:

```python
# With chaining
c = await (bundlebase.create()
    .attach("data.parquet")
    .filter("age >= 18")
    .remove_column("ssn"))

# Without chaining (more verbose)
c = await bundlebase.create()
c = await c.attach("data.parquet")
c = await c.filter("age >= 18")
c = await c.remove_column("ssn")
```

### 2. Performance

Operations are queued and executed together, reducing async overhead:

```python
# Good: Single async call
c = await (bundlebase.create()
    .attach("data.parquet")
    .filter("age >= 18")
    .remove_column("ssn"))  # All operations execute together

# Less optimal: Multiple async calls
c = await bundlebase.create()
c = await c.attach("data.parquet")  # Separate async call
c = await c.filter("age >= 18")  # Separate async call
c = await c.remove_column("ssn")  # Separate async call
```

### 3. Type Safety

Chains maintain type information for IDE autocomplete:

```python
import bundlebase

# IDE knows c is PyBundleBuilder
c = await (bundlebase.create()
    .attach("data.parquet")  # Autocomplete available
    .filter("age >= 18")  # Autocomplete available
    .remove_column("ssn"))  # Autocomplete available
```

## Advanced Usage

### Conditional Chaining

Build chains conditionally:

```python
import bundlebase

# Start with base chain
chain = bundlebase.create().attach("data.parquet")

# Conditionally add operations
if filter_active:
    chain = chain.filter("active = true")

if remove_pii:
    chain = chain.remove_column("email")
    chain = chain.remove_column("phone")

# Execute final chain
c = await chain
```

### Reusable Chain Builders

Create functions that return chains:

```python
import bundlebase
from bundlebase.chain import OperationChain

def clean_customer_data(chain: OperationChain) -> OperationChain:
    """Standard customer data cleaning."""
    return (chain
        .filter("active = true")
        .remove_column("email")
        .remove_column("phone")
        .rename_column("fname", "first_name")
        .rename_column("lname", "last_name"))

# Use the builder
c = await clean_customer_data(
    bundlebase.create().attach("customers.parquet")
)

df = await c.to_pandas()
```

### Programmatic Chain Building

Build chains programmatically:

```python
import bundlebase

# Start with create
chain = bundlebase.create().attach("data.parquet")

# Add operations from a list
operations = [
    ("filter", ["age >= 18"], {}),
    ("remove_column", ["ssn"], {}),
    ("rename_column", ["fname", "first_name"], {}),
]

for method, args, kwargs in operations:
    chain = getattr(chain, method)(*args, **kwargs)

# Execute
c = await chain
```

## Implementation Details

Operation chains work by:

1. **Queueing Operations**: Each method call adds to an operation queue
2. **Deferring Execution**: No operations execute until the chain is awaited
3. **Batch Execution**: All queued operations execute together when awaited

This provides both clean syntax and good performance.

## Sync API Chains

The sync API also supports chaining:

```python
import bundlebase.sync as dc

# Chains work in sync API too
c = (dc.create()
    .attach("data.parquet")
    .filter("age >= 18")
    .remove_column("ssn"))

df = c.to_pandas()  # No await needed
```

## See Also

- **[Async API](async-api.md)** - Main API documentation
- **[Quick Start](../../getting-started/quick-start.md#method-chaining)** - Chaining examples
- **[Basic Concepts](../../getting-started/basic-concepts.md)** - Architecture overview
