---
title: CLI Install — Bundlebase
description: Install the Bundlebase CLI binary. No Python or other runtime required.
---

# CLI Install

No Python or other runtime required — the CLI is a self-contained binary.

=== "Linux (x86_64)"

    ```bash
    curl -L https://github.com/nvoxland/bundlebase/releases/latest/download/bundlebase-x86_64-unknown-linux-gnu.tar.gz | tar xz
    sudo mv bundlebase /usr/local/bin/
    bundlebase --help
    ```

=== "macOS (Apple Silicon)"

    ```bash
    curl -L https://github.com/nvoxland/bundlebase/releases/latest/download/bundlebase-aarch64-apple-darwin.tar.gz | tar xz
    sudo mv bundlebase /usr/local/bin/
    bundlebase --help
    ```

=== "macOS (Intel)"

    ```bash
    curl -L https://github.com/nvoxland/bundlebase/releases/latest/download/bundlebase-x86_64-apple-darwin.tar.gz | tar xz
    sudo mv bundlebase /usr/local/bin/
    bundlebase --help
    ```

=== "Windows (x86_64)"

    Download [`bundlebase-x86_64-pc-windows-msvc.zip`](https://github.com/nvoxland/bundlebase/releases/latest/download/bundlebase-x86_64-pc-windows-msvc.zip), extract, and add the directory to your PATH.

Or download manually from the [GitHub releases page](https://github.com/nvoxland/bundlebase/releases):

| Platform | File |
|---|---|
| Linux (x86_64) | `bundlebase-x86_64-unknown-linux-gnu.tar.gz` |
| macOS (Apple Silicon) | `bundlebase-aarch64-apple-darwin.tar.gz` |
| macOS (Intel) | `bundlebase-x86_64-apple-darwin.tar.gz` |
| Windows (x86_64) | `bundlebase-x86_64-pc-windows-msvc.zip` |

Extract and move the binary onto your PATH:

```bash
# macOS / Linux
tar xz bundlebase-*.tar.gz
sudo mv bundlebase /usr/local/bin/

# verify
bundlebase --help
```

## Next step

[CLI Quick Start](quick-start.md) — create a bundle and query it interactively from the REPL.
