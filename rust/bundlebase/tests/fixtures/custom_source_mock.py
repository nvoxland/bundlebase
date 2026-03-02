#!/usr/bin/env python3
"""Mock subprocess implementing the custom source function JSON-RPC protocol.

Used by integration tests in rust/bundlebase/src/source/custom.rs.

Protocol:
- Line-delimited JSON-RPC 2.0 over stdin/stdout
- Arrow IPC with 4-byte big-endian length prefix for data transfer
"""

import json
import struct
import sys

import pyarrow as pa


def send_response(id, result=None, error=None):
    """Send a JSON-RPC response line to stdout."""
    resp = {"jsonrpc": "2.0", "id": id}
    if error is not None:
        resp["error"] = error
    else:
        resp["result"] = result
    line = json.dumps(resp) + "\n"
    sys.stdout.buffer.write(line.encode("utf-8"))
    sys.stdout.buffer.flush()


def send_arrow_ipc(batches):
    """Write length-prefixed Arrow IPC stream bytes to stdout.

    Args:
        batches: A list of pa.RecordBatch, or a pa.Table (written as a single batch).
    """
    if isinstance(batches, pa.Table):
        batches = batches.to_batches()

    if not batches:
        # Zero-length frame
        sys.stdout.buffer.write(struct.pack(">I", 0))
        sys.stdout.buffer.flush()
        return

    sink = pa.BufferOutputStream()
    writer = pa.ipc.new_stream(sink, batches[0].schema)
    for batch in batches:
        writer.write_batch(batch)
    writer.close()
    data = sink.getvalue().to_pybytes()

    # 4-byte big-endian length prefix
    sys.stdout.buffer.write(struct.pack(">I", len(data)))
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()


def handle_discover(id, params):
    """Return two test locations."""
    locations = [
        {
            "location": "test_file_1.parquet",
            "must_copy": True,
            "format": "parquet",
            "version": "v1",
        },
        {
            "location": "test_file_2.parquet",
            "must_copy": True,
            "format": "parquet",
            "version": "v1",
        },
    ]
    send_response(id, {"locations": locations})


def handle_data(id, params):
    """Send Arrow IPC data for the requested location.

    Protocol: JSON-RPC ack, then length-prefixed Arrow IPC frame.
    A zero-length frame means no data.

    test_file_1 sends multiple batches to exercise the streaming pipeline.
    test_file_2 sends a single batch.
    """
    loc = params.get("location", {})
    location = loc.get("location", "") if isinstance(loc, dict) else loc
    send_response(id, {"ok": True})
    if location == "test_file_1.parquet":
        schema = pa.schema([("id", pa.int64()), ("name", pa.string())])
        batch1 = pa.record_batch({"id": [1, 2], "name": ["alice", "bob"]}, schema=schema)
        batch2 = pa.record_batch(
            {"id": [3], "name": ["charlie"]}, schema=schema
        )
        send_arrow_ipc([batch1, batch2])
    elif location == "test_file_2.parquet":
        table = pa.table({"id": [4, 5], "name": ["dave", "eve"]})
        send_arrow_ipc(table)
    else:
        send_arrow_ipc([])


def handle_stable_url(id, params):
    """No stable URL available for test data."""
    send_response(id, None)


def handle_shutdown(id, params):
    """Acknowledge shutdown and exit."""
    send_response(id, {"ok": True})
    sys.exit(0)


def main():
    handlers = {
        "discover": handle_discover,
        "data": handle_data,
        "stable_url": handle_stable_url,
        "shutdown": handle_shutdown,
    }

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except json.JSONDecodeError:
            continue

        method = req.get("method", "")
        id = req.get("id")
        params = req.get("params", {})

        handler = handlers.get(method)
        if handler:
            handler(id, params)
        else:
            send_response(
                id,
                error={"code": -32601, "message": f"Method not found: {method}"},
            )


if __name__ == "__main__":
    main()
