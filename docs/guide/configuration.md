# Configuration

Bundlebase uses configuration to control how it connects to cloud storage providers, remote servers, and external services. 

## Priority Order

Configuration values are resolved with the following priority (highest wins):

```
runtime config (SET CONFIG)  >  passed config  >  environment variables  >  stored config
```

| Source | Description | Priority |
|--------|-------------|----------|
| **Runtime config** | Set via `SET CONFIG` or `set_config()` during the session | Highest |
| **Passed config** | Dict passed to `create()`/`open()` | High |
| **Environment variables** | `BB_*` env vars set at runtime | Medium |
| **Stored config** | Persisted in the bundle via `SAVE CONFIG` | Lowest |

This means a value set via `SET CONFIG` during a session always overrides the same key from any other source.

## Configuration Methods

### Runtime Config — SET CONFIG (Highest Priority)

Use `SET CONFIG` or `set_config()` to set a configuration value for the current session only. This is the highest-priority config source. The value is not persisted and is lost when the session ends.

=== "Async API"

    ```python
    await bundle.set_config("region", "us-west-2")
    await bundle.set_config("endpoint", "http://localhost:9000", scope="/s3/data")
    await bundle.set_config("region", "eu-west-1", scope="/prod")
    ```

=== "Sync API"

    ```python
    bundle.set_config("region", "us-west-2")
    bundle.set_config("endpoint", "http://localhost:9000", scope="/s3/data")
    ```

=== "SQL"

    ```sql
    SET CONFIG region = 'us-west-2'
    SET CONFIG endpoint = 'http://localhost:9000' FOR '/s3/data'
    SET CONFIG region = 'eu-west-1' FOR '/prod'
    ```

### Passed Config (High Priority)

Pass a config dict to `create()` or `open()`. These values take effect for the current session only and are not persisted.

=== "Async API"

    ```python
    import bundlebase as bb

    # Simple dict
    bundle = await bb.create("my/data", config={
        "region": "us-west-2",
        "access_key_id": "AKIA...",
        "secret_access_key": "secret...",
    })

    # Scoped (nested dict)
    bundle = await bb.create("my/data", config={
        "region": "us-west-2",                    # default for all providers
        "/s3/prod-bucket": {                       # override for specific bucket
            "endpoint": "http://localhost:9000",
        }
    })
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    bundle = bb.create("my/data", config={
        "region": "us-west-2",
        "access_key_id": "AKIA...",
    })
    ```

### Environment Variables (Medium Priority)

Set `BB_*` environment variables. These apply to all bundles in the process.

```bash
# Global default
export BB_REGION=us-west-2

# Named scope (applies to a specific config scope)
export BB_PROD__REGION=us-east-1
export BB_PROD__ENDPOINT=http://localhost:9000

# Named scope with explicit SCOPE_ prefix (equivalent to above)
export BB_SCOPE_PROD__REGION=us-east-1
```

### Stored Config (Lowest Priority)

Use `SAVE CONFIG` to persist configuration in the bundle manifest. These values are saved when you commit and apply every time the bundle is opened.

=== "Async API"

    ```python
    await bundle.save_config("region", value="us-west-2")
    await bundle.save_config("endpoint", value="http://minio:9000", scope="/s3/data")
    await bundle.save_config("region", value="eu-west-1", scope="/prod")
    await bundle.commit("Add storage config")
    ```

=== "Sync API"

    ```python
    bundle.save_config("region", value="us-west-2")
    bundle.save_config("endpoint", value="http://minio:9000", scope="/s3/data")
    bundle.commit("Add storage config")
    ```

=== "SQL"

    ```sql
    SAVE CONFIG region = 'us-west-2'
    SAVE CONFIG endpoint = 'http://minio:9000' FOR '/s3/data'
    SAVE CONFIG region = 'eu-west-1' FOR '/prod'
    COMMIT 'Add storage config'
    ```

## Scope Format

Scopes are `/`-separated paths that identify which config values apply to which storage locations. The global scope `/` matches everything. Scopes always start with `/` and never contain `://`.

| Scope | Meaning |
|-------|---------|
| `/` | Global default (matches everything) |
| `/s3/my-bucket` | Matches `s3://my-bucket` and anything under it |
| `/s3/my-bucket/subfolder` | Matches `s3://my-bucket/subfolder` and below |
| `/prod` | Named scope alias (if defined via `create_scope_alias`) |

## Config Key Patterns

All config sources support scoping keys to specific scopes. The syntax varies by source:

| Pattern | Runtime Config | Passed Config | Environment Variable | Stored Config |
|---------|---------------|--------------|---------------------|---------------|
| **Global default** | `SET CONFIG key = 'val'` | `{"key": "val"}` | `BB_KEY=val` | `SAVE CONFIG key = 'val'` |
| **URL-scoped** | `SET CONFIG key = 'val' FOR '/s3/bucket'` | `{"/s3/bucket": {"key": "val"}}` | — | `SAVE CONFIG key = 'val' FOR '/s3/bucket'` |
| **Named scope** | `SET CONFIG key = 'val' FOR '/name'` | `{"name__key": "val"}` or `{"scope_name__key": "val"}` | `BB_NAME__KEY=val` or `BB_SCOPE_NAME__KEY=val` | `SAVE CONFIG key = 'val' FOR '/name'` |

### Flat-Key Patterns in Passed Config

When passing a dict to `create()` or `open()`, you can use flat keys with double-underscore (`__`) separators to reference named scopes:

```python
# Named scope reference (requires a scope named "prod" to exist)
config_flat = {
    "prod__region": "us-west-2",
    "prod__endpoint": "http://localhost:9000",
}

# Equivalent with explicit scope_ prefix
config_flat_explicit = {
    "scope_prod__region": "us-west-2",
    "scope_prod__endpoint": "http://localhost:9000",
}
```

Named scope keys resolve when the bundle has a matching scope alias (created via `create_scope_alias`). Keys referencing unknown scope aliases are silently ignored.

The flat-key syntax mirrors environment variable patterns (without the `BB_` prefix), making it easy to move configuration between env vars and passed config.

### Scope Resolution

Global keys are resolved immediately. Named scope keys (`name__key` or `scope_name__key`) are resolved dynamically -- the scope alias doesn't need to exist when the config is passed, only when config values are actually used (after `create_scope_alias` has been called or when opening a bundle that already has scope aliases defined).

When a URL is accessed, config is resolved using longest-prefix matching:

1. Start with global defaults (scope `/`)
2. Apply the longest matching scope-prefix override

For example, if you have config for both `/s3` and `/s3/my-bucket/subfolder`, a request for `/s3/my-bucket/subfolder/data.csv` uses the more specific config.

## Scope Aliases

Scope aliases are named shortcuts for scopes. They let you assign a short name (like `prod` or `staging`) to a scope, then use that name in config keys instead of repeating the full scope path.

=== "Async API"

    ```python
    # Create a scope alias
    await bundle.create_scope_alias("prod", "/s3/prod-bucket")

    # Use the alias in stored config
    await bundle.save_config("region", value="us-east-1", scope="/prod")
    ```

=== "Sync API"

    ```python
    bundle.create_scope_alias("prod", "/s3/prod-bucket")
    bundle.save_config("region", value="us-east-1", scope="/prod")
    ```

=== "SQL"

    ```sql
    CREATE SCOPE ALIAS prod = '/s3/prod-bucket'
    SAVE CONFIG region = 'us-east-1' FOR '/prod'
    ```

Scope aliases are persisted in the bundle manifest and survive across commits. You can reference them in environment variables (`BB_PROD__REGION` or `BB_SCOPE_PROD__REGION`) or passed config (`{"prod__region": "us-east-1"}` or `{"scope_prod__region": "us-east-1"}`).

## Provider-Specific Keys

Each storage provider accepts specific configuration keys. Global defaults are not validated, but keys scoped to a URL prefix are checked against the provider's allowed keys.

### S3 (`s3://`)

| Key | Description |
|-----|-------------|
| `region` | AWS region (e.g., `us-west-2`) |
| `access_key_id` | AWS access key ID |
| `secret_access_key` | AWS secret access key |
| `session_token` | AWS session token (temporary credentials) |
| `endpoint` | Custom endpoint URL (for S3-compatible services like MinIO) |
| `bucket` | Bucket name |
| `allow_http` | Allow HTTP (non-HTTPS) connections (`true`/`false`) |
| `skip_signature` | Skip request signing (`true`/`false`) |
| `virtual_hosted_style_request` | Use virtual hosted-style requests (`true`/`false`) |
| `token` | Authentication token |
| `imdsv1_fallback` | Allow IMDSv1 fallback (`true`/`false`) |
| `metadata_endpoint` | Custom metadata endpoint |
| `container_credentials_relative_uri` | ECS container credentials URI |
| `unsigned_payload` | Send unsigned payloads (`true`/`false`) |
| `checksum_algorithm` | Checksum algorithm to use |
| `copy_if_not_exists` | Copy-if-not-exists behavior |
| `conditional_put` | Conditional put behavior |

### Google Cloud Storage (`gs://`)

| Key | Description |
|-----|-------------|
| `service_account_key` | JSON service account key (inline) |
| `service_account_path` | Path to service account key file |
| `bucket` | Bucket name |
| `application_credentials` | Application default credentials path |

### Azure Blob Storage (`azure://`)

| Key | Description |
|-----|-------------|
| `account` | Storage account name |
| `access_key` | Storage account access key |
| `container` | Container name |
| `sas_token` | Shared access signature token |
| `bearer_token` | Bearer token |
| `client_id` | Service principal client ID |
| `client_secret` | Service principal client secret |
| `tenant_id` | Azure AD tenant ID |
| `authority_host` | Azure AD authority host |
| `use_emulator` | Use Azurite emulator (`true`/`false`) |

### FTP (`ftp://`)

| Key | Description |
|-----|-------------|
| `username` | FTP username |
| `password` | FTP password |

### SFTP (`sftp://`)

| Key | Description |
|-----|-------------|
| `key_path` | Path to SSH private key file |

### Kaggle (`kaggle://`)

| Key | Description |
|-----|-------------|
| `base_url` | Kaggle API base URL (default: `https://www.kaggle.com`) |
| `username` | Kaggle username |
| `key` | Kaggle API key |
