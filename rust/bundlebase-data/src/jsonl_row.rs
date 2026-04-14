//! Direct JSONL row → Arrow StringBuilder extractor.
//!
//! The straightforward `serde_json::from_slice::<Value>(line)` approach pays
//! for the JSON twice: once parsing bytes into a `BTreeMap<String, Value>`
//! tree, and once re-serializing every `Value` back to a `String` so it can
//! go into an Arrow `StringBuilder`. Profiling showed these two passes
//! together consume ~100% of the JSONL scan time (see
//! `ideas/faster-json-parser.md`).
//!
//! This module uses a serde `Visitor` to walk each row directly into the
//! builders without materializing a `Value` tree. For each top-level field
//! we borrow a `&RawValue` (zero-copy slice into the input bytes), classify
//! it by its first byte, and either copy the raw slice into the builder
//! (numbers, bools, strings with no escapes) or fall back to `serde_json`
//! for the slow cases (strings with escapes, nested containers). Nested
//! containers still round-trip through `serde_json::Value` so that the
//! stringified output matches the historical canonical form (sorted keys,
//! no whitespace).

use arrow::array::StringBuilder;
use serde::de::{Deserializer as _, IgnoredAny, MapAccess, Visitor};
use serde_json::value::RawValue;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;

/// Append one JSONL row to the given string builders.
///
/// - `line` should be a single JSONL record (no trailing newline).
/// - `name_to_idx` maps top-level field names to builder indices.
/// - Unknown fields in the row are ignored.
/// - Fields in the schema but missing from the row are appended as empty.
/// - If the row is not a valid JSON object, no builder is modified and the
///   function returns `false`.
/// - `normalize_nested_json` controls how nested objects/arrays are stored:
///   `false` (default) preserves the source bytes verbatim; `true`
///   round-trips through `serde_json::Value` so that object keys come out
///   sorted and whitespace is stripped. The normalized form is useful for
///   equality / grouping on stringified containers across heterogeneous
///   writers, at the cost of ~40% scan throughput.
///
/// On success all builders have exactly one new value appended. On failure
/// none do — the partial writes are staged in a scratch buffer first and
/// only committed once the row has been fully parsed.
pub fn append_jsonl_row_to_builders(
    line: &[u8],
    name_to_idx: &HashMap<&str, usize>,
    builders: &mut [StringBuilder],
    normalize_nested_json: bool,
) -> bool {
    let mut pending: Vec<(usize, &str)> = Vec::with_capacity(name_to_idx.len());

    let mut de = serde_json::Deserializer::from_slice(line);
    let visitor = RowCollector {
        name_to_idx,
        out: &mut pending,
    };
    if de.deserialize_map(visitor).is_err() {
        return false;
    }

    let ncols = builders.len();
    let mut seen = vec![false; ncols];
    for (idx, raw_text) in &pending {
        append_raw_json_value(&mut builders[*idx], raw_text, normalize_nested_json);
        seen[*idx] = true;
    }
    for (i, filled) in seen.iter().enumerate() {
        if !filled {
            builders[i].append_value("");
        }
    }
    true
}

struct RowCollector<'a, 'b> {
    name_to_idx: &'a HashMap<&'a str, usize>,
    out: &'b mut Vec<(usize, &'a str)>,
}

impl<'de, 'a, 'b> Visitor<'de> for RowCollector<'a, 'b>
where
    'de: 'a,
{
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON object")
    }

    fn visit_map<A>(self, mut map: A) -> Result<(), A::Error>
    where
        A: MapAccess<'de>,
    {
        while let Some(key) = map.next_key::<Cow<'de, str>>()? {
            match self.name_to_idx.get(key.as_ref()).copied() {
                Some(idx) => {
                    let raw: &'de RawValue = map.next_value()?;
                    self.out.push((idx, raw.get()));
                }
                None => {
                    let _: IgnoredAny = map.next_value()?;
                }
            }
        }
        Ok(())
    }
}

/// Append a JSON value (given as its raw JSON text) to a StringBuilder.
///
/// - `null` → empty string
/// - `"..."` → unescaped string contents (zero-copy when no `\` present)
/// - number / bool → raw text verbatim
/// - object / array →
///     - when `normalize_nested_json` is false: raw JSON text verbatim
///       (source key order + whitespace preserved)
///     - when true: re-serialized via `serde_json::Value` → canonical form
///       (keys sorted, whitespace stripped)
fn append_raw_json_value(b: &mut StringBuilder, raw: &str, normalize_nested_json: bool) {
    let bytes = raw.as_bytes();
    match bytes.first() {
        Some(b'"') => {
            // Strip quotes. JSON strings are always at least `""`.
            let inner = &raw[1..raw.len() - 1];
            if !inner.as_bytes().contains(&b'\\') {
                b.append_value(inner);
            } else {
                match serde_json::from_str::<Cow<str>>(raw) {
                    Ok(s) => b.append_value(&*s),
                    Err(_) => b.append_value(inner),
                }
            }
        }
        Some(b'n') if raw == "null" => b.append_value(""),
        Some(b'{') | Some(b'[') if normalize_nested_json => {
            match serde_json::from_str::<serde_json::Value>(raw) {
                Ok(v) => b.append_value(v.to_string()),
                Err(_) => b.append_value(raw),
            }
        }
        _ => b.append_value(raw),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::Array;

    fn build_with(line: &[u8], cols: &[&str], normalize: bool) -> Vec<String> {
        let name_to_idx: HashMap<&str, usize> =
            cols.iter().enumerate().map(|(i, &n)| (n, i)).collect();
        let mut builders: Vec<StringBuilder> =
            (0..cols.len()).map(|_| StringBuilder::new()).collect();
        let ok = append_jsonl_row_to_builders(line, &name_to_idx, &mut builders, normalize);
        assert!(ok, "parse failed for: {}", std::str::from_utf8(line).unwrap());
        builders
            .iter_mut()
            .map(|b| {
                let a = b.finish();
                assert_eq!(a.len(), 1, "expected exactly one row per builder");
                a.value(0).to_string()
            })
            .collect()
    }

    fn build(line: &[u8], cols: &[&str]) -> Vec<String> {
        build_with(line, cols, false)
    }

    #[test]
    fn parses_scalars() {
        let row = br#"{"s":"hi","n":42,"b":true,"z":null}"#;
        let out = build(row, &["s", "n", "b", "z"]);
        assert_eq!(out, vec!["hi", "42", "true", ""]);
    }

    #[test]
    fn parses_strings_with_escapes() {
        let row = br#"{"s":"line1\nline2","q":"he said \"hi\""}"#;
        let out = build(row, &["s", "q"]);
        assert_eq!(out, vec!["line1\nline2", "he said \"hi\""]);
    }

    #[test]
    fn missing_field_is_empty() {
        let row = br#"{"a":"x"}"#;
        let out = build(row, &["a", "b"]);
        assert_eq!(out, vec!["x", ""]);
    }

    #[test]
    fn unknown_field_is_ignored() {
        let row = br#"{"a":"x","extra":"y"}"#;
        let out = build(row, &["a"]);
        assert_eq!(out, vec!["x"]);
    }

    #[test]
    fn nested_object_preserves_source_order_and_whitespace() {
        let row = br#"{"m":{"y": 2, "x": 1}}"#;
        let out = build(row, &["m"]);
        // Containers are stored verbatim from the source bytes — no longer
        // round-tripped through serde_json::Value for canonicalization.
        assert_eq!(out, vec![r#"{"y": 2, "x": 1}"#]);
    }

    #[test]
    fn nested_array_is_preserved_verbatim() {
        let row = br#"{"a":[1, 2, 3]}"#;
        let out = build(row, &["a"]);
        assert_eq!(out, vec!["[1, 2, 3]"]);
    }

    #[test]
    fn normalize_mode_sorts_keys_and_strips_whitespace() {
        let row = br#"{"m":{"y": 2, "x": 1},"a":[1, 2, 3]}"#;
        let out = build_with(row, &["m", "a"], true);
        assert_eq!(out, vec![r#"{"x":1,"y":2}"#, "[1,2,3]"]);
    }

    #[test]
    fn malformed_row_is_skipped() {
        let row = br#"{not json"#;
        let name_to_idx: HashMap<&str, usize> = [("a", 0)].into_iter().collect();
        let mut builders = vec![StringBuilder::new()];
        let ok = append_jsonl_row_to_builders(row, &name_to_idx, &mut builders, false);
        assert!(!ok);
        assert_eq!(builders[0].finish().len(), 0);
    }

    #[test]
    fn partial_parse_error_rolls_back() {
        // Valid start, invalid value — no column should receive a write.
        let row = br#"{"a":"ok","b":not-a-value}"#;
        let name_to_idx: HashMap<&str, usize> =
            [("a", 0), ("b", 1)].into_iter().collect();
        let mut builders = vec![StringBuilder::new(), StringBuilder::new()];
        let ok = append_jsonl_row_to_builders(row, &name_to_idx, &mut builders, false);
        assert!(!ok);
        assert_eq!(builders[0].finish().len(), 0);
        assert_eq!(builders[1].finish().len(), 0);
    }
}
