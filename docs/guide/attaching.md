# Attaching Data

Data is added to the container via the `.attach()` method

## Path Resolution

The `attach()` method handles paths flexibly:

- Paths can be any supported URL and the data will be read from there.
- Paths can be relative to the data_dir. But NOT `..` to a parent dir. 

## Supported Formats

- CSV
- JSON Line
- Parquet
