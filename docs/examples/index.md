---
title: Examples — Bundlebase
description: "Practical Bundlebase examples: basic operations, data ingestion patterns, persistent rules, and complete end-to-end scenarios."
---

# Examples

Focused, copy-paste examples organized by task. All examples use the sync API — swap `import bundlebase.sync as bb` for `import bundlebase as bb` and add `await` if you're in an async context.

| Example | What it covers |
|---------|---------------|
| [Basic Operations](basic-operations.md) | Create, attach, transform, query, export, version |
| [Data Ingestion](data-ingestion.md) | Sources, fetch modes, HTTP/S3/SFTP, multi-source pipelines |
| [Persistent Rules](persistent-rules.md) | `always_delete` and `always_update` for automatic data hygiene |
| [Real-World Scenarios](real-world.md) | Complete end-to-end examples for common use cases |
| [Claude Code History](claude-history.md) | Full example: Go FFI connector + bundled source archive, ready for `FETCH` |
