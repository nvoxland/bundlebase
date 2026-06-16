# Progress Tracking

Monitor the progress of long-running Bundlebase operations.

## Overview

Bundlebase reports progress events for operations like attaching data, indexing, and fetching from sources. If [tqdm](https://tqdm.github.io/) is installed, progress bars appear automatically — no setup required. You can also register a custom callback to handle progress events yourself.

## Progress Module

::: bundlebase.progress
    options:
      show_root_heading: true
      show_root_full_path: false
      members: true

## Examples

### Automatic Progress Bars (tqdm)

If tqdm is installed, progress bars appear automatically for long-running operations:

```python
import bundlebase

c = await bundlebase.create("/path/to/bundle")
c = await c.attach("large_data.parquet")  # Progress bar appears automatically
```

Install tqdm if you don't have it:

```bash
pip install tqdm
```

### Custom Progress Callback

Register your own callback to handle progress events. The callback receives an event type (`'start'`, `'update'`, or `'finish'`), the operation name, a unique operation id, the current progress value, the total (or `None` if unknown), and an optional message:

```python
import bundlebase

def my_progress(event, operation, id, current, total, message):
    if event == 'start':
        print(f"Starting: {operation}")
    elif event == 'update':
        pct = (current / total * 100) if total else 0
        print(f"Progress: {pct:.1f}%")
    elif event == 'finish':
        print(f"Finished: {operation}")

bundlebase.progress.set_callback(my_progress)
```

### Disabling Progress

Turn off all progress feedback:

```python
import bundlebase

bundlebase.progress.disable()
```

Re-enable tqdm progress bars later:

```python
bundlebase.progress.enable_tqdm()
```

### Logging Progress Events

Forward progress events to Python logging instead of the terminal:

```python
import bundlebase
import logging

logger = logging.getLogger("bundlebase.progress")

def log_progress(event, operation, id, current, total, message):
    if event == 'start':
        logger.info("Starting %s", operation)
    elif event == 'finish':
        logger.info("Finished %s", operation)

bundlebase.progress.set_callback(log_progress)
```

## Callback Reference

A progress callback has the signature:

```python
def callback(event: str, operation: str, id: int, current: int,
             total: Optional[int], message: Optional[str]) -> None: ...
```

| Parameter | Description |
|---|---|
| `event` | `'start'`, `'update'`, or `'finish'` |
| `operation` | Human-readable operation name |
| `id` | Unique identifier for this progress operation (multiple operations can run concurrently) |
| `current` | Current progress value |
| `total` | Total expected value, or `None` for indeterminate progress |
| `message` | Optional status message |

## See Also

- **[Conversion Functions](conversion.md)** - `stream_batches()` usage
- **[Logging](../../guide/logging.md)** - Detailed guide
- **[Async API](async-api.md)** - Core bundle operations
