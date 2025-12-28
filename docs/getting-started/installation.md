# Installation

## Requirements

- Python 3.13 or higher
- pip or poetry package manager

## Install from PyPI

The simplest way to install Bundlebase is via pip:

```bash
pip install bundlebase
```

## Install with Poetry

If you're using Poetry for dependency management:

```bash
poetry add bundlebase
```

## Optional Dependencies

Bundlebase works with multiple data processing libraries. Install the ones you need:

### For Jupyter Notebooks

```bash
pip install "bundlebase[jupyter]"
```

This includes `nest-asyncio` for running async code in Jupyter notebooks.

### Polars Support

```bash
pip install bundlebase polars
```

### NumPy Support

```bash
pip install bundlebase numpy
```

## Verify Installation

Test that Bundlebase is installed correctly:

```python
import bundlebase

print(bundlebase.__version__)
```

## Development Installation

If you want to contribute to Bundlebase or run from source, see the [Development Setup](../development/setup.md) guide.

## Troubleshooting

### Import Errors

If you encounter import errors, ensure you're using Python 3.13 or higher:

```bash
python --version
```

### Rust Extension Issues

Bundlebase includes a Rust extension module. On some systems, you may need to update pip:

```bash
pip install --upgrade pip
pip install bundlebase
```

## Next Steps

Now that Bundlebase is installed, continue to the [Quick Start](quick-start.md) guide to start using it.
