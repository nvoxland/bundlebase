"""Version-pinned Arrow pushdown investigation; run with poetry run python.

Dataset subclassing below is an experiment, not a supported Bundlebase API.
Counts are logical source batch/column requests, not physical file IO.
"""

import json
import subprocess
import sys

import duckdb
import pyarrow as pa
import pyarrow.dataset as ds


SCHEMA = pa.schema([("id", pa.int64()), ("payload", pa.string())])


class Source:
    def __init__(self):
        self.batch_requests = 0
        self.column_requests = 0
        self.exports = []

    def reader(self):
        def batches():
            for start in range(0, 80, 10):
                self.batch_requests += 1
                self.column_requests += 2
                yield pa.record_batch(
                    [list(range(start, start + 10)), ["data"] * 10], schema=SCHEMA
                )

        return pa.RecordBatchReader.from_batches(SCHEMA, batches())

    @property
    def schema(self):
        return SCHEMA

    def __arrow_c_schema__(self):
        return SCHEMA.__arrow_c_schema__()

    def __arrow_c_stream__(self, requested_schema=None):
        self.exports.append(requested_schema is not None)
        return self.reader().__arrow_c_stream__(requested_schema)


class DuckTypedDataset:
    schema = SCHEMA

    def scanner(self, **kwargs):
        raise AssertionError("DuckDB unexpectedly accepted a duck-typed Dataset")


class ExperimentalDataset(ds.Dataset):
    """Uninitialized Arrow C++ base: only testing DuckDB's Python dispatch."""

    def __init__(self, enforce):
        self.source = Source()
        self.enforce = enforce
        self.calls = []

    @property
    def schema(self):
        return SCHEMA

    def scanner(self, **kwargs):
        self.calls.append(
            {"columns": kwargs.get("columns"), "has_filter": "filter" in kwargs}
        )
        if not self.enforce:
            kwargs.pop("filter", None)  # Deliberately wrong: demonstrates lost residual.
        return ds.Scanner.from_batches(self.source.reader(), **kwargs)


def main():
    con = duckdb.connect(config={"threads": 1})
    report = {"duckdb": duckdb.__version__, "pyarrow": pa.__version__}
    source = Source()
    con.register("capsule_source", source)
    for _ in range(2):
        assert con.sql("SELECT sum(id) FROM capsule_source WHERE id >= 70").fetchone() == (745,)
    assert source.batch_requests == 16
    assert not any(source.exports)
    report["capsule"] = {
        "scans": 2,
        "source_batches": source.batch_requests,
        "source_column_requests": source.column_requests,
        "requested_schema_supplied": source.exports,
        "result": 745,
    }
    try:
        con.register("fake_dataset", DuckTypedDataset())
    except duckdb.InvalidInputException as exc:
        report["duck_typed_dataset"] = str(exc)
    else:
        raise AssertionError("Duck-typed Dataset unexpectedly registered")

    con.close()
    # The subclass intentionally bypasses C++ initialization. Isolate it so
    # future Arrow releases cannot crash the caller running the safe probes.
    probe = subprocess.run(
        [sys.executable, __file__, "--dataset-probe"],
        check=True, capture_output=True, text=True, timeout=30,
    )
    report.update(json.loads(probe.stdout))
    print(json.dumps(report, indent=2))


def dataset_probe():
    con = duckdb.connect(config={"threads": 1})
    report = {}
    for enforce in (True, False):
        dataset = ExperimentalDataset(enforce)
        con.register("experimental", dataset)
        results = [con.sql("SELECT sum(id) FROM experimental WHERE id >= 70").fetchone()[0] for _ in range(2)]
        assert results == ([745, 745] if enforce else [3160, 3160])
        assert dataset.source.batch_requests == 16
        assert all(call["has_filter"] for call in dataset.calls)
        report[f"dataset_subclass_enforce_{enforce}"] = {
            "results": results,
            "scan_requests": dataset.calls,
            "source_batches": dataset.source.batch_requests,
            "source_column_requests": dataset.source.column_requests,
        }
    con.close()
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    if sys.argv[1:] == ["--dataset-probe"]:
        dataset_probe()
    else:
        main()
