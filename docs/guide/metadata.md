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

## Set Config

Set a configuration value for the bundle. Use the optional `url_prefix` to scope the config to a specific URL pattern.

=== "Async API"

    ```python
    # Global config
    await bundle.set_config("region", value="us-east-1")

    # Config scoped to a URL prefix
    await bundle.set_config("region", value="eu-west-1", url_prefix="s3://eu-bucket/")
    ```

=== "Sync API"

    ```python
    bundle.set_config("region", value="us-east-1")
    bundle.set_config("region", value="eu-west-1", url_prefix="s3://eu-bucket/")
    ```

=== "SQL"

    ```sql
    SET CONFIG region = 'us-east-1'

    SET CONFIG region = 'eu-west-1' FOR 's3://eu-bucket/'
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
