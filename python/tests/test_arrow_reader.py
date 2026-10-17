"""Arrow ownership/runtime binding coverage with a real DuckDB consumer."""

import asyncio
import gc
from datetime import datetime, timezone

import duckdb
import pyarrow as pa
import pyarrow.parquet as pq
import pytest

import bundlebase as bb
import bundlebase.sync as sync

pytestmark = pytest.mark.timeout(30)


@pytest.fixture
def data_path(tmp_path):
    path = tmp_path / "sales.parquet"
    table = pa.table({
        "id": pa.array([1, 2, 3], type=pa.int64()),
        "amount": pa.array([10, None, 30], type=pa.int64()),
        "tags": pa.array([["a"], [], None], type=pa.list_(pa.string())),
        "at": pa.array([datetime(2026, 1, 1, tzinfo=timezone.utc)] * 3,
                       type=pa.timestamp("us", tz="UTC")),
        "region": pa.array(["east", "west", "east"]),
    })
    pq.write_table(table, path, row_group_size=1)
    return path


def consume_duckdb(reader):
    with duckdb.connect() as con:
        con.register("sales", reader)
        return con.sql("SELECT SUM(amount), COUNT(*) FROM sales").fetchone()


@pytest.mark.asyncio
async def test_async_reader_terminal_and_query_ownership(tmp_path, data_path):
    reader = await bb.create(str(tmp_path / "chain")).attach(data_path.as_uri()).arrow_reader()
    assert isinstance(reader, pa.RecordBatchReader)
    assert hasattr(reader, "__arrow_c_stream__")
    assert await asyncio.to_thread(consume_duckdb, reader) == (40, 3)

    builder = await bb.create(str(tmp_path / "bundle")).attach(data_path.as_uri())
    result = await builder.query("SELECT id, amount FROM bundle WHERE id > $1 ORDER BY id", [1])
    selected = await result.arrow_reader()
    with pytest.raises(RuntimeError, match="consumed"):
        await result.arrow_reader()
    with pytest.raises(RuntimeError, match="consumed"):
        await result.to_dict()
    assert (await asyncio.to_thread(selected.read_all)).to_pydict() == {
        "id": [2, 3], "amount": [None, 30]
    }
    with pytest.raises(StopIteration):
        selected.read_next_batch()

    # Cached materializations must not reopen an already consumed query stream.
    cached = await builder.query("SELECT id FROM bundle")
    await cached.to_dict()
    with pytest.raises(RuntimeError, match="consumed"):
        await cached.arrow_reader()


@pytest.mark.asyncio
async def test_empty_schema_and_native_peek_transfer(tmp_path, data_path):
    builder = await bb.create(str(tmp_path / "bundle")).attach(data_path.as_uri())
    result = await builder.query("SELECT id, at, tags FROM bundle WHERE FALSE")
    empty = await result.arrow_reader()
    assert empty.schema.names == ["id", "at", "tags"]
    assert empty.schema.field("id").type == pa.int64()
    assert pa.types.is_timestamp(empty.schema.field("at").type)
    assert pa.types.is_list(empty.schema.field("tags").type)
    assert await asyncio.to_thread(lambda: duckdb.from_arrow(empty).count("*").fetchone()) == (0,)

    stream = await builder.as_pyarrow_stream()
    assert not await stream.is_empty()
    reader = stream.arrow_reader()
    assert (await asyncio.to_thread(reader.read_all)).num_rows == 3
    with pytest.raises(RuntimeError, match="transfer|reader"):
        await stream.next_batch()
    with pytest.raises(RuntimeError, match="transfer|reader"):
        stream.arrow_reader()


def test_sync_reader_reopen_types_and_independent_streams(tmp_path, data_path):
    location = str(tmp_path / "bundle")
    builder = sync.create(location).attach(data_path.as_uri())
    first = builder.arrow_reader()
    second = builder.arrow_reader()
    first_batch = first.read_next_batch()
    first.close()
    assert first_batch.num_rows > 0
    rows = sorted(second.read_all().to_pylist(), key=lambda row: row["id"])
    assert [row["tags"] for row in rows] == [["a"], [], None]
    assert [row["region"] for row in rows] == ["east", "west", "east"]
    assert rows[0]["at"] == datetime(2026, 1, 1, tzinfo=timezone.utc)
    builder.commit("reader fixture")
    readonly = sync.open(location)
    assert consume_duckdb(readonly.arrow_reader()) == (40, 3)
    result = readonly.query("SELECT id FROM bundle")
    assert result.arrow_reader().read_all().num_rows == 3
    with pytest.raises(RuntimeError, match="consumed"):
        result.arrow_reader()


def test_reader_outlives_original_asyncio_loop_and_wrappers(tmp_path, data_path):
    async def prepare():
        builder = await bb.create(str(tmp_path / "bundle")).attach(data_path.as_uri())
        return await builder.arrow_reader()

    reader = asyncio.run(prepare())
    gc.collect()
    assert consume_duckdb(reader) == (40, 3)
    reader.close()


@pytest.mark.asyncio
async def test_reader_freezes_prepared_builder_and_view(tmp_path, data_path):
    builder = await bb.create(str(tmp_path / "bundle")).attach(data_path.as_uri())
    old = await builder.arrow_reader()
    await builder.update("SET amount = 50 WHERE id = 1")
    await builder.delete("id = 3")
    await builder.commit("edits")
    await builder.create_view("selected", "SELECT id, amount FROM bundle WHERE id = 1")
    await builder.commit("view")
    view = await builder.view("selected")
    assert await asyncio.to_thread(consume_duckdb, old) == (40, 3)
    assert await asyncio.to_thread(consume_duckdb, await view.arrow_reader()) == (50, 1)


@pytest.mark.asyncio
async def test_python_udf_reader_gil_and_worker_consumption(tmp_path, data_path):
    builder = await bb.create(str(tmp_path / "bundle"), config={"system": {"allow_external_code": "true"}})
    await builder.attach(data_path.as_uri())
    await builder.import_temp_function("test.double_val", "python::test_function_helpers:double_val")
    result = await builder.query("SELECT test.double_val(id) AS doubled FROM bundle ORDER BY id")
    reader = await result.arrow_reader()
    assert (await asyncio.to_thread(reader.read_all))["doubled"].to_pylist() == [2, 4, 6]
    # The native read callback must also work directly on Python's calling thread.
    result = await builder.query("SELECT test.double_val(id) AS doubled FROM bundle ORDER BY id")
    assert (await result.arrow_reader()).read_all()["doubled"].to_pylist() == [2, 4, 6]


@pytest.mark.asyncio
async def test_reader_surfaces_execution_error(tmp_path, data_path):
    builder = await bb.create(str(tmp_path / "bundle"), config={"system": {"allow_external_code": "true"}})
    await builder.attach(data_path.as_uri())
    await builder.import_temp_function("test.explode", "python::test_arrow_reader:fail_arrow_batch")
    result = await builder.query("SELECT test.explode(id) FROM bundle")
    reader = await result.arrow_reader()
    with pytest.raises((pa.ArrowException, OSError), match="arrow bridge failure"):
        await asyncio.to_thread(reader.read_all)
    reader.close()


def fail_arrow_batch(value):
    raise ValueError("arrow bridge failure")


def bundlebase_metadata():
    return {"functions": [{"name": "fail_arrow_batch", "input_types": ["Int64"],
                           "return_type": "Int64", "kind": "scalar"}]}
