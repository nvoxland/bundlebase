# CLI Installation

The Bundlebase CLI provides an interactive REPL and an Arrow Flight SQL server for working with bundles from the command line.

## Requirements

- macOS, Linux, or Windows
- No Python or other runtime required — the CLI is a standalone binary

## Download

Download the latest `bundlebase-cli` binary from the [GitHub releases page](https://github.com/nvoxland/bundlebase/releases).

Choose the archive matching your platform:

| Platform | File |
|---|---|
| macOS (Apple Silicon) | `bundlebase-cli-aarch64-apple-darwin.tar.gz` |
| macOS (Intel) | `bundlebase-cli-x86_64-apple-darwin.tar.gz` |
| Linux (x86_64) | `bundlebase-cli-x86_64-unknown-linux-gnu.tar.gz` |
| Windows (x86_64) | `bundlebase-cli-x86_64-pc-windows-msvc.zip` |

Extract the archive and place the `bundlebase-cli` binary somewhere on your system.

## Verify Installation

```bash
bundlebase-cli --help
```

You should see output describing the available flags and modes.

## Add to PATH

To run `bundlebase-cli` from any directory, add its location to your `PATH`:

```bash
# Example: move to a directory already on your PATH
mv bundlebase-cli /usr/local/bin/

# Or add the directory to your PATH
export PATH="$PATH:/path/to/bundlebase-cli"
```

## Next Steps

Now that the CLI is installed, continue to the [CLI Quick Start](cli-quick-start.md) guide.
