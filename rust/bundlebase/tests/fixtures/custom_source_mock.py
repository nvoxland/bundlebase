#!/usr/bin/env python3
"""Mock subprocess implementing a custom connector using the Bundlebase SDK.

Used by integration tests in rust/bundlebase/src/source/ipc.rs.
"""

import pyarrow as pa

from bundlebase_sdk import Connector, Location, serve


class TestConnector(Connector):
    def discover(self, attached_locations, **kwargs):
        return [
            Location("test_file_1.parquet", must_copy=True, format="parquet", version="v1"),
            Location("test_file_2.parquet", must_copy=True, format="parquet", version="v1"),
        ]

    def data(self, location, **kwargs):
        schema = pa.schema([("id", pa.int64()), ("name", pa.string())])
        if location.location == "test_file_1.parquet":
            return [
                pa.record_batch({"id": [1, 2], "name": ["alice", "bob"]}, schema=schema),
                pa.record_batch({"id": [3], "name": ["charlie"]}, schema=schema),
            ]
        elif location.location == "test_file_2.parquet":
            return pa.table({"id": [4, 5], "name": ["dave", "eve"]})
        return None


if __name__ == "__main__":
    serve(TestConnector())
