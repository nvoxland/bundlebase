"""Event loop management for synchronous Bundlebase API.

Handles bridging between async Rust bindings and synchronous Python scripts/Jupyter notebooks.
Automatically detects environment and uses appropriate async execution strategy.
"""

import asyncio
import atexit
import sys
import threading
from typing import Any, Coroutine, List, Optional, TypeVar

T = TypeVar("T")


def _is_jupyter_kernel(ipython_shell: Any) -> bool:
    """Return True only for a Jupyter/ipykernel shell (``ZMQInteractiveShell``).

    Terminal IPython (``TerminalInteractiveShell``) and a plain interpreter
    (``None``) return False, since they have no running event loop that would
    require nest_asyncio.
    """
    if ipython_shell is None:
        return False
    return type(ipython_shell).__name__ == "ZMQInteractiveShell"


class EventLoopManager:
    """Manages event loop lifecycle for synchronous operations.

    This class handles:
    - Detecting whether we're running in Jupyter/IPython
    - Using a per-thread persistent asyncio.Runner() for scripts
    - Using nest_asyncio for Jupyter's nested event loops
    - Executing coroutines synchronously with appropriate strategy
    - Automatically cleaning up resources on application exit

    Thread safety: a single ``asyncio.Runner`` is not safe to drive from
    multiple threads concurrently, so each thread gets its own runner via
    thread-local storage. Threads run independently (the Rust async work
    releases the GIL), and every runner is tracked so ``cleanup()`` closes
    them all on exit.
    """

    def __init__(self) -> None:
        """Initialize the event loop manager with environment detection.

        Automatically registers cleanup on application exit to prevent
        resource leaks in long-running applications.
        """
        # Per-thread runner: asyncio.Runner.run() must not be called
        # concurrently from multiple threads on the same instance.
        self._local = threading.local()
        # Track every created runner so cleanup() can close all of them.
        self._runners: List[asyncio.Runner] = []
        self._runners_lock = threading.Lock()
        self._in_jupyter = self._detect_jupyter()
        # Register cleanup to run on application exit
        atexit.register(self.cleanup)

    def _detect_jupyter(self) -> bool:
        """Detect if running inside a Jupyter kernel (which needs nest_asyncio).

        Only a Jupyter/ipykernel shell (``ZMQInteractiveShell``) runs an event
        loop that requires the nested-loop path. Terminal IPython
        (``TerminalInteractiveShell``) has no running loop, so the plain
        asyncio.Runner path works and nest_asyncio is not required.

        Returns:
            True only if running inside a Jupyter kernel.
        """
        try:
            from IPython import get_ipython

            return _is_jupyter_kernel(get_ipython())
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

        Uses asyncio.Runner for efficiency.

        Args:
            coro: The coroutine to execute

        Returns:
            The result of the coroutine
        """
        # Per-thread persistent Runner: reused across calls within a thread
        # (avoids creating a new loop each time) but isolated between threads
        # so concurrent run_sync() calls from different threads are safe.
        runner = getattr(self._local, "runner", None)
        if runner is None:
            runner = asyncio.Runner(debug=False)
            self._local.runner = runner
            with self._runners_lock:
                self._runners.append(runner)
        return runner.run(coro)

    def cleanup(self) -> None:
        """Clean up resources.

        Should be called on application exit to properly close every
        thread's event loop runner.
        """
        with self._runners_lock:
            runners, self._runners = self._runners, []
        for runner in runners:
            try:
                runner.close()
            except Exception:
                # Best-effort cleanup on exit; a runner whose loop is mid-use
                # in another thread may refuse to close — ignore.
                pass
        # Drop this thread's reference so a later call re-creates one.
        if getattr(self._local, "runner", None) is not None:
            self._local.runner = None
