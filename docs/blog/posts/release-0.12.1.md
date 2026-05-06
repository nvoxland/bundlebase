---
date: 2026-05-05
categories:
  - Releases
---

# Bundlebase 0.12.1

A small release: a `--create` flag on the CLI and a cleanup to how the bundle format version is stored.

<!-- more -->

## CLI

- New `--create` flag on `bundlebase repl` and `bundlebase serve`. Creates a
  new bundle at the given path, commits an initial empty state, and then
  drops you into the REPL or starts the Flight server. Saves the
  `bundlebase create` + `bundlebase repl` two-step.

  ```bash
  bundlebase repl --bundle ./orders --create
  bundlebase serve --bundle ./orders --create
  ```

  Errors if a bundle already exists at the path, or if combined with
  `--read-only`.

- When `bundlebase repl` is run against a path that has no bundle yet,
  the error now suggests `--create` instead of pointing at the separate
  `bundlebase create` subcommand. `query`, `extend`, and the other
  read/extend commands still suggest `bundlebase create`, since they
  don't have their own create flag.

## Format version storage

- `setMinVersion` / `setMaxVersion` operations now normalize to
  `major.minor` at construction. Previously a caller passing
  `"1.2.3"` would write that exact string into the manifest, even
  though the runtime check has always ignored the patch. The on-disk
  form now matches the semantics, so manifests don't churn across
  patch releases.

  No behavior change for existing bundles — the runtime check was
  already patch-blind, and `parse_format_version` continues to accept
  either form on read.

## Installation

**Python package:**

```
pip install bundlebase==0.12.1
```

**CLI binaries** (macOS arm64, Linux x86_64, Windows x86_64) are on the
[releases page](https://github.com/nvoxland/bundlebase/releases).
