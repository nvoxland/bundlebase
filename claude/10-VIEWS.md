# Views

Views are named snapshots of container transformations that are stored within the bundle's manifest structure. They allow you to create reusable, versioned query patterns that automatically inherit changes from their parent container.

## Overview

A view captures uncommitted operations (like `select()`, `filter()`, etc.) from a BundleBuilder and stores them as a named, independent bundle that references its parent container. When you open a view, it automatically loads all parent operations plus its own captured operations.

## Key Concepts

### What is a View?

- A **named fork** of a container that captures a specific transformation pipeline
- Stored in `_manifest/view_{id}/_manifest/` subdirectory within the parent container
- Has its own commit history starting with an init commit that references the parent via `from` field
- **Read-only** when opened - returns a `Bundle` not a `BundleBuilder`
- **Dynamic inheritance** - automatically sees new commits from parent container

### View Storage Structure

```
container/
├── _manifest/
│   ├── 00000000000000000.yaml      # Parent init commit
│   ├── 00001abc123.yaml            # Parent commit 1
│   ├── 00002def456.yaml            # Parent commit 2 (contains AttachView op)
│   └── view_{uuid}/                # View subdirectory
│       └── _manifest/
│           ├── 00000000000000000.yaml  # View init: from="../../../"
│           └── 00001xyz789.yaml        # View commit with captured operations
└── data/
```

### How Views Inherit from Parents

1. View's init commit contains `from: <parent_url>` field
2. When opening a view, `Bundle::open()` follows the `from` reference
3. Parent bundle is recursively loaded first
4. View's operations are applied on top of parent's operations
5. If parent has new commits, view automatically sees them on next open

## Python API

### Creating a View

```python
import bundlebase

# Create container and add data
c = await bundlebase.create("/path/to/container")
await c.attach("customers.csv")
await c.commit("Initial data")

# Create a filtered view
adults = await c.select("select * where age > 21")
await c.attach_view("adults", adults)
await c.commit("Add adults view")

# Create a view with multiple operations
working_age = await c.select("select * where age > 21")
await working_age.filter("age < 65")
await c.attach_view("working_age", working_age)
await c.commit("Add working age view")
```

### Opening a View

```python
# Open view - returns read-only Bundle
view = await c.view("adults")

# Access view properties
print(f"View has {len(view.operations())} operations")
for op in view.operations():
    print(f"  - {op.describe()}")

# Views are read-only Bundles
# view.filter(...)  # ERROR: Bundle doesn't have filter()
# view.attach(...)  # ERROR: Bundle doesn't have attach()
```

### View Inheritance Example

```python
# Create container with initial data
c = await bundlebase.create("container")
await c.attach("customers-1-100.csv")
await c.commit("v1")

# Create view
active = await c.select("select * where status = 'active'")
await c.attach_view("active", active)
await c.commit("v2")

# Open view - sees data from customers-1-100.csv
view1 = await c.view("active")
print(f"Operations: {len(view1.operations())}")  # 3: CREATE PACK, ATTACH, SELECT

# Add more data to parent
c_bundle = await bundlebase.open("container")
c_reopened = c_bundle.extend("container")
await c_reopened.attach("customers-101-200.csv")
await c_reopened.commit("v3")

# Open view again - now sees both data files!
view2 = await c_reopened.view("active")
print(f"Operations: {len(view2.operations())}")  # 4: CREATE PACK, ATTACH, ATTACH, SELECT
```

## Rust API

### Creating a View

```rust
use bundlebase::{BundleBuilder, BundleFacade};

let mut c = BundleBuilder::create("memory:///container").await?;
c.attach("data.csv").await?;
c.commit("Initial").await?;

// Create view from select
let adults = c.select("select * where age > 21", vec![]).await?;
c.attach_view("adults", &adults).await?;
c.commit("Add adults view").await?;
```

### Opening a View

```rust
// Open view - returns Bundle
let view = c.view("adults").await?;

// Access operations
for op in view.operations() {
    println!("{}", op.describe());
}
```

## Operation Details

### AttachViewOp

The `AttachViewOp` operation is created when you call `attach_view()`. It:

1. **Captures operations** - Extracts all uncommitted operations from the source BundleBuilder
2. **Generates view ID** - Creates unique ObjectId for the view directory
3. **Creates view directory** - `_manifest/view_{id}/_manifest/`
4. **Writes init commit** - With `from` field pointing to parent container URL
5. **Writes first commit** - Contains the captured operations
6. **Registers view** - Stores name→ID mapping in parent Bundle's `views` HashMap

When applied to a Bundle:
- Adds the view name→ID mapping to `bundle.views`
- Does NOT modify the dataframe (views are metadata only)

### View Resolution

When you call `view(name)`:

1. Looks up view ID from `bundle.views` HashMap
2. Constructs view path: `<parent>/_manifest/view_{id}/`
3. Calls `Bundle::open()` on view path
4. Bundle loading process:
   - Reads view's init commit
   - Sees `from` field pointing to parent
   - Recursively loads parent bundle first
   - Applies parent's operations
   - Then applies view's operations on top

## Use Cases

### 1. Reusable Query Patterns

```python
# Define common filters as views
await c.attach_view("high_value", await c.select("select * where value > 1000"))
await c.attach_view("recent", await c.select("select * where date > today() - 30"))
await c.commit("Add standard views")

# Reuse later
high_value = await c.view("high_value")
```

### 2. Multi-Tenant Data Access

```python
# Create tenant-specific views
tenant_a = await c.select("select * where tenant_id = 'A'")
await c.attach_view("tenant_a", tenant_a)

tenant_b = await c.select("select * where tenant_id = 'B'")
await c.attach_view("tenant_b", tenant_b)

await c.commit("Add tenant views")
```

### 3. Versioned Analytics

```python
# Create analysis-specific views
await c.attach_view("sales_analysis",
    await c.select("""
        select product, sum(revenue) as total
        group by product
    """))
await c.commit("Q4 2024 analysis")

# View automatically updates when parent data changes
```

## Best Practices

### Do:
- ✅ Use views for reusable query patterns
- ✅ Create views from clean, focused transformations
- ✅ Commit after creating views
- ✅ Use descriptive view names

### Don't:
- ❌ Try to modify a view (they're read-only)
- ❌ Create views with uncommitted operations in parent
- ❌ Create circular view references
- ❌ Use views as primary data storage (they reference parent)

## Limitations

1. **Read-only** - Views return `Bundle` not `BundleBuilder`, so you can't call transformation methods
2. **Query execution** - Currently views can access operations metadata but executing queries (`.to_pandas()`, `.dataframe()`) may have schema resolution issues
3. **No nested views** - You cannot create a view of a view
4. **Parent dependency** - View requires parent container to be accessible

## Implementation Notes

### Key Files
- `rust/bundlebase/src/bundle/operation/attach_view.rs` - AttachViewOp implementation
- `rust/bundlebase/src/bundle/builder.rs` - attach_view() and view() methods
- `rust/bundlebase/src/bundle.rs` - views HashMap and view resolution
- `rust/bundlebase-python/src/builder.rs` - Python bindings

### Critical Bug Fix
The implementation required fixing `list_files()` in `object_store_dir.rs` to exclude subdirectory files. Previously, when loading a container's manifest, it would recursively find view manifest files and incorrectly load them, causing duplicate operations.

## Future Enhancements

Potential improvements:
- Support for materialized views (pre-computed results)
- View-specific permissions and access control
- View dependencies and composition
- Better dataframe execution support for views
- View usage statistics and monitoring
