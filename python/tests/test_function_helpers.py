"""Helper functions for UDF E2E tests.

These functions are invoked by the Rust Python UDF bridge during query execution.
Each function receives PyArrow arrays and returns a PyArrow array.
"""

import pyarrow as pa
import pyarrow.compute as pc


def double_val(col: pa.Array) -> pa.Array:
    """Double an Int64 column."""
    return pc.multiply(col, 2)


def double_val_float(col: pa.Array) -> pa.Array:
    """Double a Float64 column."""
    return pc.multiply(col, 2.0)


def add_vals(a: pa.Array, b: pa.Array) -> pa.Array:
    """Add two Int64 columns."""
    return pc.add(a, b)


class MySum:
    """Aggregate UDF that sums Int64 values."""

    def create_state(self):
        return pa.scalar(0, type=pa.int64())

    def accumulate(self, state, values):
        batch_sum = pc.sum(values).as_py()
        if batch_sum is None:
            return state
        return pa.scalar(state.as_py() + batch_sum, type=pa.int64())

    def merge(self, state1, state2):
        return pa.scalar(state1.as_py() + state2.as_py(), type=pa.int64())

    def evaluate(self, state):
        return state


def bundlebase_metadata():
    """Return metadata for all functions in this module.

    This is the standard convention for auto-detecting function signatures.
    Bundlebase calls this function to discover types when CREATE FUNCTION
    is used without explicit type signatures.
    """
    return {
        "functions": [
            {
                "name": "double_val",
                "input_types": ["Int64"],
                "return_type": "Int64",
                "kind": "scalar",
            },
            {
                "name": "add_vals",
                "input_types": ["Int64", "Int64"],
                "return_type": "Int64",
                "kind": "scalar",
            },
            {
                "name": "MySum",
                "input_types": ["Int64"],
                "return_type": "Int64",
                "kind": "aggregate",
            },
        ]
    }
