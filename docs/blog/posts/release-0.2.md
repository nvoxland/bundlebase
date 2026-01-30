---
date: 2026-01-04
categories:
  - Releases
---

# Bundlebase 0.2.0

## New Features

### Views

The main new feature is views. You can now create named views on a bundle, which are essentially saved forks with their own operation history:

```python
await c.create_view("us_only")
```

Views support the full lifecycle: create, rename, drop. 

## Breaking Changes

### `query` is now `select`

The old `query()` method has been renamed to `select()`. This wasn't just a name change -- `select()` now returns a new forked bundle instead of mutating in place. This makes more sense semantically: you're selecting data out of the bundle, not transforming the bundle itself.

If you were using `query()`, update your code. The old name is gone.

### Manifest Rename

Also renamed the metadata directory from `_manifest` to `_bundlebase`. 

## Other changes

- **Configuration system**: New `BundleConfig` for things like S3 endpoints and regions. Passed to `create()` and `open()`.
- **Command parser**: Added a PEG grammar-based command parser for SQL-style commands. Early days, but it's the foundation for a proper command language.
- **Indexing improvements**: New row ID caching, better column index performance, and a new `drop_index` operation.
- **Documentation site**: Stood up a MkDocs site at [nvoxland.github.io/bundlebase](https://nvoxland.github.io/bundlebase). Getting started guide, API reference, the basics.
- **Build improvements**: Narrowed Tokio features to reduce compile times. Tuned codegen-units and enabled incremental builds.

```
pip install bundlebase==0.2.0
```

Let me know if you run into issues.
