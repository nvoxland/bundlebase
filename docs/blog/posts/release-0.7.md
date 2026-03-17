---
date: 2026-03-12
categories:
  - Releases
---

# Bundlebase 0.7.0

This release adds custom connectors, user-defined functions, and column operations. The big theme: you can now extend Bundlebase with your own code.

<!-- more -->

## New Features

### Custom Connectors

You can now write your own data connectors in Python, Go, Java, Rust, or anything that runs in Docker. Connectors define how to connect to a data source — you import a connector, create source instances from it, then fetch data.

```python
# Import a connector (persisted into the bundle)
bundle.import_connector('acme.weather', 'ipc::./my_connector')

# Create a source instance from it
bundle.create_source('acme.weather', {'region': 'us-east'})

# Fetch the data
bundle.fetch("base", "add")
```

Multiple runners are available depending on your needs:

- **`python`** — in-process, zero-copy Arrow transfer (use `import_temp_connector` since Python code can't be serialized into the bundle)
- **`lib`** — load a compiled shared library via `dlopen`, also zero-copy
- **`ipc`** — run any executable as a subprocess
- **`java`** — run a JAR file
- **`docker`** — run a container image

SDKs for Python, Go, Java, and Rust handle the IPC protocol for you. Here's a complete Python connector:

```python
from bundlebase_sdk import Connector, Location, serve
import pyarrow as pa

class WeatherConnector(Connector):
    def discover(self, attached_locations, **kwargs):
        return [Location("forecast.parquet", format="parquet", version="v1")]

    def data(self, location, **kwargs):
        return pa.table({"city": ["NYC", "LA"], "temp_f": [45, 72]})

if __name__ == "__main__":
    serve(WeatherConnector())
```

To clarify terminology: **connectors** are the new concept here — they define *how* to connect to data. **Sources** are instances created from connectors (or from built-in connectors like `remote_dir` and `kaggle`, which have always been there). This isn't a rename.

See the [custom connectors guide](../../guide/custom-connectors/index.md) for the full details.

### User-Defined Functions

Extend Bundlebase's SQL with your own scalar and aggregate functions. Same runner options as connectors — `python`, `lib`, `ipc`, `java`, `docker`.

```python
# Import and use a function
bundle.import_temp_function("tools.double_val", "ipc::python:my_functions.py")
bundle.query("SELECT tools.double_val(amount) FROM bundle")
```

Functions support overloading — same name, different type signatures. And if your function module exports a `bundlebase_metadata()` function (or the equivalent manifest in compiled languages), Bundlebase can auto-discover all the functions without you specifying types:

```sql
-- Import all functions from a module, types auto-detected
IMPORT TEMP FUNCTION tools.* FROM 'ipc::python:my_functions.py'
```

Aggregate UDFs work too — implement `create_state`, `accumulate`, `merge`, and `evaluate`, and you can use them with `GROUP BY` like any built-in aggregate.

See the [functions guide](../../guide/functions.md).

### Column Operations

Three new operations for wrangling messy columns:

- **`standardize_column_names()`** — normalizes column names to lowercase, underscore-separated identifiers. `"Customer Id"` becomes `customer_id`, `"Phone 1"` becomes `phone_1`.

    ```python
    bundle.standardize_column_names()
    ```

- **`add_column(name, expr)`** — creates a computed column from a SQL expression. Even a SQL expression using your custom functions.

    ```python
    bundle.add_column("full_name", "first_name || ' ' || last_name")
    ```

- **`cast_column(name, type)`** — changes a column's data type, with optional regex cleaning to strip junk before conversion.

    ```python
    bundle.cast_column("price", "integer", clean="[^0-9]")
    ```

### Drop Commands

You can now remove connectors and sources you no longer need:

```sql
DROP CONNECTOR acme.weather
DROP SOURCE acme.weather
```

Also available as `bundle.drop_connector()` in Python.

### External Code Security

Running custom connectors and functions means executing external code, so there's a new `allow_external_code` config setting that defaults to `false`. You need to opt in:

```python
config = {"system": {"allow_external_code": "true"}}
bundle = bb.create("my/data", config=config)
```

## Other Changes

- **Object store support for tar URLs** — bundle tar files can now live on S3, Azure Blob, and GCS
- **IPC protocol hardening** — fixed a deadlock in the subprocess protocol, improved type safety
- **SDK improvements** — better error handling, complex type support, `DRY RUN` and `DESCRIBE CONNECTOR`/`DESCRIBE FUNCTION` commands
- **`bundlebase init-sdk`** — scaffold a new connector or function project with `bundlebase init-sdk python my_project --type connector`
- **CI upgraded to GitHub Actions Node 24**

---

```
pip install bundlebase==0.7.0
```

Give the connector and function system a try — I'm curious how people end up using it. Let me know if you run into issues.
