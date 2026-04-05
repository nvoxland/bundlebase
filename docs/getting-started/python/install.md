---
title: Python Install — Bundlebase
description: Install Bundlebase as a Python library for scripts, notebooks, and applications.
---

# Python Install

Requires Python 3.13+.

=== "pip"

    ```bash
    pip install bundlebase
    ```

=== "pip (with pandas)"

    ```bash
    pip install "bundlebase[pandas]"
    ```

=== "Poetry"

    ```bash
    poetry add bundlebase
    ```

=== "Jupyter"

    ```bash
    pip install "bundlebase[jupyter]"
    ```

pandas, polars, and numpy are optional — install whichever you export to. The base install includes only PyArrow.

## Next step

[Python Quick Start](quick-start.md) — create a bundle, attach data, query with SQL.
