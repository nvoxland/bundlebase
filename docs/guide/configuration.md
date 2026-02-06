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
    await bundle.set_config("s3", "region", "us-west-2")
    await bundle.set_config("s3/data", "endpoint", "http://localhost:9000")
    ```

=== "Sync API"

    ```python
    bundle.set_config("s3", "region", "us-west-2")
    bundle.set_config("s3/data", "endpoint", "http://localhost:9000")
    ```

=== "SQL"

    ```sql
    SET CONFIG region = 'us-west-2' FOR 's3'
    SET CONFIG endpoint = 'http://localhost:9000' FOR 's3/data'
    ```

### Passed Config (High Priority)

Pass a config dict to `create()` or `open()`. These values take effect for the current session only and are not persisted.

=== "Async API"

    ```python
    import bundlebase as bb

    # Scoped to specific provider
    bundle = await bb.create("my/data", config={
        "s3": {
            "region": "us-west-2",
            "access_key_id": "AKIA...",
            "secret_access_key": "secret...",
        }
    })

    # Multiple scopes with override
    bundle = await bb.create("my/data", config={
        "s3": {"region": "us-west-2"},            # default for S3
        "s3/prod-bucket": {                        # override for specific bucket
            "endpoint": "http://localhost:9000",
        }
    })
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    bundle = bb.create("my/data", config={
        "s3": {
            "region": "us-west-2",
            "access_key_id": "AKIA...",
        }
    })
    ```

### Environment Variables (Medium Priority)

Set `BB_*` environment variables. These apply to all bundles in the process.

```bash
# Global default
export BB_REGION=us-west-2

# Scoped (applies to a specific config scope)
export BB_S3__REGION=us-east-1
export BB_S3__MY_BUCKET__ENDPOINT=http://localhost:9000
```

### Stored Config (Lowest Priority)

Use `SAVE CONFIG` to persist configuration in the bundle manifest. These values are saved when you commit and apply every time the bundle is opened.

=== "Async API"

    ```python
    await bundle.save_config("s3", "region", "us-west-2")
    await bundle.save_config("s3/data", "endpoint", "http://minio:9000")
    await bundle.commit("Add storage config")
    ```

=== "Sync API"

    ```python
    bundle.save_config("s3", "region", "us-west-2")
    bundle.save_config("s3/data", "endpoint", "http://minio:9000")
    bundle.commit("Add storage config")
    ```

=== "SQL"

    ```sql
    SAVE CONFIG region = 'us-west-2' FOR 's3'
    SAVE CONFIG endpoint = 'http://minio:9000' FOR 's3/data'
    COMMIT 'Add storage config'
    ```

## Scope Format

Scopes are `/`-separated paths that identify which config values apply to which storage locations. Each scope matches its own name and any child paths.

| Scope | Meaning |
|-------|---------|
| `system` | Bundlebase-level settings (e.g., `max_memory`, `catalog_name`) |
| `s3` | Matches all S3 URLs |
| `s3/my-bucket` | Matches `s3://my-bucket` and anything under it |
| `s3/my-bucket/subfolder` | Matches `s3://my-bucket/subfolder` and below |
## Config Key Patterns

All config sources support scoping keys to specific scopes. The syntax varies by source:

| Pattern | Runtime Config | Passed Config | Environment Variable | Stored Config |
|---------|---------------|--------------|---------------------|---------------|
| **Provider default** | `SET CONFIG key = 'val' FOR 's3'` | `{"s3": {"key": "val"}}` | `BB_S3__KEY=val` | `SAVE CONFIG key = 'val' FOR 's3'` |
| **URL-scoped** | `SET CONFIG key = 'val' FOR 's3/bucket'` | `{"s3/bucket": {"key": "val"}}` | `BB_S3__BUCKET__KEY=val` | `SAVE CONFIG key = 'val' FOR 's3/bucket'` |

### Environment Variable Scoping

Environment variables use double-underscore (`__`) to separate scope segments from the config key. The suffix after `BB_` is split on `__`: the last segment becomes the config key, and any preceding segments form the scope path (joined with `/`).

| Environment Variable | Scope | Key |
|---------------------|-------|-----|
| `BB_REGION=us-west-2` | global | `region` |
| `BB_S3__REGION=us-west-2` | `s3` | `region` |
| `BB_S3__MY_BUCKET__KEY=val` | `s3/my_bucket` | `key` |

### Scope Resolution

When a URL is accessed, config is resolved using longest-prefix matching:

1. Start with the provider's default scope (e.g., `s3`)
2. Apply the longest matching scope-prefix override

For example, if you have config for both `s3` and `s3/my-bucket/subfolder`, a request for `s3://my-bucket/subfolder/data.csv` uses the more specific config.

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

### System (`system`)

| Key | Description |
|-----|-------------|
| `max_memory` | Maximum memory for query execution |
| `catalog_name` | Name of the DataFusion catalog |
