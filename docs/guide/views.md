# Views

Views are named, reusable query patterns stored within the bundle. They automatically inherit changes from their parent container -- when you add new data to the parent, views see it on next open.

## Creating a View

Create a view by providing a name and a SQL query.

=== "Async API"

    ```python
    import bundlebase as bb

    bundle = await bb.create("my/data")
    await bundle.attach("customers.csv")

    await bundle.create_view("adults", sql="SELECT * FROM bundle WHERE age > 21")
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    bundle = bb.create("my/data")
    bundle.attach("customers.csv")

    bundle.create_view("adults", sql="SELECT * FROM bundle WHERE age > 21")
    ```

=== "SQL"

    ```sql
    CREATE VIEW adults AS SELECT * FROM bundle WHERE age > 21
    ```

## Opening a View

=== "Async API"

    ```python
    view = await bundle.view("adults")
    ```

=== "Sync API"

    ```python
    view = bundle.view("adults")
    ```

!!! note
    Views are read-only. You cannot call transformation methods like `filter()` or `attach()` on a view.

## Drop View

Remove a view from the bundle.

=== "Async API"

    ```python
    await bundle.drop_view("adults")
    ```

=== "Sync API"

    ```python
    bundle.drop_view("adults")
    ```

=== "SQL"

    ```sql
    DROP VIEW adults
    ```

## Rename View

Rename an existing view.

=== "Async API"

    ```python
    await bundle.rename_view("adults", new_name="adult_customers")
    ```

=== "Sync API"

    ```python
    bundle.rename_view("adults", new_name="adult_customers")
    ```

=== "SQL"

    ```sql
    RENAME VIEW adults TO adult_customers
    ```

## Dynamic Inheritance

Views automatically see new data added to the parent container:

=== "Async API"

    ```python
    bundle = await bb.create("my/data")
    await bundle.attach("customers-1.csv")
    await bundle.commit("v1")

    await bundle.create_view("active", sql="SELECT * FROM bundle WHERE status = 'active'")
    await bundle.commit("v2")

    # Later, add more data
    await bundle.attach("customers-2.csv")
    await bundle.commit("v3")

    # View now sees both data files
    view = await bundle.view("active")
    ```

=== "Sync API"

    ```python
    bundle = bb.create("my/data")
    bundle.attach("customers-1.csv")
    bundle.commit("v1")

    bundle.create_view("active", sql="SELECT * FROM bundle WHERE status = 'active'")
    bundle.commit("v2")

    # Later, add more data
    bundle.attach("customers-2.csv")
    bundle.commit("v3")

    # View now sees both data files
    view = bundle.view("active")
    ```
