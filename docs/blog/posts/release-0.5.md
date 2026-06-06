---
date: 2026-02-08
categories:
  - Releases
---

# Bundlebase 0.5.0

You can now pull datasets directly from Kaggle. This release also overhauls the configuration system to support scoped config with multiple priority layers.

<!-- more -->

## Overview

We're getting closer to a real, usable project. This release is partly about adding Kaggle support as a source, 
but even more about continuing to clean up the source and configuration logic.

## New Features

### Kaggle source

Kaggle datasets are now a first-class source. Point it at a dataset, fetch, and you've got data:

```python
import bundlebase.sync as bb

bundle = bb.create("housing/data").create_source("kaggle", {
    "dataset": "zillow/zecon",
    "patterns": "*.csv"
})

bundle.fetch("base", "add")
bundle.commit("Loaded Zillow data from Kaggle")
```

It handles ZIP extraction automatically, supports glob patterns for filtering files, and tracks dataset versions so re-fetches only pull what's changed.

Auth uses your existing `~/.kaggle/kaggle.json` by default. You can also configure credentials through Bundlebase's config system under the `kaggle` scope:

```python
bundle.set_config("kaggle", "username", "my-user")
bundle.set_config("kaggle", "key", "my-api-key")
```

### Configuration overhaul

Config is now scoped and layered. Four sources, highest priority wins:

```
runtime config (set_config)  >  passed config  >  env vars  >  stored config (save_config)
```

Scopes are `/`-separated paths that match storage URLs. You can set defaults for a provider and override for specific buckets:

```python
# Default for all S3
bundle.set_config("s3", "region", "us-west-2")

# Override for a specific bucket
bundle.set_config("s3/prod-bucket", "endpoint", "http://localhost:9000")
```

`set_config()` is session-only. `save_config()` persists to the bundle manifest and applies every time you open it.

Environment variables work too -- `BB_S3_REGION=us-west-2` for provider-level config, `BB_S3__MY_BUCKET__ENDPOINT=http://localhost:9000` for URL-scoped overrides (double `__` encodes path separators).

See the [configuration guide](../../guide/configuration.md) for the full details.

## Breaking Changes

### Config API now requires a scope

`set_config()` and `save_config()` take a scope as the first argument:

```python
# Before
bundle.set_config("region", "us-west-2")

# After
bundle.set_config("s3", "region", "us-west-2")
```

### Environment variable format changed

Env vars now encode scope in the name. Single `_` separates scope from key, double `__` encodes sub-path separators:

| Variable | Scope | Key |
|----------|-------|-----|
| `BB_S3_REGION` | `s3` | `region` |
| `BB_S3__MY_BUCKET__KEY` | `s3/my_bucket` | `key` |

---

```
pip install bundlebase==0.5.0
```

Let me know if you run into issues, especially with Kaggle auth or the new config scoping.
