"""End-to-end tests for the serve loop with mock stdin/stdout."""

import io
import json
import struct

import pyarrow as pa

from bundlebase_sdk import Connector, Location, StableUrl
from bundlebase_sdk.serve import _serve


class SimpleSource(Connector):
    """A minimal source for testing."""

    def discover(self, attached_locations, **kwargs):
        return [
            Location("file1.csv", must_copy=True, format="csv", version="v1"),
            Location("file2.csv", must_copy=False, format="csv", version="v2"),
        ]

    def data(self, location, **kwargs):
        if location.location == "file1.csv":
            return pa.table({"id": [1, 2], "name": ["a", "b"]})
        return None

    def stable_url(self, location, **kwargs):
        if location.location == "file1.csv":
            return StableUrl("https://example.com/file1.csv")
        return None


class MultiReturnSource(Connector):
    """Source that tests different data return types."""

    def schema(self):
        return {"x": "int64"}

    def discover(self, attached_locations, **kwargs):
        return [
            Location("table"),
            Location("batch"),
            Location("batch_list"),
            Location("dict_list"),
            Location("none"),
        ]

    def data(self, location, **kwargs):
        schema = pa.schema([("x", pa.int64())])
        if location.location == "table":
            return pa.table({"x": [1, 2]})
        elif location.location == "batch":
            return pa.record_batch({"x": [3]}, schema=schema)
        elif location.location == "batch_list":
            return [
                pa.record_batch({"x": [4]}, schema=schema),
                pa.record_batch({"x": [5]}, schema=schema),
            ]
        elif location.location == "dict_list":
            return [{"x": 10}, {"x": 20}]
        return None


def _make_request(method, params=None, req_id=1):
    """Build a JSON-RPC request line as bytes."""
    req = {"jsonrpc": "2.0", "id": req_id, "method": method, "params": params or {}}
    return json.dumps(req).encode() + b"\n"


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


class TestServeDiscover:
    def test_discover_returns_locations(self):
        stdin = io.BytesIO(_make_request("discover", {"attached_locations": []}))
        stdout = io.BytesIO()
        # Add shutdown to terminate the loop
        stdin = io.BytesIO(
            _make_request("discover", {"attached_locations": []}, req_id=1)
            + _make_request("shutdown", req_id=2)
        )
        _serve(SimpleSource(), stdin, stdout)

        resp, offset = _read_response(stdout.getvalue())
        assert resp["id"] == 1
        locations = resp["result"]["locations"]
        assert len(locations) == 2
        assert locations[0]["location"] == "file1.csv"
        assert locations[0]["must_copy"] is True
        assert locations[1]["location"] == "file2.csv"
        assert locations[1]["must_copy"] is False

    def test_discover_passes_extra_kwargs(self):
        """Extra params (besides attached_locations) are passed as kwargs."""

        class KwargsSource(Connector):
            def discover(self, attached_locations, **kwargs):
                # Echo back kwargs as a location name
                name = kwargs.get("custom_arg", "missing")
                return [Location(name)]

            def data(self, location, **kwargs):
                return None

        stdin = io.BytesIO(
            _make_request("discover", {"attached_locations": [], "custom_arg": "hello"}, req_id=1)
            + _make_request("shutdown", req_id=2)
        )
        stdout = io.BytesIO()
        _serve(KwargsSource(), stdin, stdout)

        resp, _ = _read_response(stdout.getvalue())
        assert resp["result"]["locations"][0]["location"] == "hello"


class TestServeData:
    def test_data_returns_arrow(self):
        stdin = io.BytesIO(
            _make_request(
                "data",
                {"location": {"location": "file1.csv", "must_copy": True, "format": "csv", "version": "v1"}},
                req_id=1,
            )
            + _make_request("shutdown", req_id=2)
        )
        stdout = io.BytesIO()
        _serve(SimpleSource(), stdin, stdout)

        out = stdout.getvalue()
        resp, offset = _read_response(out)
        assert resp["id"] == 1
        assert resp["result"]["ok"] is True

        table, offset = _read_arrow_frame(out, offset)
        assert table is not None
        assert table.num_rows == 2
        assert table.column("id").to_pylist() == [1, 2]

    def test_data_returns_none(self):
        stdin = io.BytesIO(
            _make_request(
                "data",
                {"location": {"location": "nonexistent"}},
                req_id=1,
            )
            + _make_request("shutdown", req_id=2)
        )
        stdout = io.BytesIO()
        _serve(SimpleSource(), stdin, stdout)

        out = stdout.getvalue()
        resp, offset = _read_response(out)
        assert resp["id"] == 1

        table, offset = _read_arrow_frame(out, offset)
        assert table is None


class TestServeStableUrl:
    def test_stable_url_present(self):
        stdin = io.BytesIO(
            _make_request(
                "stable_url",
                {"location": {"location": "file1.csv"}},
                req_id=1,
            )
            + _make_request("shutdown", req_id=2)
        )
        stdout = io.BytesIO()
        _serve(SimpleSource(), stdin, stdout)

        resp, _ = _read_response(stdout.getvalue())
        assert resp["result"]["url"] == "https://example.com/file1.csv"

    def test_stable_url_none(self):
        stdin = io.BytesIO(
            _make_request(
                "stable_url",
                {"location": {"location": "file2.csv"}},
                req_id=1,
            )
            + _make_request("shutdown", req_id=2)
        )
        stdout = io.BytesIO()
        _serve(SimpleSource(), stdin, stdout)

        resp, _ = _read_response(stdout.getvalue())
        assert resp["result"] is None


class TestServeErrorHandling:
    def test_unknown_method(self):
        stdin = io.BytesIO(
            _make_request("bogus", req_id=1)
            + _make_request("shutdown", req_id=2)
        )
        stdout = io.BytesIO()
        _serve(SimpleSource(), stdin, stdout)

        resp, _ = _read_response(stdout.getvalue())
        assert resp["error"]["code"] == -32601
        assert "Method not found" in resp["error"]["message"]

    def test_user_exception_wrapped(self):
        class BrokenSource(Connector):
            def discover(self, attached_locations, **kwargs):
                raise ValueError("something broke")

            def data(self, location, **kwargs):
                return None

        stdin = io.BytesIO(
            _make_request("discover", {"attached_locations": []}, req_id=1)
            + _make_request("shutdown", req_id=2)
        )
        stdout = io.BytesIO()
        _serve(BrokenSource(), stdin, stdout)

        resp, _ = _read_response(stdout.getvalue())
        assert resp["error"]["code"] == -32000
        assert "something broke" in resp["error"]["message"]


class SchemaSource(Connector):
    """Source that uses schema() and returns column-oriented dicts."""

    def schema(self):
        return {"name": "string", "score": "float32"}

    def discover(self, attached_locations, **kwargs):
        return [Location("col_dict"), Location("row_dicts")]

    def data(self, location, **kwargs):
        if location.location == "col_dict":
            return {"name": ["alice", "bob"], "score": [9.5, 8.0]}
        elif location.location == "row_dicts":
            return [{"name": "charlie", "score": 7.5}]
        return None


class TestServeSchemaConnector:
    def _fetch_data(self, location_name):
        stdin = io.BytesIO(
            _make_request(
                "data",
                {"location": {"location": location_name}},
                req_id=1,
            )
            + _make_request("shutdown", req_id=2)
        )
        stdout = io.BytesIO()
        _serve(SchemaSource(), stdin, stdout)
        out = stdout.getvalue()
        _, offset = _read_response(out)
        table, _ = _read_arrow_frame(out, offset)
        return table

    def test_column_dict_with_schema(self):
        table = self._fetch_data("col_dict")
        assert table.num_rows == 2
        assert table.schema.field("name").type == pa.string()
        assert table.schema.field("score").type == pa.float32()
        assert table.column("name").to_pylist() == ["alice", "bob"]

    def test_row_dicts_with_schema(self):
        table = self._fetch_data("row_dicts")
        assert table.num_rows == 1
        assert table.schema.field("score").type == pa.float32()


class TestServeMultiReturnTypes:
    def _fetch_data(self, location_name):
        stdin = io.BytesIO(
            _make_request(
                "data",
                {"location": {"location": location_name}},
                req_id=1,
            )
            + _make_request("shutdown", req_id=2)
        )
        stdout = io.BytesIO()
        _serve(MultiReturnSource(), stdin, stdout)
        out = stdout.getvalue()
        _, offset = _read_response(out)
        table, _ = _read_arrow_frame(out, offset)
        return table

    def test_table_return(self):
        table = self._fetch_data("table")
        assert table.num_rows == 2

    def test_batch_return(self):
        table = self._fetch_data("batch")
        assert table.num_rows == 1

    def test_batch_list_return(self):
        table = self._fetch_data("batch_list")
        assert table.num_rows == 2

    def test_dict_list_return(self):
        table = self._fetch_data("dict_list")
        assert table.num_rows == 2
        assert table.column("x").to_pylist() == [10, 20]

    def test_none_return(self):
        table = self._fetch_data("none")
        assert table is None
