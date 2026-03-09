"""Abstract base class for IPC-based functions."""

from abc import ABC, abstractmethod
from typing import Union

import pyarrow as pa


class Function(ABC):
    """Base class for implementing a custom Bundlebase IPC function provider.

    Subclass this and implement invoke() and functions().
    The function provider is run as a subprocess and communicates via
    JSON-RPC 2.0 + Arrow IPC protocol.
    """

    @abstractmethod
    def invoke(
        self, name: str, batch: pa.RecordBatch
    ) -> pa.RecordBatch:
        """Invoke a scalar function.

        Args:
            name: The function name being invoked.
            batch: Input RecordBatch with one column per argument.

        Returns:
            A single-column RecordBatch with the result.
        """
        ...

    @abstractmethod
    def functions(self) -> list[dict]:
        """Return a manifest of available functions.

        Each entry should be a dict with keys:
        - name: str
        - input_types: list[str] (Arrow type names)
        - return_type: str (Arrow type name)
        - kind: str ("scalar" or "aggregate", default "scalar")

        Returns:
            List of function metadata dicts.
        """
        ...
