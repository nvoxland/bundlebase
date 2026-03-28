#!/usr/bin/env python3
"""IPC function server for benchmarks. Implements double_val (scalar) and int_sum (aggregate)."""

import sys
import os

# Add SDK to path
sdk_path = os.path.join(os.path.dirname(__file__), "..", "..", "..", "..", "sdk", "python")
sys.path.insert(0, sdk_path)

from bundlebase_sdk import Function, serve_function
import pyarrow as pa
import pyarrow.compute as pc


class BenchFunctions(Function):
    def functions(self):
        return [
            {
                "name": "double_val",
                "input_types": ["Int64"],
                "return_type": "Int64",
                "kind": "scalar",
            },
            {
                "name": "int_sum",
                "input_types": ["Int64"],
                "return_type": "Int64",
                "kind": "aggregate",
            },
        ]

    def invoke(self, name, batch):
        col = batch.column(0)
        doubled = pc.multiply(col, 2)
        return pa.record_batch({"result": doubled})

    def create_state(self, name):
        return pa.scalar(0, type=pa.int64())

    def accumulate(self, name, state, batch):
        col = batch.column(0)
        batch_sum = pc.sum(col).as_py()
        if batch_sum is None:
            return state
        return pa.scalar(state.as_py() + batch_sum, type=pa.int64())

    def merge(self, name, state1, state2):
        return pa.scalar(state1.as_py() + state2.as_py(), type=pa.int64())

    def evaluate(self, name, state):
        return state


if __name__ == "__main__":
    serve_function(BenchFunctions())
