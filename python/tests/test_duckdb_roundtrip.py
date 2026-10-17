"""Smoke coverage for the DuckDB guide's documented export and attach workflow."""

from pathlib import Path

import duckdb
import pyarrow as pa
import pyarrow.parquet as pq
import pytest

import bundlebase.sync as bb

pytestmark = pytest.mark.timeout(30)


def test_duckdb_export_attach_commit_reopen(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    pq.write_table(
        pa.table({"region": ["east", "west", "east"], "amount": [10, 20, 30]}),
        "sales.parquet",
    )
    bundle = bb.create("sales.bundle")
    bundle.attach(Path("sales.parquet").resolve().as_uri())

    with duckdb.connect() as con:
        reader = bundle.arrow_reader()
        try:
            con.register("incoming", reader)
            con.execute("""
                COPY (
                    SELECT region, SUM(amount) AS total
                    FROM incoming GROUP BY region
                ) TO 'regional_totals.parquet' (FORMAT PARQUET)
            """)
        finally:
            reader.close()

    output = bb.create("regional-totals.bundle")
    output.attach(Path("regional_totals.parquet").resolve().as_uri())
    output.commit("Regional totals computed with DuckDB")

    reopened = bb.open("regional-totals.bundle")
    assert reopened.query(
        "SELECT region, total FROM bundle ORDER BY region"
    ).to_dict() == {"region": ["east", "west"], "total": [40, 20]}
