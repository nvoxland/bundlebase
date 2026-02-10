# Metadata

## Set Name

Set the bundle's display name.

=== "Async API"

    ```python
    await bundle.set_name("Customer Data")
    ```

=== "Sync API"

    ```python
    bundle.set_name("Customer Data")
    ```

=== "SQL"

    ```sql
    SET NAME 'Customer Data'
    ```

## Set Description

Set the bundle's description.

=== "Async API"

    ```python
    await bundle.set_description("Contains all customer records from 2024")
    ```

=== "Sync API"

    ```python
    bundle.set_description("Contains all customer records from 2024")
    ```

=== "SQL"

    ```sql
    SET DESCRIPTION 'Contains all customer records from 2024'
    ```

## Accessing Properties

Bundle metadata is available as properties:

=== "Async API"

    ```python
    import bundlebase as bb

    bundle = await bb.open("my/data")
    print(bundle.name)
    print(bundle.description)
    print(bundle.id)
    print(bundle.url)
    print(bundle.version)
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    bundle = bb.open("my/data")
    print(bundle.name)
    print(bundle.description)
    print(bundle.id)
    print(bundle.url)
    print(bundle.version)
    ```

=== "SQL"

    ```sql
    SELECT * FROM bundle_info.details
    ```

=== "REPL"

    ```
    /details
    ```
