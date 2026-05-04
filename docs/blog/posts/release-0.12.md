---
date: 2026-05-03
categories:
  - Releases
---

# Bundlebase 0.12.0

A small release: one real bug fix, smaller python wheels.

<!-- more -->

## Bugs fixed

- DataType fields with tagged-tuple variants (`Timestamp`, `Time32`, `Time64`,
  `Duration`, `Interval`, `Struct`, …) now round-trip through YAML manifests.
  Previously a bundle that referenced one of these types in
  `expectedSchema`, a `CAST COLUMN`, or an `IMPORT FUNCTION` signature
  would fail to open with `"untagged and internally tagged enums do not
  support enum input"`. Plain types (`Int64`, `Utf8`, `Boolean`, etc.)
  were never affected.

## Build & packaging

- Release wheels now use `lto = "fat"`, `strip = "symbols"`, `debug = false`.
- Release only wheels for Python 3.13 and 3.14.

## Installation

**Python package:**

```
pip install bundlebase==0.12.0
```

**CLI binaries** (macOS arm64, Linux x86_64, Windows x86_64) are on the
[releases page](https://github.com/nvoxland/bundlebase/releases).
