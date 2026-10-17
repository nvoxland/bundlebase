# Vendored Apache Ossie specification

These files are unmodified copies from the official
[Apache Ossie repository](https://github.com/apache/ossie) at immutable commit
[`64a1ae44035f5360dcd53401a29bcd900fdce349`](https://github.com/apache/ossie/tree/64a1ae44035f5360dcd53401a29bcd900fdce349).
The schema declares specification version `0.2.0.dev0`, JSON Schema draft 2020-12.

| Local file | Upstream path |
| --- | --- |
| `ossie-schema.json` | `core-spec/ossie-schema.json` |
| `examples/flights.semantic_model.yaml` | `examples/flights.semantic_model.yaml` |
| `examples/tpcds_semantic_model.yaml` | `examples/tpcds_semantic_model.yaml` |
| `LICENSE` | `LICENSE` |
| `NOTICE` | `NOTICE` |
| `DISCLAIMER` | `DISCLAIMER` |

The two examples are the upstream standalone semantic model documents. Other
upstream example files describe ontology and mapping layers outside this schema.
`manifest.json` records SHA-256 hashes of each copied file. It is provenance
metadata maintained by Bundlebase, rather than an upstream file.

Apache Ossie (incubating) is copyright 2026 The Apache Software Foundation and
licensed under Apache License 2.0. Its license, attribution notice, and incubator
disclaimer are reproduced alongside the schema.

There is no build script, remote schema resolver, or automatic update mechanism.
To upgrade, choose another immutable official commit, replace the copied files,
update the hashes and Rust constants, reconcile typed objects with every schema
definition, and run the upstream example round trips and vocabulary tests.
