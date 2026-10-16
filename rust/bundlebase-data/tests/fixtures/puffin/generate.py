"""Reproduce the wrapper around an unmodified Apache Iceberg golden DV.

Run: python3 rust/bundlebase-data/tests/fixtures/puffin/generate.py
Requires network access, but no Python dependencies. Tests use checked-in files.
"""

import hashlib
import json
from pathlib import Path
import struct
from urllib.request import urlopen
import zlib

REVISION = "2114bf631e49af532d66e2ce148ee49dd1dd1f1f"  # apache-iceberg-1.10.0
SOURCE = (
    f"https://raw.githubusercontent.com/apache/iceberg/{REVISION}/"
    "core/src/test/resources/org/apache/iceberg/deletes/"
    "all-container-types-position-index.bin"
)
SHA256 = "98f569f1e0dfe39d38b83c8b3c852d3ba1e7fd8179bb4da19dc94f5318ec70ae"


def main():
    with urlopen(SOURCE) as response:
        dv = response.read()
    assert hashlib.sha256(dv).hexdigest() == SHA256
    assert struct.unpack(">i", dv[:4])[0] == len(dv) - 8
    assert dv[4:8] == bytes.fromhex("d1d33964")
    assert struct.unpack(">I", dv[-4:])[0] == zlib.crc32(dv[4:-4])
    metadata = {
        "blobs": [{
            "type": "deletion-vector-v1",
            "fields": [],
            "snapshot-id": -1,
            "sequence-number": -1,
            "offset": 4,
            "length": len(dv),
            "properties": {
                "referenced-data-file": "data/iceberg-fixture.parquet",
                "cardinality": "132561",
            },
        }],
    }
    footer = json.dumps(metadata, separators=(",", ":")).encode()
    puffin = (b"PFA1" + dv + b"PFA1" + footer
              + struct.pack("<i", len(footer)) + bytes(4) + b"PFA1")
    directory = Path(__file__).parent
    (directory / "iceberg-1.10.0-all-containers.dv").write_bytes(dv)
    (directory / "iceberg-1.10.0-all-containers.puffin").write_bytes(puffin)


if __name__ == "__main__":
    main()
