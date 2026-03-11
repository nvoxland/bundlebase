"""Entry point for running a connector as a subprocess."""

import json
import struct
import sys
from typing import IO

import pyarrow as pa

from bundlebase_sdk.source import Connector
from bundlebase_sdk.types import Location, StableUrl
from bundlebase_sdk._protocol import (
    write_response,
    write_error,
    write_arrow_ipc,
    normalize_to_batches,
)


def _buffer_arrow_ipc(batches: list[pa.RecordBatch]) -> bytes:
    """Serialize Arrow IPC to bytes buffer (for buffering before ack)."""
    if not batches:
        return struct.pack(">I", 0)

    sink = pa.BufferOutputStream()
    writer = pa.ipc.new_stream(sink, batches[0].schema)
    for batch in batches:
        writer.write_batch(batch)
    writer.close()
    data = sink.getvalue().to_pybytes()

    return struct.pack(">I", len(data)) + data


def _serve(source: Connector, stdin: IO[bytes], stdout: IO[bytes]) -> None:
    """Run the JSON-RPC serve loop with explicit IO streams (for testing)."""
    while True:
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
            elif method == "discover":
                _handle_discover(source, req_id, params, stdout)
            elif method == "data":
                _handle_data(source, req_id, params, stdout)
            elif method == "stable_url":
                _handle_stable_url(source, req_id, params, stdout)
            elif method == "shutdown":
                write_response(stdout, req_id, {"ok": True})
                break
            else:
                write_error(stdout, req_id, -32601, f"Method not found: {method}")
        except Exception as e:
            write_error(stdout, req_id, -32000, str(e))


def _handle_discover(
    source: Connector,
    req_id: int,
    params: dict,
    stdout: IO[bytes],
) -> None:
    attached = params.pop("attached_locations", [])
    locations = source.discover(attached, **params)
    write_response(
        stdout,
        req_id,
        {"locations": [loc.to_dict() for loc in locations]},
    )


def _handle_data(
    source: Connector,
    req_id: int,
    params: dict,
    stdout: IO[bytes],
) -> None:
    loc_dict = params.pop("location", {})
    location = Location.from_dict(loc_dict)

    kwargs = {k: v for k, v in params.items()}
    data = source.data(location, **kwargs)

    batches = normalize_to_batches(data, schema=source.schema())

    # Buffer Arrow IPC first so we can send an error if serialization fails
    buf = _buffer_arrow_ipc(batches)

    write_response(stdout, req_id, {"ok": True})
    stdout.write(buf)
    stdout.flush()


def _handle_stable_url(
    source: Connector,
    req_id: int,
    params: dict,
    stdout: IO[bytes],
) -> None:
    loc_dict = params.pop("location", {})
    location = Location.from_dict(loc_dict)

    kwargs = {k: v for k, v in params.items()}
    result = source.stable_url(location, **kwargs)

    if isinstance(result, StableUrl):
        write_response(stdout, req_id, {"url": result.url})
    else:
        write_response(stdout, req_id, None)


def serve(source: Connector) -> None:
    """Run the connector as a JSON-RPC subprocess.

    Reads requests from stdin and writes responses to stdout.
    This is the main entry point for connector scripts.
    """
    _serve(source, sys.stdin.buffer, sys.stdout.buffer)
