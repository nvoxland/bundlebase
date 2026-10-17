# bundlebase-ossie

Standalone Rust types, JSON/YAML parsing, and offline validation for the pinned
Apache Ossie semantic model standard. This crate depends only on serde,
serde_json, serde_yaml_ng, and jsonschema. It has no Bundlebase core or query
engine dependency.

```rust
use bundlebase_ossie::{parse_document, to_json, DocumentFormat};

let document = parse_document(
    "version: 0.2.0.dev0\nname: retail\ndatasets:\n  - name: sales\n    source: public.sales\n",
    DocumentFormat::Yaml,
)?;
let json = to_json(&document)?;
# Ok::<(), bundlebase_ossie::OssieError>(())
```

`parse_document`, `validate_document`, `to_json`, and `to_yaml` work with
`serde_json::Value`. `parse_model` and `SemanticModel::from_value` provide the
complete typed standard. Typed models preserve optional fields, empty arrays,
arbitrary structured AI context keys, vendor names, and vendor data strings.
Schema violations expose RFC 6901 instance and schema pointers through
`OssieError::Validation`; display messages also include those pointers.

The full standard accepts every declared dialect. An integration projecting a
native executable model must independently enforce its supported dialect and
expression subset. This crate does not execute SQL, resolve dataset references,
check relationship cardinality, or enforce adapter-specific portability rules.
It rejects versions other than the pinned version, with no compatibility mode.
Serialization validates before producing output.

Schema and examples are embedded or vendored locally. jsonschema's default
network/file resolver features are disabled and validator construction explicitly
uses offline mode. See [vendor/README.md](vendor/README.md) and
[vendor/manifest.json](vendor/manifest.json) for immutable provenance and hashes.
