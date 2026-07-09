"""Regression test for July 2026 audit finding bb-cko: Jupyter detection.

Terminal IPython must not be treated as a Jupyter kernel, otherwise the sync API
demands nest_asyncio and raises ImportError even though the plain Runner path
would work.
"""

from bundlebase._loop_manager import _is_jupyter_kernel


class ZMQInteractiveShell:
    """Stand-in for the Jupyter/ipykernel shell class."""


class TerminalInteractiveShell:
    """Stand-in for terminal IPython's shell class."""


def test_terminal_ipython_is_not_a_jupyter_kernel():
    assert _is_jupyter_kernel(TerminalInteractiveShell()) is False


def test_jupyter_kernel_is_detected():
    assert _is_jupyter_kernel(ZMQInteractiveShell()) is True


def test_plain_interpreter_is_not_jupyter():
    assert _is_jupyter_kernel(None) is False
