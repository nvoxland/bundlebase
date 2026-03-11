"""Abstract base class for IPC-based functions."""

from abc import ABC, abstractmethod
from typing import Union

import pyarrow as pa


class Function(ABC):
    """Base class for implementing a custom Bundlebase IPC function provider.

    Subclass this and implement invoke() and functions().
    The function provider is run as a subprocess and communicates via
    Arrow Flight protocol.

    For aggregate functions, also implement the aggregate methods:
    create_state(), accumulate(), merge(), evaluate().
    """

    @abstractmethod
    def invoke(
        self, name: str, batch: pa.RecordBatch
    ) -> Union[pa.RecordBatch, pa.Array, dict]:
        """Invoke a scalar function.

        Args:
            name: The function name being invoked.
            batch: Input RecordBatch with one column per argument.

        Returns:
            A single-column RecordBatch, a PyArrow Array, or a dict
            of column-oriented data (requires schema() to be defined).
        """
        ...

    def schema(self) -> dict[str, str] | None:
        """Optional schema for dict return types.

        If invoke() returns a dict, this method must return a schema
        mapping column names to Arrow type strings.

        Returns:
            Dict mapping column names to type strings, or None.
        """
        return None

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

    def create_state(self, name: str) -> object:
        """Create initial accumulator state for an aggregate function.

        Args:
            name: The aggregate function name.

        Returns:
            An opaque state object (held server-side).
        """
        raise NotImplementedError(
            f"Aggregate function '{name}' requires create_state()"
        )

    def accumulate(self, name: str, state: object, batch: pa.RecordBatch) -> object:
        """Accumulate a batch into an aggregate state.

        Args:
            name: The aggregate function name.
            state: The current state object.
            batch: Input RecordBatch with one column per argument.

        Returns:
            Updated state object.
        """
        raise NotImplementedError(
            f"Aggregate function '{name}' requires accumulate()"
        )

    def merge(self, name: str, state1: object, state2: object) -> object:
        """Merge two aggregate states.

        Args:
            name: The aggregate function name.
            state1: First state object.
            state2: Second state object.

        Returns:
            Merged state object.
        """
        raise NotImplementedError(
            f"Aggregate function '{name}' requires merge()"
        )

    def evaluate(self, name: str, state: object) -> pa.Scalar:
        """Evaluate an aggregate state to produce the final result.

        Args:
            name: The aggregate function name.
            state: The accumulated state object.

        Returns:
            A PyArrow scalar with the final result.
        """
        raise NotImplementedError(
            f"Aggregate function '{name}' requires evaluate()"
        )
