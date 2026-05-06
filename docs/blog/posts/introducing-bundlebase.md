---
date: 2026-05-06
categories:
  - Announcements
---

# Introducing Bundlebase: Versioned Data Bundles

Like Docker, but for data — package a dataset and its transformations into a versioned bundle you can query.

<!-- more -->

Datasets keep getting passed around as loose files and one-off scripts. Someone exports a CSV, somebody else writes a notebook to clean it, a third person reruns half of that pipeline a month later and gets slightly different numbers. The data itself is fine. The wrapper around it is the problem.

Bundlebase gives a dataset the same kind of packaging that container images gave applications: **a single artifact** that holds the data plus the **transformations applied** to it, with a **version history**, that you can **hand to someone else** and know they'll get the same view you did.

## What it looks like

Create a bundle, attach some files, query it:

=== "Python"

    ```python
    import bundlebase.sync as bb

    bundle = bb.create("./orders")
    bundle.attach("jan.csv")
    bundle.attach("feb.csv")
    bundle.commit("attach jan and feb orders")

    df = bundle.sql("""
        SELECT region, SUM(amount) AS total
        FROM orders
        GROUP BY region
        ORDER BY total DESC
    """).to_pandas()
    ```

=== "SQL"

    ```bash
    bundlebase repl --bundle ./orders --create
    ```

    ```sql
    ATTACH 'jan.csv';
    ATTACH 'feb.csv';
    COMMIT 'attach jan and feb orders';

    SELECT region, SUM(amount) AS total
    FROM orders
    GROUP BY region
    ORDER BY total DESC;
    ```

Apply transformations and commit them as part of the bundle:

=== "Python"

    ```python
    bundle.filter("amount > 0")
    bundle.add_column("month", "date_trunc('month', ordered_at)")
    bundle.commit("drop refunds, add month column")
    ```

=== "SQL"

    ```sql
    FILTER amount > 0;
    ADD COLUMN month AS date_trunc('month', ordered_at);
    COMMIT 'drop refunds, add month column';
    ```

Now anyone who opens that bundle gets the cleaned, versioned dataset — no notebook to rerun, no shared drive folder of `final_v3_real.parquet`.

The same surface is also available over Arrow Flight, so it isn't tied to any one client.

## Connectors: bundles know where their data came from

A bundle isn't just a snapshot of bytes. When you `ATTACH` from a connector — a Postgres table, an HTTP API, a vendor export, a JSONL directory — the bundle records the source it came from along with the transformations applied. Pulling fresh data later is `FETCH`, not "find the original script and re-run it." The provenance lives inside the bundle, and you control when and how the bundle re-syncs from upstream.

## Remix and extend, like base images

Bundles compose. You can open someone else's published bundle, layer your own filters, joins, or derived columns on top, and commit a new bundle that points back at the original as its parent. The shared cleanup work — schema normalization, deduping, regional mappings — gets done once and reused, the same way `FROM ubuntu:24.04` is the start of half the Dockerfiles on the internet, not a copy-paste of every line.

## Local or remote, same bundle

A bundle is just a directory layout, so it works the same whether it lives on your laptop, on a shared NFS mount, or in object storage. `bb.open("s3://bucket/orders")`, `bb.open("az://container/orders")`, `bb.open("gs://bucket/orders")`, and `bb.open("./orders")` are interchangeable — the SQL CLI, Python API, and Flight server don't care which one you point them at. That means a bundle you build locally can be published to S3 (or Azure Blob, or GCS) and queried in place by anyone with read access, no copy-down step, no separate "remote mode."

## AI-agent friendly by design

Most data tools were built assuming a human is in the loop reading column descriptions and remembering which CSV is the "real" one. Agents don't have that context, and they hallucinate when forced to guess. Bundlebase is built so an agent can introspect a bundle the same way a human would: schema, version history with commit messages, source connectors, applied transformations, sample rows, and indexes are all queryable. Operations are explicit and reversible — the agent commits a transformation, sees the diff, and can roll back instead of silently corrupting the dataset. There is an MCP mode and an Arrow Flight surface so agents can both **collect** data (attach sources, fetch, commit) and **analyze** it (SQL, streaming exports) without needing to invent file paths or remember conventions.

## What problems it's actually trying to solve

- **Reproducible handoffs.** "Here's the bundle at v4" is a complete answer. There is no separate cleaning script to find.
- **Local-first analysis.** It runs on your laptop, against local files or S3, without a warehouse or scheduler in the loop.
- **Datasets bigger than RAM.** The query engine is DataFusion + Arrow, and execution streams end-to-end. `to_pandas()` and `to_polars()` stream internally instead of materializing the whole result.
- **One artifact across languages.** Rust core, Python bindings, SQL CLI, Flight server. Same bundle, same answers.

## What it isn't

- Not a warehouse. There's no hosted service, no cluster, no cost model.
- Not an orchestrator. It doesn't schedule pipelines or trigger jobs.
- Not a notebook replacement. It's the thing the notebook reads from and writes to.
- Not a data catalog. A bundle is an artifact; a catalog of bundles is something you'd build on top.

## Status

We've not yet released the stable 1.0 version, and so I'm still reserving the right to introduce incompatibilities between versions.

However, the bulk of the functionality I'm expecting to be there for the 1.0 version should be there, and I'm expecting mainly polish and performance work heading to 1.0.


## Try it

Python:

```bash
pip install bundlebase
```

Or grab the standalone CLI binary — no Python or other runtime required:

=== "Linux (x86_64)"

    ```bash
    curl -L https://github.com/nvoxland/bundlebase/releases/latest/download/bundlebase-x86_64-unknown-linux-gnu.tar.gz | tar xz
    sudo mv bundlebase /usr/local/bin/
    ```

=== "macOS (Apple Silicon)"

    ```bash
    curl -L https://github.com/nvoxland/bundlebase/releases/latest/download/bundlebase-aarch64-apple-darwin.tar.gz | tar xz
    sudo mv bundlebase /usr/local/bin/
    ```

=== "macOS (Intel)"

    ```bash
    curl -L https://github.com/nvoxland/bundlebase/releases/latest/download/bundlebase-x86_64-apple-darwin.tar.gz | tar xz
    sudo mv bundlebase /usr/local/bin/
    ```

=== "Windows (x86_64)"

    Download [`bundlebase-x86_64-pc-windows-msvc.zip`](https://github.com/nvoxland/bundlebase/releases/latest/download/bundlebase-x86_64-pc-windows-msvc.zip), extract, and add the directory to your PATH.

Other platforms and manual downloads are on the [GitHub releases page](https://github.com/nvoxland/bundlebase/releases).

- Quickstart: [Getting Started](../../getting-started/basic-concepts.md)
- CLI install: [CLI Install](../../getting-started/cli/install.md)
- Examples: [Examples](../../examples/index.md)

## What is next?

That depends on you: what use cases of yours should be addressed by Bundlebase but are not? How can it better work for you? Where does it fall short?  

[Open an issue](https://github.com/nvoxland/bundlebase/issues) and let us know. Or even better: send a pull request!