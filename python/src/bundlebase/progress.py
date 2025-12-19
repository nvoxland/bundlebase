"""Progress tracking for long-running Bundle operations.

This module provides integration with progress bar libraries like tqdm,
allowing users to see visual progress feedback for operations like indexing,
attaching data, and querying large datasets.

By default, if tqdm is installed, progress bars will appear automatically.
Users can also provide custom callbacks for progress tracking.

Example:
    # Auto-enabled if tqdm installed
    import bundlebase
    c = await bundlebase.create("/path/to/bundle")
    await c.attach("large_data.parquet")  # Progress bar appears automatically

    # Custom callback
    def my_progress(event, operation, id, current, total, message):
        if event == 'start':
            print(f"Starting: {operation}")
        elif event == 'update':
            print(f"Progress: {current}/{total or '?'}")
        elif event == 'finish':
            print(f"Finished: {operation}")

    bundlebase.progress.set_callback(my_progress)
"""

from typing import Optional, Callable
import sys


def _has_tqdm() -> bool:
    """Check if tqdm is available."""
    try:
        import tqdm  # noqa: F401
        return True
    except ImportError:
        return False


class TqdmProgressTracker:
    """Progress tracker that uses tqdm for visual progress bars.

    This tracker creates and manages tqdm progress bars for Bundle
    operations. It supports both determinate (known total) and indeterminate
    (unknown total) progress.
    """

    def __init__(self):
        """Initialize the tqdm progress tracker."""
        try:
            from tqdm.auto import tqdm
            self.tqdm = tqdm
            self._bars = {}  # id -> tqdm instance
        except ImportError:
            raise RuntimeError("tqdm is not installed. Install with: pip install tqdm")

    def __call__(self, event: str, operation: str, id: int, current: int,
                 total: Optional[int], message: Optional[str]):
        """Handle a progress event.

        Args:
            event: Type of event ('start', 'update', 'finish')
            operation: Human-readable operation name
            id: Unique identifier for this progress operation
            current: Current progress value
            total: Total expected value (None for indeterminate)
            message: Optional status message
        """
        if event == 'start':
            # Create a new progress bar
            self._bars[id] = self.tqdm(
                total=total,
                desc=operation,
                unit='items' if total else '',
                leave=False,  # Auto-clear on completion
                file=sys.stderr,  # Write to stderr to not interfere with output
            )

        elif event == 'update':
            bar = self._bars.get(id)
            if bar:
                # Update progress
                if total is not None:
                    # Determinate: update to absolute position
                    bar.n = current
                else:
                    # Indeterminate: just increment to show activity
                    bar.update(1)

                # Update description with message if provided
                if message:
                    bar.set_postfix_str(message)

                bar.refresh()

        elif event == 'finish':
            bar = self._bars.pop(id, None)
            if bar:
                # Complete and close the progress bar
                if bar.total is not None:
                    bar.n = bar.total
                bar.close()


class CustomCallbackTracker:
    """Progress tracker that forwards events to a custom callback.

    This tracker allows users to provide their own progress handling logic.
    """

    def __init__(self, callback: Callable):
        """Initialize with a custom callback.

        Args:
            callback: Function with signature:
                fn(event, operation, id, current, total, message)
        """
        self.callback = callback

    def __call__(self, event: str, operation: str, id: int, current: int,
                 total: Optional[int], message: Optional[str]):
        """Forward the event to the custom callback."""
        self.callback(event, operation, id, current, total, message)


# Current active tracker (None means no progress tracking)
_current_tracker: Optional[Callable] = None


def set_callback(callback: Optional[Callable] = None):
    """Set a custom progress callback.

    Args:
        callback: Function with signature:
            fn(event: str, operation: str, id: int, current: int,
               total: Optional[int], message: Optional[str])
            If None, disables progress tracking.

    Example:
        def my_callback(event, operation, id, current, total, message):
            if event == 'start':
                print(f"Starting: {operation}")
            elif event == 'update':
                pct = (current / total * 100) if total else 0
                print(f"Progress: {pct:.1f}%")
            elif event == 'finish':
                print(f"Finished: {operation}")

        bundlebase.progress.set_callback(my_callback)
    """
    global _current_tracker

    if callback is None:
        _current_tracker = None
        # Register no-op in Rust
        from ._bundlebase import _register_progress_callback
        _register_progress_callback(lambda *args: None)
    else:
        _current_tracker = CustomCallbackTracker(callback)
        from ._bundlebase import _register_progress_callback
        _register_progress_callback(_current_tracker)


def enable_tqdm():
    """Explicitly enable tqdm progress bars.

    This is called automatically on module import if tqdm is available.
    Users only need to call this if they disabled progress and want to re-enable it.

    Raises:
        RuntimeError: If tqdm is not installed
    """
    global _current_tracker
    _current_tracker = TqdmProgressTracker()
    from ._bundlebase import _register_progress_callback
    _register_progress_callback(_current_tracker)


def disable():
    """Disable all progress tracking.

    Operations will run silently without any progress feedback.
    """
    set_callback(None)


def install_default_tracker():
    """Install the default progress tracker.

    If tqdm is available, enables tqdm progress bars.
    Otherwise, does nothing (silent operation).

    This is called automatically when the bundlebase module is imported.
    """
    global _current_tracker

    if _has_tqdm():
        try:
            enable_tqdm()
        except Exception:
            # If enabling tqdm fails, silently continue without progress
            _current_tracker = None
    else:
        # No tqdm available, remain silent
        _current_tracker = None


# Auto-install default tracker on module import
install_default_tracker()


__all__ = [
    'set_callback',
    'enable_tqdm',
    'disable',
    'install_default_tracker',
    'TqdmProgressTracker',
    'CustomCallbackTracker',
]
