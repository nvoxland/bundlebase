"""Unit tests for the internal protocol module."""

import io
import json
import struct

import pyarrow as pa

import pytest

from bundlebase_sdk._protocol import (
    read_request,
    write_response,
    write_error,
    write_arrow_ipc,
    normalize_to_batches,
    schema_to_arrow,
    TYPE_MAP,
)


class TestReadRequest:
    def test_reads_json_line(self):
        data = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "discover"}).encode() + b"\n"
        stdin = io.BytesIO(data)
        req = read_request(stdin)
        assert req["method"] == "discover"
        assert req["id"] == 1

    def test_returns_none_on_eof(self):
        stdin = io.BytesIO(b"")
        assert read_request(stdin) is None

    def test_returns_none_on_blank_line(self):
        stdin = io.BytesIO(b"\n")
        assert read_request(stdin) is None


class TestWriteResponse:
    def test_writes_result(self):
        stdout = io.BytesIO()
        write_response(stdout, 1, {"locations": []})
        line = stdout.getvalue().decode()
        resp = json.loads(line)
        assert resp["jsonrpc"] == "2.0"
        assert resp["id"] == 1
        assert resp["result"] == {"locations": []}

    def test_writes_error(self):
        stdout = io.BytesIO()
        write_error(stdout, 2, -32601, "Method not found")
        line = stdout.getvalue().decode()
        resp = json.loads(line)
        assert resp["error"]["code"] == -32601
        assert "Method not found" in resp["error"]["message"]


class TestWriteArrowIpc:
    def test_writes_zero_length_for_empty(self):
        stdout = io.BytesIO()
        write_arrow_ipc(stdout, [])
        data = stdout.getvalue()
        assert len(data) == 4
        assert struct.unpack(">I", data)[0] == 0

    def test_writes_valid_ipc_stream(self):
        schema = pa.schema([("x", pa.int64())])
        batch = pa.record_batch({"x": [1, 2, 3]}, schema=schema)
        stdout = io.BytesIO()
        write_arrow_ipc(stdout, [batch])
        data = stdout.getvalue()

        # Read back: 4-byte length prefix + IPC data
        length = struct.unpack(">I", data[:4])[0]
        assert length > 0
        ipc_bytes = data[4:]
        assert len(ipc_bytes) == length

        # Verify it's valid Arrow IPC
        reader = pa.ipc.open_stream(ipc_bytes)
        result = reader.read_all()
        assert result.num_rows == 3
        assert result.column("x").to_pylist() == [1, 2, 3]

    def test_multi_batch(self):
        schema = pa.schema([("v", pa.string())])
        b1 = pa.record_batch({"v": ["a", "b"]}, schema=schema)
        b2 = pa.record_batch({"v": ["c"]}, schema=schema)
        stdout = io.BytesIO()
        write_arrow_ipc(stdout, [b1, b2])

        data = stdout.getvalue()
        length = struct.unpack(">I", data[:4])[0]
        reader = pa.ipc.open_stream(data[4:])
        result = reader.read_all()
        assert result.num_rows == 3
        assert result.column("v").to_pylist() == ["a", "b", "c"]


class TestNormalizeToBatches:
    def test_none_returns_empty(self):
        assert normalize_to_batches(None) == []

    def test_table(self):
        table = pa.table({"x": [1, 2]})
        batches = normalize_to_batches(table)
        assert len(batches) >= 1
        total = sum(b.num_rows for b in batches)
        assert total == 2

    def test_record_batch(self):
        batch = pa.record_batch({"x": [1]}, schema=pa.schema([("x", pa.int64())]))
        batches = normalize_to_batches(batch)
        assert len(batches) == 1
        assert batches[0].num_rows == 1

    def test_list_of_batches(self):
        schema = pa.schema([("x", pa.int64())])
        b1 = pa.record_batch({"x": [1]}, schema=schema)
        b2 = pa.record_batch({"x": [2]}, schema=schema)
        batches = normalize_to_batches([b1, b2])
        assert len(batches) == 2

    def test_list_of_dicts_with_schema(self):
        batches = normalize_to_batches([{"a": 1}, {"a": 2}], schema={"a": "Int64"})
        assert len(batches) >= 1
        total = sum(b.num_rows for b in batches)
        assert total == 2

    def test_list_of_dicts_without_schema_raises(self):
        with pytest.raises(ValueError, match="schema\\(\\) is required"):
            normalize_to_batches([{"a": 1}, {"a": 2}])

    def test_empty_list(self):
        assert normalize_to_batches([]) == []

    def test_iterator_of_dicts_with_schema(self):
        def gen():
            yield {"k": "x"}
            yield {"k": "y"}

        batches = normalize_to_batches(gen(), schema={"k": "Utf8"})
        total = sum(b.num_rows for b in batches)
        assert total == 2

    def test_iterator_of_dicts_without_schema_raises(self):
        def gen():
            yield {"k": "x"}
            yield {"k": "y"}

        with pytest.raises(ValueError, match="schema\\(\\) is required"):
            normalize_to_batches(gen())

    def test_column_oriented_dict_with_schema(self):
        data = {"name": ["alice", "bob"], "age": [30, 25]}
        schema = {"name": "Utf8", "age": "Int32"}
        batches = normalize_to_batches(data, schema=schema)
        assert len(batches) >= 1
        total = sum(b.num_rows for b in batches)
        assert total == 2
        assert batches[0].schema.field("age").type == pa.int32()

    def test_column_oriented_dict_without_schema_raises(self):
        data = {"x": [1, 2, 3], "y": ["a", "b", "c"]}
        with pytest.raises(ValueError, match="schema\\(\\) is required"):
            normalize_to_batches(data)

    def test_list_of_dicts_with_schema(self):
        data = [{"val": 1}, {"val": 2}]
        schema = {"val": "Float64"}
        batches = normalize_to_batches(data, schema=schema)
        assert len(batches) >= 1
        assert batches[0].schema.field("val").type == pa.float64()

    def test_iterator_of_dicts_with_schema(self):
        def gen():
            yield {"k": 1}
            yield {"k": 2}

        batches = normalize_to_batches(gen(), schema={"k": "Int32"})
        assert batches[0].schema.field("k").type == pa.int32()


class TestSchemaToArrow:
    def test_valid_mapping(self):
        schema = schema_to_arrow({"name": "Utf8", "age": "Int64", "active": "Boolean"})
        assert schema.field("name").type == pa.string()
        assert schema.field("age").type == pa.int64()
        assert schema.field("active").type == pa.bool_()

    def test_unknown_type_raises(self):
        with pytest.raises(ValueError, match="Unknown type 'bigint'"):
            schema_to_arrow({"col": "bigint"})

    def test_all_canonical_types(self):
        schema = schema_to_arrow({
            "a": "Utf8", "b": "Int64", "c": "Float64",
            "d": "Boolean", "e": "Binary", "f": "LargeUtf8",
        })
        assert schema.field("a").type == pa.string()
        assert schema.field("b").type == pa.int64()
        assert schema.field("c").type == pa.float64()
        assert schema.field("d").type == pa.bool_()
        assert schema.field("e").type == pa.binary()
        assert schema.field("f").type == pa.large_string()
