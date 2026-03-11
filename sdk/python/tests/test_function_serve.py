"""End-to-end tests for the function serve loop with mock stdin/stdout."""

import io
import json
import struct
import time

import pyarrow as pa

from bundlebase_sdk.function import Function
from bundlebase_sdk.function_serve import _serve_function, _AggregateStateStore


# ======================== Test Function Implementations ========================


class DoubleFunction(Function):
    """A scalar function that doubles an Int64 column."""

    def invoke(self, name, batch):
        col = batch.column(0)
        doubled = pa.array([v.as_py() * 2 for v in col], type=pa.int64())
        return pa.record_batch({"result": doubled})

    def functions(self):
        return [
            {
                "name": "double_val",
                "input_types": ["Int64"],
                "return_type": "Int64",
                "kind": "scalar",
            }
        ]


class SumAggregate(Function):
    """An aggregate function that sums an Int64 column."""

    def invoke(self, name, batch):
        raise NotImplementedError("SumAggregate is aggregate-only")

    def functions(self):
        return [
            {
                "name": "my_sum",
                "input_types": ["Int64"],
                "return_type": "Int64",
                "kind": "aggregate",
            }
        ]

    def create_state(self, name):
        return 0

    def accumulate(self, name, state, batch):
        col = batch.column(0)
        for v in col:
            state += v.as_py()
        return state

    def merge(self, name, state1, state2):
        return state1 + state2

    def evaluate(self, name, state):
        return pa.scalar(state, type=pa.int64())


class MultiFunction(Function):
    """A function provider with multiple functions registered."""

    def invoke(self, name, batch):
        col = batch.column(0)
        if name == "add_one":
            result = pa.array([v.as_py() + 1 for v in col], type=pa.int64())
        elif name == "negate":
            result = pa.array([-v.as_py() for v in col], type=pa.int64())
        else:
            raise ValueError(f"Unknown function: {name}")
        return pa.record_batch({"result": result})

    def functions(self):
        return [
            {
                "name": "add_one",
                "input_types": ["Int64"],
                "return_type": "Int64",
                "kind": "scalar",
            },
            {
                "name": "negate",
                "input_types": ["Int64"],
                "return_type": "Int64",
                "kind": "scalar",
            },
        ]


class ErrorFunction(Function):
    """A function that raises exceptions for error handling tests."""

    def invoke(self, name, batch):
        raise ValueError("invoke failed on purpose")

    def functions(self):
        return [
            {
                "name": "broken",
                "input_types": ["Int64"],
                "return_type": "Int64",
                "kind": "scalar",
            }
        ]


# ======================== Helpers ========================


def _make_request(method, params=None, req_id=1):
    """Build a JSON-RPC request line as bytes."""
    req = {"jsonrpc": "2.0", "id": req_id, "method": method, "params": params or {}}
    return json.dumps(req).encode() + b"\n"


def _make_arrow_ipc(batch):
    """Serialize a RecordBatch into a length-prefixed Arrow IPC frame."""
    sink = pa.BufferOutputStream()
    writer = pa.ipc.new_stream(sink, batch.schema)
    writer.write_batch(batch)
    writer.close()
    data = sink.getvalue().to_pybytes()
    return struct.pack(">I", len(data)) + data


def _read_response(stdout_bytes, offset=0):
    """Read a JSON-RPC response line from bytes at offset.

    Returns (response_dict, new_offset).
    """
    end = stdout_bytes.index(b"\n", offset)
    line = stdout_bytes[offset:end]
    return json.loads(line), end + 1


def _read_arrow_frame(stdout_bytes, offset):
    """Read a length-prefixed Arrow IPC frame from bytes.

    Returns (pa.Table or None, new_offset).
    """
    length = struct.unpack(">I", stdout_bytes[offset : offset + 4])[0]
    offset += 4
    if length == 0:
        return None, offset
    ipc_data = stdout_bytes[offset : offset + length]
    reader = pa.ipc.open_stream(ipc_data)
    table = reader.read_all()
    return table, offset + length


# ======================== Scalar Function Tests ========================


class TestScalarInvoke:
    def test_invoke_doubles_values(self):
        input_batch = pa.record_batch({"x": pa.array([1, 2, 3], type=pa.int64())})
        arrow_frame = _make_arrow_ipc(input_batch)

        stdin = io.BytesIO(
            _make_request("invoke", {"function": "double_val"}, req_id=1)
            + arrow_frame
            + _make_request("shutdown", req_id=2)
        )
        stdout = io.BytesIO()
        _serve_function(DoubleFunction(), stdin, stdout)

        out = stdout.getvalue()
        # First response is the ack
        resp, offset = _read_response(out)
        assert resp["id"] == 1
        assert resp["result"]["ok"] is True

        # Then the Arrow IPC result
        table, offset = _read_arrow_frame(out, offset)
        assert table is not None
        assert table.column("result").to_pylist() == [2, 4, 6]

    def test_invoke_empty_input(self):
        """Invoking with zero-length Arrow frame returns zero-length output."""
        empty_frame = struct.pack(">I", 0)

        stdin = io.BytesIO(
            _make_request("invoke", {"function": "double_val"}, req_id=1)
            + empty_frame
            + _make_request("shutdown", req_id=2)
        )
        stdout = io.BytesIO()
        _serve_function(DoubleFunction(), stdin, stdout)

        out = stdout.getvalue()
        resp, offset = _read_response(out)
        assert resp["id"] == 1
        assert resp["result"]["ok"] is True

        # Should get a zero-length Arrow frame back
        table, offset = _read_arrow_frame(out, offset)
        assert table is None


# ======================== Aggregate Function Tests ========================


class TestAggregateLifecycle:
    def test_create_accumulate_evaluate(self):
        """Full aggregate lifecycle: create_state -> accumulate -> evaluate."""
        input_batch = pa.record_batch({"x": pa.array([10, 20, 30], type=pa.int64())})
        arrow_frame = _make_arrow_ipc(input_batch)

        stdin = io.BytesIO(
            # Step 1: create_state
            _make_request("create_state", {"function": "my_sum"}, req_id=1)
            # Step 2: accumulate
            + _make_request("accumulate", {"function": "my_sum", "state_id": "0"}, req_id=2)
            + arrow_frame
            # Step 3: evaluate
            + _make_request("evaluate", {"function": "my_sum", "state_id": "0"}, req_id=3)
            + _make_request("shutdown", req_id=4)
        )
        stdout = io.BytesIO()
        _serve_function(SumAggregate(), stdin, stdout)

        out = stdout.getvalue()

        # create_state response
        resp, offset = _read_response(out)
        assert resp["id"] == 1
        assert resp["result"]["state_id"] == "0"

        # accumulate ack
        resp, offset = _read_response(out, offset)
        assert resp["id"] == 2
        assert resp["result"]["ok"] is True

        # evaluate ack
        resp, offset = _read_response(out, offset)
        assert resp["id"] == 3
        assert resp["result"]["ok"] is True

        # evaluate Arrow result
        table, offset = _read_arrow_frame(out, offset)
        assert table is not None
        assert table.column("result").to_pylist() == [60]

    def test_merge_two_states(self):
        """Merge two accumulated states and evaluate the merged result."""
        batch1 = pa.record_batch({"x": pa.array([10, 20], type=pa.int64())})
        batch2 = pa.record_batch({"x": pa.array([30, 40], type=pa.int64())})
        frame1 = _make_arrow_ipc(batch1)
        frame2 = _make_arrow_ipc(batch2)

        stdin = io.BytesIO(
            # Create two states
            _make_request("create_state", {"function": "my_sum"}, req_id=1)
            + _make_request("create_state", {"function": "my_sum"}, req_id=2)
            # Accumulate into each
            + _make_request("accumulate", {"function": "my_sum", "state_id": "0"}, req_id=3)
            + frame1
            + _make_request("accumulate", {"function": "my_sum", "state_id": "1"}, req_id=4)
            + frame2
            # Merge
            + _make_request("merge", {"function": "my_sum", "state_id1": "0", "state_id2": "1"}, req_id=5)
            # Evaluate merged state (id "2" since it's the third created state)
            + _make_request("evaluate", {"function": "my_sum", "state_id": "2"}, req_id=6)
            + _make_request("shutdown", req_id=7)
        )
        stdout = io.BytesIO()
        _serve_function(SumAggregate(), stdin, stdout)

        out = stdout.getvalue()

        # create_state responses
        resp, offset = _read_response(out)
        assert resp["result"]["state_id"] == "0"
        resp, offset = _read_response(out, offset)
        assert resp["result"]["state_id"] == "1"

        # accumulate acks
        resp, offset = _read_response(out, offset)
        assert resp["id"] == 3
        resp, offset = _read_response(out, offset)
        assert resp["id"] == 4

        # merge response
        resp, offset = _read_response(out, offset)
        assert resp["id"] == 5
        merged_id = resp["result"]["state_id"]
        assert merged_id == "2"

        # evaluate ack
        resp, offset = _read_response(out, offset)
        assert resp["id"] == 6

        # evaluate Arrow result: 10 + 20 + 30 + 40 = 100
        table, offset = _read_arrow_frame(out, offset)
        assert table is not None
        assert table.column("result").to_pylist() == [100]


# ======================== State Store TTL Tests ========================


class TestStateTTLCleanup:
    def test_cleanup_removes_expired_states(self):
        store = _AggregateStateStore()
        sid = store.create("some_state")
        assert store.get(sid) == "some_state"

        # Cleanup with a very short TTL should remove the state
        # We need to simulate time passing. Monkey-patch the created_at.
        store._states[sid] = ("some_state", time.monotonic() - 400)
        store.cleanup(ttl_seconds=300.0)
        assert store.get(sid) is None

    def test_cleanup_keeps_fresh_states(self):
        store = _AggregateStateStore()
        sid = store.create("fresh_state")
        store.cleanup(ttl_seconds=300.0)
        assert store.get(sid) == "fresh_state"

    def test_remove_deletes_state(self):
        store = _AggregateStateStore()
        sid = store.create("to_remove")
        store.remove(sid)
        assert store.get(sid) is None


# ======================== Error Handling Tests ========================


class TestErrorHandling:
    def test_invoke_exception_returns_error(self):
        """When a function raises, the serve loop returns a JSON-RPC error."""
        input_batch = pa.record_batch({"x": pa.array([1], type=pa.int64())})
        arrow_frame = _make_arrow_ipc(input_batch)

        stdin = io.BytesIO(
            _make_request("invoke", {"function": "broken"}, req_id=1)
            + arrow_frame
            + _make_request("shutdown", req_id=2)
        )
        stdout = io.BytesIO()
        _serve_function(ErrorFunction(), stdin, stdout)

        out = stdout.getvalue()
        # The ack response comes first
        resp, offset = _read_response(out)
        assert resp["id"] == 1
        assert resp["result"]["ok"] is True

        # The error is caught at the top level, but since the ack was already sent
        # and the Arrow read happened, the exception occurs during invoke.
        # The function_serve loop catches it as a general Exception and writes an error.
        # However, looking at the code: the invoke ack is sent first, then Arrow is read,
        # then func.invoke is called. If func.invoke raises, the exception propagates
        # up to the main try/except which writes an error response.
        # But wait -- the ack was already written for req_id=1. The error gets written
        # as a *second* response for the same request. Let's check what actually happens.
        #
        # Actually, looking at _handle_invoke: write_response(ack) then read arrow then
        # func.invoke. If func.invoke raises, it propagates to the except in _serve_function
        # which calls write_error with the same req_id. So we get TWO responses for req_id=1.
        resp2, offset = _read_response(out, offset)
        assert resp2["id"] == 1
        assert "error" in resp2
        assert "invoke failed on purpose" in resp2["error"]["message"]

    def test_unknown_method_returns_error(self):
        stdin = io.BytesIO(
            _make_request("bogus_method", req_id=1)
            + _make_request("shutdown", req_id=2)
        )
        stdout = io.BytesIO()
        _serve_function(DoubleFunction(), stdin, stdout)

        out = stdout.getvalue()
        resp, _ = _read_response(out)
        assert resp["error"]["code"] == -32601
        assert "Method not found" in resp["error"]["message"]

    def test_malformed_json_returns_parse_error(self):
        stdin = io.BytesIO(
            b"this is not json\n"
            + _make_request("shutdown", req_id=1)
        )
        stdout = io.BytesIO()
        _serve_function(DoubleFunction(), stdin, stdout)

        out = stdout.getvalue()
        resp, _ = _read_response(out)
        assert resp["error"]["code"] == -32700
        assert "Parse error" in resp["error"]["message"]

    def test_evaluate_unknown_state_returns_error(self):
        """Evaluating a non-existent state ID returns an error."""
        stdin = io.BytesIO(
            _make_request("evaluate", {"function": "my_sum", "state_id": "999"}, req_id=1)
            + _make_request("shutdown", req_id=2)
        )
        stdout = io.BytesIO()
        _serve_function(SumAggregate(), stdin, stdout)

        out = stdout.getvalue()
        # The ack is sent first
        resp, offset = _read_response(out)
        assert resp["id"] == 1
        assert resp["result"]["ok"] is True

        # Then the error for the unknown state
        resp2, _ = _read_response(out, offset)
        assert "error" in resp2
        assert "Unknown state ID" in resp2["error"]["message"]


# ======================== Multiple Functions Tests ========================


class TestMultipleFunctions:
    def test_invoke_different_functions(self):
        """Multiple functions can be invoked in the same session."""
        batch = pa.record_batch({"x": pa.array([5], type=pa.int64())})
        frame = _make_arrow_ipc(batch)

        stdin = io.BytesIO(
            _make_request("invoke", {"function": "add_one"}, req_id=1)
            + frame
            + _make_request("invoke", {"function": "negate"}, req_id=2)
            + frame
            + _make_request("shutdown", req_id=3)
        )
        stdout = io.BytesIO()
        _serve_function(MultiFunction(), stdin, stdout)

        out = stdout.getvalue()

        # add_one ack + result
        resp, offset = _read_response(out)
        assert resp["id"] == 1
        table, offset = _read_arrow_frame(out, offset)
        assert table.column("result").to_pylist() == [6]

        # negate ack + result
        resp, offset = _read_response(out, offset)
        assert resp["id"] == 2
        table, offset = _read_arrow_frame(out, offset)
        assert table.column("result").to_pylist() == [-5]


# ======================== Handshake Test ========================


class TestHandshake:
    def test_handshake_returns_protocol_version(self):
        stdin = io.BytesIO(
            _make_request("handshake", req_id=1)
            + _make_request("shutdown", req_id=2)
        )
        stdout = io.BytesIO()
        _serve_function(DoubleFunction(), stdin, stdout)

        out = stdout.getvalue()
        resp, _ = _read_response(out)
        assert resp["id"] == 1
        assert resp["result"]["protocol_version"] == "1"
