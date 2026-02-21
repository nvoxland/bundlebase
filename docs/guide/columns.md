# Columns

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
