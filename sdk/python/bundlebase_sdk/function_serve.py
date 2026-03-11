"""Entry point for running a function provider as a subprocess.

Uses JSON-RPC 2.0 + Arrow IPC over stdin/stdout.
Supports both scalar and aggregate functions with server-side state management.
"""

import json
import struct
import sys
import threading
import time
from typing import IO

import pyarrow as pa

from bundlebase_sdk.function import Function
from bundlebase_sdk._protocol import (
    write_response,
    write_error,
    write_arrow_ipc,
)


def _read_arrow_ipc(stdin: IO[bytes]) -> list[pa.RecordBatch]:
    """Read length-prefixed Arrow IPC stream from stdin.

    Protocol: 4-byte big-endian u32 length, then Arrow IPC stream bytes.
    """
    len_bytes = stdin.read(4)
    if len(len_bytes) < 4:
        raise IOError("Unexpected EOF reading Arrow IPC length prefix")
    data_len = struct.unpack(">I", len_bytes)[0]

    if data_len == 0:
        return []

    data = stdin.read(data_len)
    if len(data) < data_len:
        raise IOError(
            f"Unexpected EOF reading Arrow IPC data: expected {data_len}, got {len(data)}"
        )

    reader = pa.ipc.open_stream(data)
    return reader.read_all().to_batches()


class _AggregateStateStore:
    """Thread-safe store for aggregate function state held server-side."""

    def __init__(self):
        self._states: dict[str, tuple[object, float]] = {}
        self._next_id = 0
        self._lock = threading.Lock()

    def create(self, state: object) -> str:
        with self._lock:
            state_id = str(self._next_id)
            self._next_id += 1
            self._states[state_id] = (state, time.monotonic())
            return state_id

    def get(self, state_id: str) -> object:
        entry = self._states.get(state_id)
        if entry is None:
            return None
        return entry[0]

    def put(self, state_id: str, state: object) -> None:
        # Preserve original creation timestamp
        existing = self._states.get(state_id)
        created_at = existing[1] if existing is not None else time.monotonic()
        self._states[state_id] = (state, created_at)

    def remove(self, state_id: str) -> None:
        self._states.pop(state_id, None)

    def cleanup(self, ttl_seconds: float = 300.0) -> None:
        """Remove states older than ttl_seconds."""
        now = time.monotonic()
        with self._lock:
            expired = [
                sid for sid, (_, created_at) in self._states.items()
                if now - created_at > ttl_seconds
            ]
            for sid in expired:
                del self._states[sid]


def _serve_function(func: Function, stdin: IO[bytes], stdout: IO[bytes]) -> None:
    """Run the JSON-RPC serve loop for functions with explicit IO streams."""
    state_store = _AggregateStateStore()
    last_cleanup = time.monotonic()

    while True:
        # Periodically clean up expired aggregate state
        now = time.monotonic()
        if now - last_cleanup >= 60.0:
            state_store.cleanup()
            last_cleanup = now

        line = stdin.readline()
        if not line:
            break
        line = line.strip()
        if not line:
            continue

        try:
            req = json.loads(line)
        except json.JSONDecodeError as e:
            write_error(stdout, None, -32700, f"Parse error: {e}")
            continue

        method = req.get("method", "")
        req_id = req.get("id")
        params = req.get("params", {})

        try:
            if method == "handshake":
                write_response(stdout, req_id, {"protocol_version": "1"})
            elif method == "ping":
                write_response(stdout, req_id, "pong")
            elif method == "invoke":
                _handle_invoke(func, req_id, params, stdin, stdout)
            elif method == "create_state":
                _handle_create_state(func, req_id, params, state_store, stdout)
            elif method == "accumulate":
                _handle_accumulate(func, req_id, params, state_store, stdin, stdout)
            elif method == "merge":
                _handle_merge(func, req_id, params, state_store, stdout)
            elif method == "evaluate":
                _handle_evaluate(func, req_id, params, state_store, stdout)
            elif method == "shutdown":
                write_response(stdout, req_id, {"ok": True})
                break
            else:
                write_error(stdout, req_id, -32601, f"Method not found: {method}")
        except json.JSONDecodeError as e:
            write_error(stdout, req_id, -32700, f"JSON parse error: {e}")
        except KeyError as e:
            write_error(stdout, req_id, -32602, f"Missing required param: {e}")
        except (TypeError, ValueError) as e:
            write_error(stdout, req_id, -32602, f"Invalid params: {e}")
        except pa.ArrowInvalid as e:
            write_error(stdout, req_id, -32000, f"Arrow error: {e}")
        except Exception as e:
            write_error(stdout, req_id, -32000, f"Internal error: {e}")


def _handle_invoke(
    func: Function,
    req_id: int,
    params: dict,
    stdin: IO[bytes],
    stdout: IO[bytes],
) -> None:
    func_name = params.get("function", "")
    # Acknowledge the request
    write_response(stdout, req_id, {"ok": True})

    # Read input Arrow IPC
    batches = _read_arrow_ipc(stdin)
    if not batches:
        # No input data — write empty output
        write_arrow_ipc(stdout, [])
        return

    # Invoke the function with the input batch
    input_batch = batches[0]
    result_batch = func.invoke(func_name, input_batch)

    # Write output Arrow IPC
    if isinstance(result_batch, pa.RecordBatch):
        write_arrow_ipc(stdout, [result_batch])
    elif isinstance(result_batch, pa.Array):
        # Wrap single array in a RecordBatch
        rb = pa.record_batch({"result": result_batch})
        write_arrow_ipc(stdout, [rb])
    else:
        raise TypeError(
            f"Function '{func_name}' must return pa.RecordBatch or pa.Array, "
            f"got {type(result_batch)}"
        )


def _handle_create_state(
    func: Function,
    req_id: int,
    params: dict,
    state_store: _AggregateStateStore,
    stdout: IO[bytes],
) -> None:
    func_name = params.get("function", "")
    state = func.create_state(func_name)
    state_id = state_store.create(state)
    write_response(stdout, req_id, {"state_id": state_id})


def _handle_accumulate(
    func: Function,
    req_id: int,
    params: dict,
    state_store: _AggregateStateStore,
    stdin: IO[bytes],
    stdout: IO[bytes],
) -> None:
    func_name = params.get("function", "")
    state_id = params.get("state_id", "")

    # Acknowledge the request
    write_response(stdout, req_id, {"ok": True})

    # Read input Arrow IPC batch
    batches = _read_arrow_ipc(stdin)
    if not batches:
        return

    state = state_store.get(state_id)
    if state is None:
        raise ValueError(f"Unknown state ID '{state_id}' for function '{func_name}'")

    updated_state = func.accumulate(func_name, state, batches[0])
    state_store.put(state_id, updated_state)


def _handle_merge(
    func: Function,
    req_id: int,
    params: dict,
    state_store: _AggregateStateStore,
    stdout: IO[bytes],
) -> None:
    func_name = params.get("function", "")
    id1 = params.get("state_id1", "")
    id2 = params.get("state_id2", "")

    state1 = state_store.get(id1)
    state2 = state_store.get(id2)
    if state1 is None or state2 is None:
        raise ValueError(f"Unknown state ID in merge for '{func_name}'")

    merged = func.merge(func_name, state1, state2)
    merged_id = state_store.create(merged)
    write_response(stdout, req_id, {"state_id": merged_id})


def _handle_evaluate(
    func: Function,
    req_id: int,
    params: dict,
    state_store: _AggregateStateStore,
    stdout: IO[bytes],
) -> None:
    func_name = params.get("function", "")
    state_id = params.get("state_id", "")

    # Acknowledge, then send Arrow IPC result
    write_response(stdout, req_id, {"ok": True})

    state = state_store.get(state_id)
    if state is None:
        raise ValueError(f"Unknown state ID '{state_id}' for evaluate '{func_name}'")

    result = func.evaluate(func_name, state)

    # Encode result as Arrow IPC (single-row, single-column)
    if isinstance(result, pa.Scalar):
        arr = pa.array([result.as_py()], type=result.type)
    else:
        arr = pa.array([result])
    rb = pa.record_batch({"result": arr})
    write_arrow_ipc(stdout, [rb])

    # Clean up state after evaluation
    state_store.remove(state_id)


def _build_manifest(func: Function) -> str:
    """Build JSON manifest from the function provider."""
    functions = func.functions()
    return json.dumps({"functions": functions})


def serve_function(func: Function) -> None:
    """Run the function provider as a JSON-RPC subprocess.

    Handles both:
    - `--bundlebase-functions` CLI flag for manifest discovery
    - JSON-RPC serve loop for scalar and aggregate function invocation

    This is the main entry point for function provider scripts.
    """
    if "--bundlebase-functions" in sys.argv:
        manifest = _build_manifest(func)
        sys.stdout.write(manifest)
        sys.stdout.flush()
        return

    _serve_function(func, sys.stdin.buffer, sys.stdout.buffer)
