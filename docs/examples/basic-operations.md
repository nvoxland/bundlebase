# Basic Operations

This guide demonstrates common Bundlebase operations with practical examples.

## Creating Bundles

### In-Memory Bundle

```python
import bundlebase as bb

# Create a temporary in-memory bundle
c = await bb.create("memory:///")
```

### Persistent Bundle

```python
import bundlebase as bb

# Create a bundle at a specific path
c = await bb.create("/path/to/bundle")

# Later, open the saved bundle
c = await bb.open("/path/to/bundle")
```

## Loading Data

### Single File

```python
import bundlebase as bb

c = await bb.create("memory:///")
c = await c.attach("file:///path/data.parquet")
```

### Multiple Files

```python
import bundlebase as bb

# Files are automatically unioned
c = await bb.create("memory:///")
c = await c.attach("file:///path/january.parquet")
c = await c.attach("file:///path/february.parquet")
c = await c.attach("file:///path/march.parquet")
```

## Filtering Data

### Simple Filters

```python
import bundlebase as bb

c = await (bb.create("memory:///")
    .attach("file:///path/data.parquet")
    .filter("age >= 18"))

df = await c.to_pandas()
```

### Complex Filters

```python
import bundlebase as bb

c = await (bb.create("memory:///")
    .attach("file:///path/data.parquet")
    .filter("age >= 18 AND status = 'active' AND balance > 1000"))

df = await c.to_pandas()
```

### Parameterized Filters

```python
import bundlebase as bb

min_age = 18
status = "active"

c = await (bb.create("memory:///")
    .attach("file:///path/data.parquet")
    .filter("age >= $1 AND status = $2", [min_age, status]))

df = await c.to_pandas()
```

## Column Operations

### Removing Columns

```python
import bundlebase as bb

# Remove sensitive columns
c = await (bb.create("memory:///")
    .attach("file:///path/data.parquet")
    .remove_column("ssn")
    .remove_column("credit_card")
    .remove_column("password"))

df = await c.to_pandas()
```

### Renaming Columns

```python
import bundlebase as bb

# Rename columns for clarity
c = await (bb.create("memory:///")
    .attach("file:///path/data.parquet")
    .rename_column("fname", "first_name")
    .rename_column("lname", "last_name")
    .rename_column("addr", "address"))

df = await c.to_pandas()
```

## Data Export

### Pandas DataFrame

```python
import bundlebase as bb

c = await (bb.create("memory:///")
    .attach("file:///path/data.parquet")
    .filter("active = true"))

# Convert to pandas
df = await c.to_pandas()

# Continue with pandas operations
df = df.sort_values("date")
df.to_csv("output.csv")
```

### Polars DataFrame

```python
import bundlebase as bb

c = await (bb.create("memory:///")
    .attach("file:///path/data.parquet"))

# Convert to Polars
df = await c.to_polars()

# Continue with Polars operations
result = df.group_by("category").agg(pl.col("value").sum())
```

### NumPy Arrays

```python
import bundlebase as bb

c = await (bb.create("memory:///")
    .attach("file:///path/data.parquet")
    .select(["x", "y", "z"]))

# Convert to NumPy arrays
arrays = await c.to_numpy()

# Access arrays by column name
x = arrays["x"]
y = arrays["y"]
z = arrays["z"]

# Perform NumPy operations
mean_x = x.mean()
```

### Python Dictionary

```python
import bundlebase as bb

c = await (bb.create("memory:///")
    .attach("file:///path/data.parquet"))

# Convert to dictionary
data = await c.to_dict()

# Access data by column name
ids = data["id"]
names = data["name"]
ages = data["age"]
```

## Indexing

### Creating Indexes

```python
import bundlebase as bb

c = await (bb.create("memory:///")
    .attach("file:///path/data.parquet")
    .define_index("email")  # Create index on email column
    .define_index("user_id"))  # Create index on user_id column

# Queries on indexed columns will be faster
c = await c.filter("email = 'user@example.com'")
```

## Complete Example: Data Cleaning Pipeline

```python
import bundlebase as bb

# Create a comprehensive data cleaning pipeline
c = await (bundlebase.create("/path/to/cleaned_data")
    # Load data
    .attach("raw_customers.csv")
    .attach("raw_orders.csv")

    # Remove PII
    .remove_column("ssn")
    .remove_column("email")
    .remove_column("phone")
    .remove_column("credit_card")

    # Filter to active records
    .filter("status = 'active'")
    .filter("year >= 2020")

    # Rename columns for clarity
    .rename_column("fname", "first_name")
    .rename_column("lname", "last_name")
    .rename_column("addr", "address")
    .rename_column("zip", "postal_code")

    # Select final columns
    .select([
        "id",
        "first_name",
        "last_name",
        "address",
        "postal_code",
        "country",
        "order_total"
    ]))

# Export cleaned data
df = await c.to_pandas()
print(f"Cleaned {len(df)} records")

# Commit for versioning
await c.commit("Data cleaning pipeline v1.0")
```

## See Also

- [Getting Started](../getting-started/quick-start.md)
- [User Guide](../guide/attaching.md) 
- [API Reference](../api/python/index.md)
