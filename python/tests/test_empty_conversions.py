"""Binding regressions for schema-preserving empty conversions."""

from datetime import datetime, timezone

import numpy as np
import pyarrow as pa
import pyarrow.parquet as pq
import pytest

import bundlebase as bb
import bundlebase.sync as sync


@pytest.fixture
def data_path(tmp_path):
    path = tmp_path / "typed.parquet"
    schema = pa.schema([
        pa.field("id", pa.int64(), nullable=False, metadata={b"unit": b"count"}),
        pa.field("amount", pa.float64()),
        pa.field("at", pa.timestamp("us", tz="UTC")),
    ], metadata={b"fixture": b"empty conversions"})
    pq.write_table(pa.Table.from_pydict({
        "id": [1, 2], "amount": [1.5, None],
        "at": [datetime(2026, 1, 1, tzinfo=timezone.utc), None],
    }, schema=schema), path)
    return path


@pytest.mark.asyncio
async def test_schema_export_does_not_consume_stream(tmp_path, data_path):
    b = await bb.create(str(tmp_path / "b")).attach(data_path.as_uri())
    stream = await b.as_pyarrow_stream()
    schema = stream.schema.to_pyarrow()
    assert isinstance(schema, pa.Schema)
    reader = stream.arrow_reader()
    assert schema.equals(reader.schema, check_metadata=True)
    assert reader.read_all().num_rows == 2


@pytest.mark.asyncio
@pytest.mark.parametrize("suffix", ["WHERE FALSE", "LIMIT 0"])
async def test_async_empty_query_keeps_columns_and_types(tmp_path, data_path, suffix):
    b = await bb.create(str(tmp_path / "b")).attach(data_path.as_uri())
    sql = f"SELECT id AS key, amount, at FROM bundle {suffix}"
    schema = (await (await b.query(sql)).arrow_reader()).schema
    expected = pa.Table.from_batches([], schema=schema)
    result = await b.query(sql)
    assert await result.to_dict() == {"key": [], "amount": [], "at": []}
    pandas = await result.to_pandas()
    assert list(pandas.columns) == schema.names
    assert pandas.dtypes.equals(expected.to_pandas().dtypes)
    polars = await result.to_polars()
    import polars as pl
    assert polars.schema == pl.from_arrow(expected).schema
    assert polars.height == 0


@pytest.mark.parametrize("suffix", ["WHERE FALSE", "LIMIT 0"])
def test_sync_empty_query_to_pyarrow_preserves_schema(tmp_path, data_path, suffix):
    b = sync.create(str(tmp_path / "b")).attach(data_path.as_uri())
    sql = f"SELECT id AS key, amount, at FROM bundle {suffix}"
    schema = b.query(sql).arrow_reader().schema
    table = b.query(sql).to_pyarrow()
    assert table.num_rows == 0
    assert table.schema.equals(schema, check_metadata=True)
    assert table.schema.field("key").type == pa.int64()
    assert table.schema.field("at").type == pa.timestamp("us", tz="UTC")
    assert b.query("SELECT id FROM bundle ORDER BY id").to_pyarrow()["id"].to_pylist() == [1, 2]


@pytest.mark.asyncio
async def test_async_empty_bundle_converters(tmp_path, data_path):
    b = await bb.create(str(tmp_path / "b")).attach(data_path.as_uri())
    await b.filter("SELECT * FROM bundle WHERE FALSE")
    schema = (await b.arrow_reader()).schema
    assert await b.to_dict() == {name: [] for name in schema.names}
    arrays = await b.to_numpy()
    assert list(arrays) == schema.names
    assert arrays["id"].dtype == np.dtype("int64")
    assert arrays["amount"].dtype == np.dtype("float64")
    assert all(array.size == 0 for array in arrays.values())
    expected = pa.Table.from_batches([], schema=schema)
    assert (await b.to_pandas()).dtypes.equals(expected.to_pandas().dtypes)
    import polars as pl
    assert (await b.to_polars()).schema == pl.from_arrow(expected).schema


def test_sync_empty_bundle_converters(tmp_path, data_path):
    b = sync.create(str(tmp_path / "b")).attach(data_path.as_uri())
    b.filter("SELECT * FROM bundle WHERE FALSE")
    schema = b.arrow_reader().schema
    assert b.to_dict() == {name: [] for name in schema.names}
    arrays = b.to_numpy()
    assert list(arrays) == schema.names
    assert arrays["id"].dtype == np.dtype("int64")
    assert all(array.size == 0 for array in arrays.values())
    assert list(b.to_pandas().columns) == schema.names
    assert b.to_polars().columns == schema.names


@pytest.mark.asyncio
async def test_empty_table_helper_preserves_execution_errors():
    from bundlebase.conversion import _get_arrow_table

    class FailedBundle:
        async def as_pyarrow_stream(self):
            raise RuntimeError("execution sentinel")

    with pytest.raises(ValueError, match="execution sentinel"):
        await _get_arrow_table(FailedBundle())
