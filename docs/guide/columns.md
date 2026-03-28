# Columns

## Identifiers and Case Sensitivity

**Bundlebase is always case-sensitive.** Column names preserve their exact case — `Revenue`, `revenue`, and `REVENUE` are three different columns. This is intentional: bundlebase works with disparate data sources (CSVs, APIs, Parquet files, databases) that each have their own casing conventions, so no normalization is assumed.

**Quoted identifiers:** Use double quotes for column names containing spaces, dots, slashes, or other special characters:

```sql
RENAME COLUMN "ResultMeasureValue" TO secchi_depth
CAST COLUMN "Measure/Unit" TO Utf8
DROP COLUMN "column with spaces"
```

Bare identifiers (without quotes) work for names containing only letters, digits, and underscores. Quotes are always optional for such names.

!!! tip
    If you're working with data that has messy column names (spaces, mixed case, special characters), use `STANDARDIZE COLUMN NAMES` to normalize them all at once.

## Drop Column

Remove a column from the bundle.

=== "Async API"

    ```python
    await bundle.drop_column("middle_name")
    ```

=== "Sync API"

    ```python
    bundle.drop_column("middle_name")
    ```

=== "SQL"

    ```sql
    DROP COLUMN middle_name
    ```

## Rename Column

Rename an existing column.

=== "Async API"

    ```python
    await bundle.rename_column("first_name", new_name="name")
    ```

=== "Sync API"

    ```python
    bundle.rename_column("first_name", new_name="name")
    ```

=== "SQL"

    ```sql
    RENAME COLUMN first_name TO name
    ```

## Standardize Column Names

Convert all column names to lowercase, underscore-separated identifiers that work without quoting in SQL. Spaces, dashes, dots, and other special characters are replaced with underscores, consecutive underscores are collapsed, and leading/trailing underscores are stripped.

For example, `"Customer Id"` becomes `customer_id` and `"Phone 1"` becomes `phone_1`.

=== "Async API"

    ```python
    await bundle.standardize_column_names()
    ```

=== "Sync API"

    ```python
    bundle.standardize_column_names()
    ```

## Add Column

Create a computed column from a SQL expression.

=== "Async API"

    ```python
    await bundle.add_column("full_name", "first_name || ' ' || last_name")
    ```

=== "Sync API"

    ```python
    bundle.add_column("full_name", "first_name || ' ' || last_name")
    ```

=== "SQL"

    ```sql
    ADD COLUMN full_name AS first_name || ' ' || last_name
    ```

Computed columns can be indexed just like regular columns.

## Cast Column

Change a column's data type.

=== "Async API"

    ```python
    await bundle.cast_column("price", "integer")
    ```

=== "Sync API"

    ```python
    bundle.cast_column("price", "integer")
    ```

=== "SQL"

    ```sql
    CAST COLUMN price TO integer
    ```
