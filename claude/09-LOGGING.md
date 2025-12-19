# Logging

Bundlebase provides seamless logging from Rust code to Python using the `pyo3-log` bridge, and logging in the REPL using `tracing-subscriber` with CLI-configurable log levels.

## Rust→Python Logging Bridge

All Rust code uses the standard `log` crate macros (`log::info!()`, `log::debug!()`, `log::warn!()`, `log::error!()`). When the Python extension is loaded, these logs are automatically forwarded to Python's `logging` module via the `pyo3-log` bridge.

### How It Works

1. Rust code uses standard `log` macros: `info!()`, `debug!()`, `warn!()`, `error!()`
2. The `pyo3-log` crate (initialized on module import) forwards these to Python's `logging` module
3. Logs appear under the logger name `Bundlebase.rust`
4. Python code can configure filtering, formatting, and handlers using standard Python logging

### Python Configuration

Users can configure the Rust logger like any Python logger:

```python
import logging
import Bundlebase

# Option 1: Use the helper function (simplest)
Bundlebase.set_rust_log_level(logging.DEBUG)

# Option 2: Configure the logger directly
logging.getLogger('Bundlebase.rust').setLevel(logging.INFO)

# Option 3: Use basicConfig for all loggers
logging.basicConfig(level=logging.INFO)

# Option 4: Add a custom handler
rust_logger = logging.getLogger('Bundlebase.rust')
file_handler = logging.FileHandler('rust.log')
file_handler.setFormatter(logging.Formatter('%(asctime)s - %(levelname)s - %(message)s'))
rust_logger.addHandler(file_handler)
```

### Rust Usage

```rust
use log::{debug, info, warn, error};

info!("Container initialized: {}", name);
debug!("Processing {} rows", count);
warn!("Large dataset detected: {} rows", count);
error!("Failed to load: {}", err);
```

### Log Output Example

When logs are emitted from Rust and captured by Python:

```
INFO [Bundlebase.rust] Container initialized: mydata
DEBUG [Bundlebase.rust] Processing 1000000 rows
WARNING [Bundlebase.rust] Large dataset detected: 1000000 rows
ERROR [Bundlebase.rust] Failed to load: file not found
```

### Default Behavior

By default, the `Bundlebase.rust` logger is configured with:
- **Level**: INFO (shows INFO, WARN, ERROR; hides DEBUG, TRACE)
- **Handler**: StreamHandler to stderr
- **Formatter**: `%(levelname)s [%(name)s] %(message)s`

This means:
- INFO and higher logs appear by default when importing Bundlebase
- DEBUG logs are hidden unless explicitly enabled
- Integration with Python logging frameworks (like in Jupyter notebooks) works automatically

### Integration with Python Logging

The Rust logger integrates seamlessly with Python's logging ecosystem:

```python
import logging
import Bundlebase

# Configure logging at application startup
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s',
    handlers=[
        logging.StreamHandler(),
        logging.FileHandler('app.log')
    ]
)

# Now both Python and Rust logs use the same configuration
# Rust logs will appear in the file and console
```

## REPL Logging

The `Bundlebase-server` REPL displays Rust logs using the `ui` log level by default, which shows a minimal format (message only) for clean interactive output. The log level is fully configurable via the `--log-level` CLI argument.

### How It Works

1. The REPL binary (`Bundlebase-server`) uses `tracing-subscriber` as the logging backend
2. The `tracing-log` crate bridges `log::*` macros to `tracing-subscriber`
3. All Rust logs from the Bundlebase library are captured and displayed
4. Log level is configured via the `--log-level` CLI argument
5. Default level: `ui` (minimal format, shows INFO and higher messages without timestamps)

### Usage

```bash
# Default: UI mode (minimal format, message only)
Bundlebase-server --container mydata --repl

# Show DEBUG logs with full format (timestamps, module names, levels)
Bundlebase-server --container mydata --repl --log-level debug

# Show TRACE logs (most verbose with full format)
Bundlebase-server --container mydata --repl --log-level trace

# Show only INFO logs with full format
Bundlebase-server --container mydata --repl --log-level info

# Show only WARN and ERROR with full format
Bundlebase-server --container mydata --repl --log-level warn

# Show only ERROR with full format
Bundlebase-server --container mydata --repl --log-level error
```

### Log Levels

The `--log-level` argument accepts the following values (case-insensitive):
- `ui` - Minimal format (message only), INFO level - good for interactive use (default)
- `trace` - Most verbose, shows all logs with full format (timestamps, module names, levels)
- `debug` - Show debug information with full format
- `info` - Show informational messages with full format
- `warn` or `warning` - Show warnings and errors only with full format
- `error` - Show errors only with full format

### Log Format Examples

**UI mode (default):**
```
Creating container at: memory:///mydata
Loading schema
Attached data.parquet
```

**Debug/Info/Trace modes (full format with timestamps and module names):**
```
2024-12-12T10:30:45.123456Z  INFO Bundlebase_server: Creating container at: memory:///mydata
2024-12-12T10:30:45.456789Z  DEBUG bundlebase::functions: Creating FunctionRegistry
2024-12-12T10:30:45.789012Z  INFO bundlebase::schema: Loading schema
2024-12-12T10:31:00.012345Z  WARN bundlebase::query: Large result set: 1000000 rows
```

## Implementation Details

### Python Bindings (pyo3-log)

- Uses the `pyo3-log` crate to bridge Rust logs to Python
- Initialized automatically when the `_Bundlebase` module is imported
- Handles GIL acquisition and thread safety transparently
- Minimal overhead: zero cost for filtered logs
- Log levels mapped:
  - `log::Level::Error` → `logging.ERROR`
  - `log::Level::Warn` → `logging.WARNING`
  - `log::Level::Info` → `logging.INFO`
  - `log::Level::Debug` → `logging.DEBUG`
  - `log::Level::Trace` → `logging.DEBUG`

### REPL (tracing-log + tracing-subscriber)

- Uses `tracing-log` to bridge `log` crate logs to `tracing`
- Uses `tracing-subscriber` to format and display the logs
- Configured at startup in `Bundlebase-server/src/main.rs`
- Log level is set via the `--log-level` CLI argument with default "ui"
- Default level: `ui` mode (shows INFO and higher with minimal formatting)
- `ui` mode configuration: Disables timestamps, log levels, target/module names, and thread info
- Other modes: Show full format with timestamps, module names, and log levels
- Handles concurrent logging safely with no performance overhead for filtered logs

## Debugging Tips

### Python: Check if logs are being captured

```python
import logging
import Bundlebase

# Enable debug logging
logging.basicConfig(level=logging.DEBUG)
Bundlebase.set_rust_log_level(logging.DEBUG)

# Now perform operations - you should see DEBUG logs from Rust
```

### REPL: Check if logs are displayed

```bash
# Run with DEBUG level
Bundlebase-server --container mydata --repl --log-level debug

# You should see DEBUG logs like:
# 2024-12-12T10:30:45.123456Z DEBUG bundlebase::builder: ...
```

### Verify pyo3-log is initialized

```python
import logging

# After importing Bundlebase, the logger should exist
rust_logger = logging.getLogger('Bundlebase.rust')
print(f"Logger level: {rust_logger.level}")
print(f"Has handlers: {len(rust_logger.handlers) > 0}")
```

## Performance

- **Zero overhead** for filtered logs: If a log level is not enabled, no string formatting occurs
- **Minimal impact** for enabled logs: Small overhead from Python->Rust bridge (microseconds per log)
- **No GIL contention**: pyo3-log handles GIL acquisition efficiently
- **No blocking**: Logging does not block the main thread

## Common Issues

### Logs not appearing in Python

**Problem**: You're not seeing any Rust logs in your Python script.

**Solutions**:
1. Make sure you imported Bundlebase: `import Bundlebase`
2. Set the log level: `Bundlebase.set_rust_log_level(logging.DEBUG)`
3. Configure basicConfig before using Bundlebase:
   ```python
   import logging
   logging.basicConfig(level=logging.DEBUG)
   import Bundlebase
   ```

### Logs appearing in Jupyter but not in console script

**Problem**: Logs work in Jupyter but not when running a script directly.

**Solution**: Jupyter has logging pre-configured. In a regular script, you need to configure logging:
```python
import logging
logging.basicConfig(level=logging.INFO)
import Bundlebase
```

### Too many logs in REPL

**Problem**: The REPL is showing too many DEBUG logs or too much formatting.

**Solution**: Use the `ui` log level (default) or set it to a higher level:
```bash
# Use UI mode (minimal format, message only) - this is the default
Bundlebase-server --container mydata --repl

# Or explicitly set to UI mode
Bundlebase-server --container mydata --repl --log-level ui

# Or set to INFO level with full format
Bundlebase-server --container mydata --repl --log-level info

# Or show only WARN and ERROR with full format
Bundlebase-server --container mydata --repl --log-level warn
```
