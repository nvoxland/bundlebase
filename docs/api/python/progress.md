# Progress Tracking

Monitor the progress of long-running Bundlebase operations.

## Overview

Bundlebase provides a progress tracking system for monitoring data processing operations. This is particularly useful when working with large datasets or long-running transformations.

## Progress Module

::: bundlebase.progress
    options:
      show_root_heading: true
      show_root_full_path: false
      members: true

## Examples

### Basic Progress Tracking

```python
import bundlebase
from bundlebase.progress import StreamProgress

# Create a progress tracker
progress = StreamProgress()

c = await bundlebase.create()
c = await c.attach("large_dataset.parquet")

# Stream with progress tracking
async for batch in bundlebase.stream_batches(c, progress=progress):
    # Show progress
    print(f"Progress: {progress.percentage:.1f}%")
    print(f"Rows processed: {progress.rows_processed}")

    # Process batch
    process_batch(batch)
```

### Custom Progress Reporting

```python
import bundlebase
from bundlebase.progress import StreamProgress

class CustomProgress(StreamProgress):
    """Custom progress tracker with logging."""

    def update(self, rows: int):
        """Called when progress updates."""
        super().update(rows)
        if self.rows_processed % 100000 == 0:
            print(f"Processed {self.rows_processed:,} rows...")

progress = CustomProgress()

c = await bundlebase.create().attach("data.parquet")
async for batch in bundlebase.stream_batches(c, progress=progress):
    process(batch)
```

### Progress with Time Estimates

```python
import bundlebase
from bundlebase.progress import StreamProgress
import time

progress = StreamProgress()
start_time = time.time()

c = await bundlebase.open("large_dataset.parquet")

async for batch in bundlebase.stream_batches(c, progress=progress):
    # Calculate ETA
    elapsed = time.time() - start_time
    if progress.percentage > 0:
        total_time = elapsed / (progress.percentage / 100)
        remaining = total_time - elapsed
        print(f"Progress: {progress.percentage:.1f}% (ETA: {remaining:.0f}s)")

    process(batch)
```

### Integration with tqdm

```python
import bundlebase
from bundlebase.progress import StreamProgress
from tqdm import tqdm

progress = StreamProgress()
c = await bundlebase.open("large_dataset.parquet")

# Use tqdm for nice progress bar
with tqdm(total=100, desc="Processing") as pbar:
    last_percentage = 0

    async for batch in bundlebase.stream_batches(c, progress=progress):
        # Update tqdm
        current = progress.percentage
        pbar.update(current - last_percentage)
        last_percentage = current

        process(batch)
```

### Batch Size Control

```python
import bundlebase
from bundlebase.progress import StreamProgress

# Control batch size (in bytes)
progress = StreamProgress(batch_size=50_000_000)  # 50MB batches

c = await bundlebase.create().attach("data.parquet")
async for batch in bundlebase.stream_batches(c, progress=progress):
    # Smaller batches for memory-constrained environments
    print(f"Batch size: {batch.nbytes / 1024 / 1024:.1f} MB")
    process(batch)
```

## Progress Properties

Progress trackers typically provide:

- **rows_processed** - Total number of rows processed so far
- **percentage** - Completion percentage (0-100)
- **batch_count** - Number of batches processed
- **batch_size** - Target size of each batch in bytes

## Implementing Custom Trackers

Create custom progress trackers by subclassing `StreamProgress`:

```python
from bundlebase.progress import StreamProgress
import logging

class LoggingProgress(StreamProgress):
    """Progress tracker that logs to Python logging."""

    def __init__(self, batch_size: int = 100_000_000):
        super().__init__(batch_size)
        self.logger = logging.getLogger(__name__)

    def update(self, rows: int):
        """Called when new rows are processed."""
        super().update(rows)
        self.logger.info(
            f"Processed {self.rows_processed:,} rows "
            f"({self.percentage:.1f}%)"
        )

    def complete(self):
        """Called when processing completes."""
        self.logger.info(
            f"Processing complete! Total rows: {self.rows_processed:,}"
        )

# Use custom tracker
progress = LoggingProgress()
async for batch in bundlebase.stream_batches(c, progress=progress):
    process(batch)
progress.complete()
```

## Performance Considerations

Progress tracking adds minimal overhead:

- **With progress**: ~0.1ms per batch
- **Without progress**: ~0.05ms per batch

For most use cases, this overhead is negligible compared to data processing time.

## Integration with UI Frameworks

### Streamlit

```python
import bundlebase
from bundlebase.progress import StreamProgress
import streamlit as st

progress = StreamProgress()
progress_bar = st.progress(0)
status_text = st.empty()

c = await bundlebase.open("large_dataset.parquet")

async for batch in bundlebase.stream_batches(c, progress=progress):
    # Update Streamlit UI
    progress_bar.progress(progress.percentage / 100)
    status_text.text(f"Processed {progress.rows_processed:,} rows")

    process(batch)

progress_bar.progress(100)
status_text.text("Processing complete!")
```

### Jupyter Widgets

```python
import bundlebase
from bundlebase.progress import StreamProgress
from ipywidgets import IntProgress, HTML, VBox
from IPython.display import display

progress = StreamProgress()
progress_widget = IntProgress(min=0, max=100, description='Progress:')
status_widget = HTML()
display(VBox([progress_widget, status_widget]))

c = await bundlebase.open("large_dataset.parquet")

async for batch in bundlebase.stream_batches(c, progress=progress):
    # Update widgets
    progress_widget.value = int(progress.percentage)
    status_widget.value = f"<p>Processed {progress.rows_processed:,} rows</p>"

    process(batch)
```

## See Also

- **[Conversion Functions](conversion.md)** - `stream_batches()` usage
- **[Progress Tracking Guide](../../guides/progress-tracking.md)** - Detailed guide
- **[Async API](async-api.md)** - Core bundle operations
