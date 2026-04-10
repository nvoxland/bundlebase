"""IPC harness for file-backed Python functions.

Loads a `.py` file, discovers functions via `bundlebase_metadata()`,
and serves them over the standard IPC protocol (JSON-RPC + Arrow IPC).

Usage:
    python -m bundlebase_sdk._ipc_harness ./script.py
    python -m bundlebase_sdk._ipc_harness ./script.py --bundlebase-functions
"""

import importlib.util
from importlib.machinery import SourceFileLoader
import sys

from bundlebase_sdk.function import Function
from bundlebase_sdk.function_serve import serve_function


def _load_module_from_file(file_path: str):
    """Load a Python module from a file path."""
    spec = importlib.util.spec_from_file_location(
        "_user_module",
        file_path,
        loader=SourceFileLoader("_user_module", file_path),
    )
    if spec is None or spec.loader is None:
        raise ImportError(f"Cannot load Python module from '{file_path}'")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class _FileBackedFunction(Function):
    """Adapter that wraps a user's .py file as a Function provider."""

    def __init__(self, module):
        self._module = module

    def functions(self) -> list[dict]:
        if hasattr(self._module, "bundlebase_metadata"):
            metadata = self._module.bundlebase_metadata()
            return metadata.get("functions", [])
        return []

    def invoke(self, name: str, batch):
        func = getattr(self._module, name, None)
        if func is None:
            raise AttributeError(
                f"Function '{name}' not found in module '{self._module.__name__}'"
            )
        # Extract columns from the batch and pass as positional args
        columns = [batch.column(i) for i in range(batch.num_columns)]
        return func(*columns)

    def create_state(self, name: str):
        cls = getattr(self._module, name, None)
        if cls is None:
            raise AttributeError(
                f"Aggregate class '{name}' not found in module '{self._module.__name__}'"
            )
        instance = cls()
        return instance.create_state()

    def accumulate(self, name: str, state, batch):
        cls = getattr(self._module, name, None)
        if cls is None:
            raise AttributeError(
                f"Aggregate class '{name}' not found in module '{self._module.__name__}'"
            )
        instance = cls()
        # Extract the first column for single-argument aggregates
        values = batch.column(0) if batch.num_columns == 1 else batch
        return instance.accumulate(state, values)

    def merge(self, name: str, state1, state2):
        cls = getattr(self._module, name, None)
        if cls is None:
            raise AttributeError(
                f"Aggregate class '{name}' not found in module '{self._module.__name__}'"
            )
        instance = cls()
        return instance.merge(state1, state2)

    def evaluate(self, name: str, state):
        cls = getattr(self._module, name, None)
        if cls is None:
            raise AttributeError(
                f"Aggregate class '{name}' not found in module '{self._module.__name__}'"
            )
        instance = cls()
        return instance.evaluate(state)


def main():
    if len(sys.argv) < 2:
        print(
            "Usage: python -m bundlebase_sdk._ipc_harness <script.py> [--bundlebase-functions]",
            file=sys.stderr,
        )
        sys.exit(1)

    script_path = sys.argv[1]
    module = _load_module_from_file(script_path)
    func = _FileBackedFunction(module)
    serve_function(func)


if __name__ == "__main__":
    main()
