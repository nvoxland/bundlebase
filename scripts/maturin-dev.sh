#!/bin/bash
# Wrapper for maturin develop that uses a separate target directory
# This prevents full rebuilds when switching between maturin and cargo builds

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$PROJECT_ROOT"

echo "Building Python package with maturin (using target/maturin)..."
exec maturin develop --target-dir target/maturin "$@"
