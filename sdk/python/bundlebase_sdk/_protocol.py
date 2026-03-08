"""Internal JSON-RPC 2.0 and Arrow IPC protocol handling."""

import json
import struct
from typing import IO, Any

import pyarrow as pa


def read_request(stdin: IO[bytes]) -> dict | None:
    """Read a single JSON-RPC request line from stdin.

    Returns None on EOF.
    """
    line = stdin.readline()
    if not line:
        return None
    line = line.strip()
    if not line:
        return None
    return json.loads(line)


def write_response(stdout: IO[bytes], id: Any, result: Any = None, error: dict | None = None) -> None:
    """Write a JSON-RPC response line to stdout."""
    resp: dict[str, Any] = {"jsonrpc": "2.0", "id": id}
    if error is not None:
        resp["error"] = error
    else:
        resp["result"] = result
    line = json.dumps(resp) + "\n"
    stdout.write(line.encode("utf-8"))
    stdout.flush()


def write_error(stdout: IO[bytes], id: Any, code: int, message: str) -> None:
    """Write a JSON-RPC error response."""
    write_response(stdout, id, error={"code": code, "message": message})


def write_arrow_ipc(stdout: IO[bytes], batches: list[pa.RecordBatch]) -> None:
    """Write length-prefixed Arrow IPC stream bytes to stdout.

    Protocol: 4-byte big-endian u32 length, then Arrow IPC stream bytes.
    Zero-length frame means no data.
    """
    if not batches:
        stdout.write(struct.pack(">I", 0))
        stdout.flush()
        return

    sink = pa.BufferOutputStream()
    writer = pa.ipc.new_stream(sink, batches[0].schema)
    for batch in batches:
        writer.write_batch(batch)
    writer.close()
    data = sink.getvalue().to_pybytes()

    stdout.write(struct.pack(">I", len(data)))
    stdout.write(data)
    stdout.flush()


TYPE_MAP: dict[str, pa.DataType] = {
    "string": pa.string(), "utf8": pa.string(),
    "int8": pa.int8(), "int16": pa.int16(), "int32": pa.int32(), "int64": pa.int64(),
    "uint8": pa.uint8(), "uint16": pa.uint16(), "uint32": pa.uint32(), "uint64": pa.uint64(),
    "float16": pa.float16(), "float32": pa.float32(), "float64": pa.float64(),
    "float": pa.float64(), "double": pa.float64(), "int": pa.int64(),
    "bool": pa.bool_(), "boolean": pa.bool_(),
    "date32": pa.date32(), "date64": pa.date64(), "date": pa.date32(),
    "timestamp": pa.timestamp("us"), "binary": pa.binary(), "bytes": pa.binary(),
}


def schema_to_arrow(schema: dict[str, str]) -> pa.Schema:
    """Convert a dict of {column_name: type_string} to a PyArrow Schema.

    Raises ValueError for unknown type strings.
    """
    fields = []
    for name, type_str in schema.items():
        if type_str not in TYPE_MAP:
            raise ValueError(
                f"Unknown type '{type_str}' for column '{name}'. "
                f"Supported types: {', '.join(sorted(TYPE_MAP.keys()))}"
            )
        fields.append(pa.field(name, TYPE_MAP[type_str]))
    return pa.schema(fields)


def normalize_to_batches(
    data: Any, schema: dict[str, str] | None = None
) -> list[pa.RecordBatch]:
    """Convert various data return types to a list of RecordBatch.

    Supports: pa.Table, pa.RecordBatch, list[RecordBatch],
    list[dict], dict[str, list] (column-oriented), iterator of dicts.

    If schema is provided (a dict mapping column names to type strings),
    it is used for explicit Arrow type conversion.
    """
    arrow_schema = schema_to_arrow(schema) if schema else None

    if data is None:
        return []

    if isinstance(data, pa.Table):
        return data.to_batches()

    if isinstance(data, pa.RecordBatch):
        return [data]

    if isinstance(data, dict):
        # Column-oriented dict: {"col": [values, ...]}
        if arrow_schema is None:
            raise ValueError(
                "schema() is required when returning dict data. "
                "Define a schema() method on your Connector."
            )
        table = pa.table(data, schema=arrow_schema)
        return table.to_batches()

    if isinstance(data, list):
        if not data:
            return []
        if isinstance(data[0], pa.RecordBatch):
            return data
        if isinstance(data[0], dict):
            if arrow_schema is None:
                raise ValueError(
                    "schema() is required when returning dict data. "
                    "Define a schema() method on your Connector."
                )
            table = pa.Table.from_pylist(data, schema=arrow_schema)
            return table.to_batches()

    # Try as iterator of dicts
    try:
        rows = list(data)
        if rows and isinstance(rows[0], dict):
            if arrow_schema is None:
                raise ValueError(
                    "schema() is required when returning dict data. "
                    "Define a schema() method on your Connector."
                )
            table = pa.Table.from_pylist(rows, schema=arrow_schema)
            return table.to_batches()
    except TypeError:
        pass

    raise TypeError(f"Unsupported data return type: {type(data)}")
