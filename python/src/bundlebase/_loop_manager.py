"""Event loop management for synchronous Bundlebase API.

Handles bridging between async Rust bindings and synchronous Python scripts/Jupyter notebooks.
Automatically detects environment and uses appropriate async execution strategy.
"""

import asyncio
import atexit
import sys
from typing import Any, Coroutine, Optional, TypeVar

T = TypeVar("T")


class EventLoopManager:
    """Manages event loop lifecycle for synchronous operations.

    This class handles:
    - Detecting whether we're running in Jupyter/IPython
    - Using persistent asyncio.Runner() for scripts (Python 3.11+)
    - Using nest_asyncio for Jupyter's nested event loops
    - Executing coroutines synchronously with appropriate strategy
    - Automatically cleaning up resources on application exit
    """

    def __init__(self) -> None:
        """Initialize the event loop manager with environment detection.

        Automatically registers cleanup on application exit to prevent
        resource leaks in long-running applications.
        """
        self._runner: Optional[asyncio.Runner] = None
        self._in_jupyter = self._detect_jupyter()
        # Register cleanup to run on application exit
        atexit.register(self.cleanup)

    def _detect_jupyter(self) -> bool:
        """Detect if running in Jupyter/IPython.

        Returns:
            True if running in IPython/Jupyter, False otherwise
        """
        try:
            from IPython import get_ipython

            return get_ipython() is not None
        except ImportError:
            return False

    def run_sync(self, coro: Coroutine[Any, Any, T]) -> T:
        """Run a coroutine synchronously.

        Chooses the appropriate execution strategy based on environment:
        - Jupyter: Uses nest_asyncio for nested event loops
        - Scripts: Uses persistent asyncio.Runner() for efficiency

        Args:
            coro: The coroutine to execute

        Returns:
            The result of the coroutine

        Raises:
            RuntimeError: If event loop execution fails
        """
        if self._in_jupyter:
            return self._run_in_jupyter(coro)
        else:
            return self._run_in_script(coro)

    def _run_in_jupyter(self, coro: Coroutine[Any, Any, T]) -> T:
        """Run coroutine in Jupyter environment using nest_asyncio.

        Args:
            coro: The coroutine to execute

        Returns:
            The result of the coroutine
        """
        try:
            import nest_asyncio

            # Allow nested event loops (required for Jupyter)
            nest_asyncio.apply()
        except ImportError:
            raise ImportError(
                "nest_asyncio is required for Jupyter support. "
                "Install it with: pip install nest-asyncio or "
                "poetry install -E jupyter"
            )

        try:
            loop = asyncio.get_event_loop()
        except RuntimeError:
            # No event loop in current thread, create one
            loop = asyncio.new_event_loop()
            asyncio.set_event_loop(loop)

        return loop.run_until_complete(coro)

    def _run_in_script(self, coro: Coroutine[Any, Any, T]) -> T:
        """Run coroutine in script environment using persistent Runner.

        Uses asyncio.Runner (Python 3.11+) for efficiency, with fallback
        to asyncio.run() for older Python versions.

        Args:
            coro: The coroutine to execute

        Returns:
            The result of the coroutine
        """
        if sys.version_info >= (3, 11):
            # Use persistent Runner for efficiency (avoids creating new loop each time)
            if self._runner is None:
                self._runner = asyncio.Runner()
            return self._runner.run(coro)
        else:
            # Fallback for Python < 3.11
            return asyncio.run(coro)

    def cleanup(self) -> None:
        """Clean up resources.

        Should be called on application exit to properly close the event loop.
        """
        if self._runner is not None:
            self._runner.close()
            self._runner = None
