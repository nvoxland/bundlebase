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
