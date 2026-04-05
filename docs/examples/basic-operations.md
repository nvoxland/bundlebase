---
title: Basic Operations — Bundlebase Examples
description: "Core Bundlebase operations: creating bundles, attaching data, filtering, column transforms, SQL queries, exporting, and versioning."
---

# Basic Operations

## Creating a bundle

=== "Python"

    ```python
    import bundlebase.sync as bb

    # Local path
    bundle = bb.create("/path/to/my-bundle")

    # Cloud storage
    bundle = bb.create("s3://my-bucket/my-bundle")

    # In-memory (no persistence, useful for testing)
    bundle = bb.create("memory:///")
    ```

=== "SQL"

    ```sql
    CREATE '/path/to/my-bundle';
    CREATE 's3://my-bucket/my-bundle';
    ```

## Opening an existing bundle

=== "Python"

    ```python
    import bundlebase.sync as bb

    bundle = bb.open("s3://my-bucket/my-bundle")

    # Inspect before doing anything
    print(bundle.name)
    print(f"{bundle.num_rows:,} rows")
    print(bundle.version)
    ```

=== "SQL"

    ```sql
    OPEN 's3://my-bucket/my-bundle';
    SHOW STATUS;
    ```

## Attaching data

Multiple files union together automatically, even across formats:

=== "Python"

    ```python
    bundle.attach("data.parquet")
    bundle.attach("s3://bucket/more-data.csv")
    bundle.attach("https://example.com/feed.json")

    # Multiple files union together
    bundle.attach("jan.parquet")
    bundle.attach("feb.parquet")
    bundle.attach("mar.parquet")
    ```

=== "SQL"

    ```sql
    ATTACH 'data.parquet';
    ATTACH 's3://bucket/more-data.csv';
    ATTACH 'https://example.com/feed.json';

    ATTACH 'jan.parquet';
    ATTACH 'feb.parquet';
    ATTACH 'mar.parquet';
    ```

!!! note
    CSV columns import as text. Use `cast_column()` to convert types after attaching.

## Filtering rows

=== "Python"

    ```python
    # Simple filter
    bundle.filter("status = 'active'")

    # Compound filter
    bundle.filter("age >= 18 AND status = 'active' AND balance > 1000")

    # Parameterized (prevents injection, handles type coercion)
    bundle.filter("age >= $1 AND status = $2", [18, "active"])
    ```

=== "SQL"

    ```sql
    FILTER WITH SELECT * FROM bundle WHERE status = 'active';

    FILTER WITH SELECT * FROM bundle WHERE age >= 18 AND status = 'active' AND balance > 1000;
    ```

## Column operations

=== "Python"

    ```python
    # Remove columns
    bundle.drop_column("ssn")
    bundle.drop_column("credit_card")

    # Rename
    bundle.rename_column("fname", "first_name")
    bundle.rename_column("lname", "last_name")

    # Normalize messy names: "Customer Id" → "customer_id", "Phone 1" → "phone_1"
    bundle.normalize_column_names()

    # Cast CSV text to typed columns
    bundle.cast_column("amount", "float64")
    bundle.cast_column("created_at", "timestamp")
    ```

=== "SQL"

    ```sql
    DROP COLUMN ssn;
    DROP COLUMN credit_card;

    RENAME COLUMN fname TO first_name;
    RENAME COLUMN lname TO last_name;

    NORMALIZE COLUMN NAMES;

    CAST COLUMN amount AS FLOAT64;
    CAST COLUMN created_at AS TIMESTAMP;
    ```

## Querying with SQL

=== "Python"

    ```python
    # Filter and reshape with SQL
    result = bundle.query("""
        SELECT region, COUNT(*) as deals, SUM(amount) as total
        FROM bundle
        WHERE status = 'closed_won'
        GROUP BY region
        ORDER BY total DESC
    """)

    df = result.to_pandas()
    ```

=== "SQL"

    ```sql
    SELECT region, COUNT(*) as deals, SUM(amount) as total
    FROM bundle
    WHERE status = 'closed_won'
    GROUP BY region
    ORDER BY total DESC;
    ```

## Exporting

=== "Python"

    ```python
    # pandas
    df = bundle.to_pandas()

    # polars
    df = bundle.to_polars()

    # numpy (returns dict of arrays keyed by column name)
    arrays = bundle.to_numpy()
    x = arrays["revenue"]

    # Streaming batches — constant memory regardless of dataset size
    for batch in bundle.stream_batches():
        process(batch)   # each batch is a PyArrow RecordBatch
    ```

=== "SQL"

    ```sql
    -- In the REPL, results stream to the terminal
    SELECT * FROM bundle;

    -- Or connect a BI tool via the SQL server
    -- bundlebase serve --bundle s3://... --port 32010
    ```

## Versioning

=== "Python"

    ```python
    import bundlebase.sync as bb

    # Create and commit
    bundle = bb.create("s3://my-bucket/sales")
    bundle.attach("jan.csv")
    bundle.commit("January data")

    # Extend (mutable copy of an existing bundle)
    bundle = bb.open("s3://my-bucket/sales").extend()
    bundle.attach("feb.csv")
    bundle.commit("Added February")

    # View history
    bundle = bb.open("s3://my-bucket/sales")
    for entry in bundle.history():
        print(entry)

    # Roll back uncommitted changes
    bundle = bb.open("s3://my-bucket/sales").extend()
    bundle.attach("bad-data.csv")
    bundle.reset()   # back to last committed state
    ```

=== "SQL"

    ```sql
    CREATE 's3://my-bucket/sales';
    ATTACH 'jan.csv';
    COMMIT 'January data';

    OPEN 's3://my-bucket/sales';
    EXTEND;
    ATTACH 'feb.csv';
    COMMIT 'Added February';

    OPEN 's3://my-bucket/sales';
    SHOW HISTORY;

    EXTEND;
    ATTACH 'bad-data.csv';
    RESET;
    ```

## Indexes

=== "Python"

    ```python
    bundle.create_index("email")
    bundle.create_index("user_id")

    # Queries on these columns now use the index automatically
    result = bundle.query("SELECT * FROM bundle WHERE email = 'user@example.com'")

    # Drop an index
    bundle.drop_index("email")
    ```

=== "SQL"

    ```sql
    CREATE INDEX email;
    CREATE INDEX user_id;

    SELECT * FROM bundle WHERE email = 'user@example.com';

    DROP INDEX email;
    ```

## Method chaining

All mutation methods return `self`, so operations can be chained:

=== "Python"

    ```python
    import bundlebase.sync as bb

    bundle = (bb.create("s3://my-bucket/sales-q1")
        .attach("jan.csv")
        .attach("feb.csv")
        .attach("mar.csv")
        .normalize_column_names()
        .cast_column("amount", "float64")
        .drop_column("internal_id")
        .filter("status = 'closed_won'")
        .set_name("Q1 Sales — Closed Won")
        .commit("Initial Q1 export"))
    ```

=== "SQL"

    ```sql
    CREATE 's3://my-bucket/sales-q1';
    ATTACH 'jan.csv';
    ATTACH 'feb.csv';
    ATTACH 'mar.csv';
    NORMALIZE COLUMN NAMES;
    CAST COLUMN amount AS FLOAT64;
    DROP COLUMN internal_id;
    FILTER WITH SELECT * FROM bundle WHERE status = 'closed_won';
    SET NAME 'Q1 Sales — Closed Won';
    COMMIT 'Initial Q1 export';
    ```
