#!/bin/bash
set -e

echo "Building Bundlebase documentation..."

# Ensure docs dependencies are installed
echo "Installing documentation dependencies..."
poetry install --with docs

# Build Rust documentation
echo "Generating Rust API documentation..."
cargo doc --no-deps --package bundlebase --package bundlebase-python

# Build MkDocs site
echo "Building MkDocs site..."
poetry run mkdocs build

# Copy Rust docs to site/rust/ (not docs/rust/ - keep source clean)
echo "Copying Rust documentation to site/rust/..."
mkdir -p site/rust
cp -r target/doc/* site/rust/

echo "Documentation built successfully!"
echo "Output: site/"
echo ""
echo "To serve locally: poetry run mkdocs serve"
