//! Uncompressed Puffin files containing Iceberg `deletion-vector-v1` blobs.
//!
//! Wire contract: Apache Iceberg `apache-iceberg-1.10.0/format/puffin-spec.md`.
//! Block identities belong to the caller; this module validates the container,
//! portable bitmap framing, checksum, and standard deletion-vector properties.

use bundlebase_common::BundlebaseError;
pub use roaring::RoaringTreemap;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const MAGIC: &[u8; 4] = b"PFA1";
const DV_MAGIC: &[u8; 4] = &[0xd1, 0xd3, 0x39, 0x64];
const DV_TYPE: &str = "deletion-vector-v1";

/// A standard deletion vector and its string-valued Puffin blob properties.
#[derive(Clone, Debug, PartialEq)]
pub struct DeletionVector {
    /// Standard properties and optional application-specific identity metadata.
    pub properties: BTreeMap<String, String>,
    /// Deleted row ordinals, kept compressed until consumed.
    pub positions: RoaringTreemap,
}

#[derive(Serialize, Deserialize)]
struct FileMetadata {
    blobs: Vec<BlobMetadata>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    properties: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct BlobMetadata {
    #[serde(rename = "type")]
    blob_type: String,
    fields: Vec<i32>,
    snapshot_id: i64,
    sequence_number: i64,
    offset: i64,
    length: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    compression_codec: Option<String>,
    #[serde(default)]
    properties: BTreeMap<String, String>,
}

fn invalid(message: &str) -> BundlebaseError {
    format!("Invalid Puffin deletion vector: {message}").into()
}

fn referenced_file(properties: &BTreeMap<String, String>) -> Result<(), BundlebaseError> {
    if properties
        .get("referenced-data-file")
        .is_none_or(String::is_empty)
    {
        return Err(invalid("missing or empty referenced-data-file property"));
    }
    Ok(())
}

/// Encode vectors in deterministic property/payload order, deriving cardinality.
pub fn serialize(vectors: &[DeletionVector]) -> Result<Vec<u8>, BundlebaseError> {
    let mut encoded = Vec::with_capacity(vectors.len());
    for vector in vectors {
        referenced_file(&vector.properties)?;
        if vector.positions.max().is_some_and(|p| p > i64::MAX as u64) {
            return Err(invalid(
                "positions must fit positive signed 64-bit integers",
            ));
        }
        let mut positions = vector.positions.clone();
        positions.optimize();
        let mut properties = vector.properties.clone();
        properties.insert("cardinality".into(), vector.positions.len().to_string());
        let data_len = positions
            .serialized_size()
            .checked_add(4)
            .and_then(|len| i32::try_from(len).ok())
            .ok_or_else(|| invalid("deletion vector exceeds signed 32-bit length"))?;
        let mut blob = Vec::new();
        blob.extend_from_slice(&data_len.to_be_bytes());
        blob.extend_from_slice(DV_MAGIC);
        // roaring 0.11's treemap encoding is the portable LE u64 count,
        // sorted LE u32 keys, and portable 32-bit bitmaps, not Java's legacy format.
        positions.serialize_into(&mut blob)?;
        let checksum = crc32fast::hash(&blob[4..]);
        blob.extend_from_slice(&checksum.to_be_bytes());
        encoded.push((properties, blob));
    }
    encoded.sort();
    let mut bytes = MAGIC.to_vec();
    let mut metadata = FileMetadata {
        blobs: Vec::new(),
        properties: BTreeMap::new(),
    };
    for (properties, blob) in encoded {
        metadata.blobs.push(BlobMetadata {
            blob_type: DV_TYPE.into(),
            fields: Vec::new(),
            snapshot_id: -1,
            sequence_number: -1,
            offset: i64::try_from(bytes.len())?,
            length: i64::try_from(blob.len())?,
            compression_codec: None,
            properties,
        });
        bytes.extend_from_slice(&blob);
    }
    let footer = serde_json::to_vec(&metadata)?;
    let footer_len = i32::try_from(footer.len()).map_err(|_| invalid("footer too large"))?;
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&footer);
    bytes.extend_from_slice(&footer_len.to_le_bytes());
    bytes.extend_from_slice(&[0; 4]);
    bytes.extend_from_slice(MAGIC);
    Ok(bytes)
}

/// Decode and validate every blob before returning any of them.
///
/// All allocation sizes are bounded by the supplied bytes; bitmap positions
/// remain compressed, including positions outside the core's u32 range.
pub fn deserialize(bytes: &[u8]) -> Result<Vec<DeletionVector>, BundlebaseError> {
    if bytes.len() < 20 || bytes.get(..4) != Some(MAGIC) {
        return Err("Unsupported DELETE format: expected Puffin PFA1".into());
    }
    let trailer = bytes.len() - 12;
    if &bytes[trailer + 8..] != MAGIC {
        return Err(invalid("bad trailing magic"));
    }
    if bytes[trailer + 4..trailer + 8] != [0; 4] {
        return Err(invalid("unsupported footer compression or flags"));
    }
    let footer_len = i32::from_le_bytes(bytes[trailer..trailer + 4].try_into()?);
    let footer_len = usize::try_from(footer_len).map_err(|_| invalid("negative footer length"))?;
    let footer_start = trailer
        .checked_sub(footer_len)
        .and_then(|n| n.checked_sub(4))
        .filter(|n| *n >= 4)
        .ok_or_else(|| invalid("footer length crosses file boundary"))?;
    if &bytes[footer_start..footer_start + 4] != MAGIC {
        return Err(invalid("bad footer magic"));
    }
    let metadata: FileMetadata = serde_json::from_slice(&bytes[footer_start + 4..trailer])
        .map_err(|e| invalid(&format!("invalid footer metadata: {e}")))?;
    let mut ranges = Vec::with_capacity(metadata.blobs.len());
    for blob in &metadata.blobs {
        if blob.blob_type != DV_TYPE {
            return Err(invalid("unsupported blob type"));
        }
        if blob.compression_codec.is_some() {
            return Err(invalid("unsupported blob compression"));
        }
        if blob.snapshot_id != -1 || blob.sequence_number != -1 {
            return Err(invalid(
                "deletion-vector snapshot-id and sequence-number must be -1",
            ));
        }
        referenced_file(&blob.properties)?;
        let start = usize::try_from(blob.offset).map_err(|_| invalid("invalid blob offset"))?;
        let length = usize::try_from(blob.length).map_err(|_| invalid("invalid blob length"))?;
        let end = start
            .checked_add(length)
            .filter(|end| *end <= footer_start)
            .ok_or_else(|| invalid("blob crosses footer or overflows"))?;
        if start < 4 || length < 20 {
            return Err(invalid("invalid blob bounds"));
        }
        ranges.push((start, end));
    }
    let mut sorted_ranges = ranges.clone();
    sorted_ranges.sort_unstable();
    if sorted_ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err(invalid("overlapping blobs"));
    }
    let mut vectors = Vec::with_capacity(metadata.blobs.len());
    for (blob, (start, end)) in metadata.blobs.into_iter().zip(ranges) {
        let positions = decode_blob(&bytes[start..end])?;
        let cardinality = blob
            .properties
            .get("cardinality")
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .ok_or_else(|| invalid("missing or invalid cardinality"))?
            .parse::<u64>()
            .map_err(|_| invalid("cardinality overflow"))?;
        if positions.len() != cardinality {
            return Err(invalid("cardinality mismatch"));
        }
        vectors.push(DeletionVector {
            properties: blob.properties,
            positions,
        });
    }
    Ok(vectors)
}

fn decode_blob(blob: &[u8]) -> Result<RoaringTreemap, BundlebaseError> {
    let length = i32::from_be_bytes(blob[..4].try_into()?);
    if usize::try_from(length).ok().and_then(|n| n.checked_add(8)) != Some(blob.len()) {
        return Err(invalid("invalid deletion-vector length"));
    }
    if &blob[4..8] != DV_MAGIC {
        return Err(invalid("bad deletion-vector magic"));
    }
    let checksum_start = blob.len() - 4;
    let checksum = u32::from_be_bytes(blob[checksum_start..].try_into()?);
    if crc32fast::hash(&blob[4..checksum_start]) != checksum {
        return Err(invalid("CRC-32 checksum mismatch"));
    }
    let vector = &blob[8..checksum_start];
    validate_vector(vector)?;
    RoaringTreemap::deserialize_from(vector).map_err(|e| invalid(&format!("invalid bitmap: {e}")))
}

// Preflight the portable format before the library allocates from its counts.
// The library validates container contents, but ignores serialized offsets and
// does not validate a run container's declared cardinality, so check those here.
fn validate_vector(bytes: &[u8]) -> Result<(), BundlebaseError> {
    let mut reader = Reader { bytes, position: 0 };
    let count = u64::from_le_bytes(reader.take(8)?.try_into()?);
    if count > (reader.remaining() / 12) as u64 {
        return Err(invalid("bitmap count exceeds payload"));
    }
    let mut last_key = None;
    for _ in 0..count {
        let key = reader.u32()?;
        if key > i32::MAX as u32 || last_key.is_some_and(|last| last >= key) {
            return Err(invalid("bitmap keys must be sorted positive 32-bit keys"));
        }
        last_key = Some(key);
        validate_bitmap(&mut reader)?;
    }
    if reader.remaining() != 0 {
        return Err(invalid("trailing bitmap bytes"));
    }
    Ok(())
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}
impl<'a> Reader<'a> {
    fn remaining(&self) -> usize {
        self.bytes.len() - self.position
    }
    fn take(&mut self, length: usize) -> Result<&'a [u8], BundlebaseError> {
        let end = self
            .position
            .checked_add(length)
            .filter(|n| *n <= self.bytes.len())
            .ok_or_else(|| invalid("truncated bitmap"))?;
        let result = &self.bytes[self.position..end];
        self.position = end;
        Ok(result)
    }
    fn u16(&mut self) -> Result<u16, BundlebaseError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into()?))
    }
    fn u32(&mut self) -> Result<u32, BundlebaseError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
}

fn validate_bitmap(reader: &mut Reader<'_>) -> Result<(), BundlebaseError> {
    let start = reader.position;
    let cookie = reader.u32()?;
    let (count, has_runs) = if cookie == 12346 {
        (usize::try_from(reader.u32()?)?, false)
    } else if cookie & 0xffff == 12347 {
        (((cookie >> 16) + 1) as usize, true)
    } else {
        return Err(invalid("invalid portable bitmap cookie"));
    };
    if count > 65536 || count > reader.remaining() / 4 {
        return Err(invalid("container count exceeds payload"));
    }
    let runs = if has_runs {
        reader.take(count.div_ceil(8))?
    } else {
        &[]
    };
    let descriptions = reader.take(count * 4)?;
    let offsets = if !has_runs || count >= 4 {
        reader.take(count * 4)?
    } else {
        &[]
    };
    let mut last_key = None;
    for i in 0..count {
        let key = u16::from_le_bytes(descriptions[i * 4..i * 4 + 2].try_into()?);
        if last_key.is_some_and(|last| last >= key) {
            return Err(invalid("container keys must be sorted"));
        }
        last_key = Some(key);
        let cardinality = u32::from(u16::from_le_bytes(
            descriptions[i * 4 + 2..i * 4 + 4].try_into()?,
        )) + 1;
        if !offsets.is_empty()
            && u32::from_le_bytes(offsets[i * 4..i * 4 + 4].try_into()?) as usize
                != reader.position - start
        {
            return Err(invalid("invalid container offset"));
        }
        if has_runs && runs[i / 8] & (1 << (i % 8)) != 0 {
            let run_count = usize::from(reader.u16()?);
            let intervals = reader.take(run_count * 4)?;
            let mut actual = 0u32;
            for interval in intervals.as_chunks::<4>().0 {
                actual += u32::from(u16::from_le_bytes(interval[2..].try_into()?)) + 1;
            }
            if actual != cardinality {
                return Err(invalid("run cardinality mismatch"));
            }
        } else {
            reader.take(if cardinality <= 4096 {
                cardinality as usize * 2
            } else {
                8192
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vector(name: &str, positions: impl IntoIterator<Item = u64>) -> DeletionVector {
        let mut bitmap = RoaringTreemap::new();
        bitmap.extend(positions);
        DeletionVector {
            properties: BTreeMap::from([("referenced-data-file".into(), name.into())]),
            positions: bitmap,
        }
    }

    fn footer(bytes: &[u8]) -> serde_json::Value {
        let trailer = bytes.len() - 12;
        let length =
            i32::from_le_bytes(bytes[trailer..trailer + 4].try_into().expect("length")) as usize;
        serde_json::from_slice(&bytes[trailer - length..trailer]).expect("footer")
    }

    fn change_footer(bytes: &[u8], change: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
        let trailer = bytes.len() - 12;
        let length =
            i32::from_le_bytes(bytes[trailer..trailer + 4].try_into().expect("length")) as usize;
        let mut metadata = footer(bytes);
        change(&mut metadata);
        let payload = serde_json::to_vec(&metadata).expect("json");
        let mut output = bytes[..trailer - length].to_vec();
        output.extend_from_slice(&payload);
        output.extend_from_slice(&(payload.len() as i32).to_le_bytes());
        output.extend_from_slice(&[0; 4]);
        output.extend_from_slice(MAGIC);
        output
    }

    fn repair_first_checksum(bytes: &mut [u8]) {
        let length = i32::from_be_bytes(bytes[4..8].try_into().expect("length")) as usize;
        let checksum = crc32fast::hash(&bytes[8..8 + length]);
        bytes[8 + length..12 + length].copy_from_slice(&checksum.to_be_bytes());
    }

    #[test]
    fn deterministic_multi_blob_round_trip() {
        let sparse = vector(
            "data/sparse.csv",
            [0, 7, u32::MAX as u64, (1u64 << 32) + 5, i64::MAX as u64],
        );
        let dense = vector("https://example.org/dense.jsonl", (0..20000).step_by(2));
        let runs = vector("data/runs.parquet", 100..10000);
        let bytes = serialize(&[sparse.clone(), dense.clone(), runs.clone()]).expect("serialize");
        assert_eq!(
            bytes,
            serialize(&[runs, dense, sparse]).expect("serialize reordered")
        );
        let decoded = deserialize(&bytes).expect("deserialize");
        assert_eq!(decoded.len(), 3);
        assert!(decoded
            .iter()
            .any(|v| v.positions.contains(i64::MAX as u64)));
        assert_eq!(serialize(&decoded).expect("serialize decoded"), bytes);
        let metadata = footer(&bytes);
        for blob in metadata["blobs"].as_array().expect("blobs") {
            assert_eq!(blob["fields"], serde_json::json!([]));
            assert_eq!(blob["snapshot-id"], -1);
            assert_eq!(blob["sequence-number"], -1);
            assert!(blob.get("compression-codec").is_none());
        }
    }

    #[test]
    fn independent_apache_iceberg_fixture() {
        let bytes = include_bytes!("../tests/fixtures/puffin/iceberg-1.10.0-all-containers.puffin");
        let decoded = deserialize(bytes).expect("Apache Iceberg 1.10.0 DV");
        let mut expected = RoaringTreemap::new();
        expected.extend([5, 7]);
        expected.insert_range(65536 + 1..65536 + 1000);
        expected.insert_range(2 * 65536 + 1..3 * 65536 - 1);
        let high = 1u64 << 32;
        expected.extend([high + 10, high + 20]);
        expected.insert_range(high + 65536 + 10..high + 65536 + 500);
        expected.insert_range(high + 2 * 65536 + 1..high + 3 * 65536 - 1);
        assert_eq!(decoded[0].positions, expected);
        assert_eq!(decoded[0].positions.len(), 132561);
        // Check our writer against the independent producer too, including
        // its portable framing, run encoding, byte order and CRC-32.
        let rewritten = serialize(&decoded).expect("write independent positions");
        assert_eq!(&rewritten[4..98], &bytes[4..98]);
        // The independent producer's exact DV is retained inside the wrapper.
        assert_eq!(
            &bytes[4..98],
            include_bytes!("../tests/fixtures/puffin/iceberg-1.10.0-all-containers.dv")
        );
    }

    #[test]
    fn empty_and_position_limits() {
        assert!(deserialize(&serialize(&[]).expect("empty"))
            .expect("decode")
            .is_empty());
        let empty = vector("empty.csv", []);
        assert!(
            deserialize(&serialize(&[empty]).expect("empty vector")).expect("decode")[0]
                .positions
                .is_empty()
        );
        assert!(serialize(&[vector("x", [1u64 << 63])]).is_err());
        assert!(serialize(&[vector("", [0])]).is_err());
    }

    #[test]
    fn fields_are_required_but_may_reference_iceberg_position_field() {
        let bytes = serialize(&[vector("x", [1])]).expect("serialize");
        // The DV specification does not require fields to be empty; Java
        // Iceberg writers can identify its reserved row-position field here.
        let external = change_footer(&bytes, |m| {
            m["blobs"][0]["fields"] = serde_json::json!([2147483645]);
        });
        assert_eq!(
            deserialize(&external).expect("external fields")[0]
                .positions
                .len(),
            1
        );
        let missing = change_footer(&bytes, |m| {
            m["blobs"][0]
                .as_object_mut()
                .expect("blob")
                .remove("fields");
        });
        assert!(deserialize(&missing).is_err());
    }

    #[test]
    fn reject_truncation_and_magic() {
        let bytes = serialize(&[vector("x", [1, 3, 5])]).expect("serialize");
        for end in 0..bytes.len() {
            assert!(deserialize(&bytes[..end]).is_err(), "truncation at {end}");
        }
        for index in [0, bytes.len() - 1, bytes.len() - 13] {
            let mut bad = bytes.clone();
            bad[index] ^= 0xff;
            assert!(deserialize(&bad).is_err(), "magic at {index}");
        }
        assert!(deserialize(b"BBTOMB02legacy tombstone")
            .expect_err("legacy")
            .to_string()
            .contains("Unsupported DELETE format"));
    }

    #[test]
    fn reject_bad_footer_lengths_and_flags() {
        let bytes = serialize(&[vector("x", [1])]).expect("serialize");
        for length in [-1i32, i32::MAX] {
            let mut bad = bytes.clone();
            let start = bad.len() - 12;
            bad[start..start + 4].copy_from_slice(&length.to_le_bytes());
            assert!(deserialize(&bad).is_err());
        }
        let mut compressed = bytes;
        let flag = compressed.len() - 8;
        compressed[flag] = 1;
        assert!(deserialize(&compressed).is_err());
    }

    #[test]
    fn reject_metadata_corruption_before_returning_any_vector() {
        let bytes = serialize(&[vector("a", [1]), vector("b", [2])]).expect("serialize");
        for (field, value) in [
            ("offset", serde_json::json!(-1)),
            ("offset", serde_json::json!(i64::MAX)),
            ("length", serde_json::json!(-1)),
            ("length", serde_json::json!(i64::MAX)),
            ("type", serde_json::json!("unknown")),
            ("compression-codec", serde_json::json!("zstd")),
            ("snapshot-id", serde_json::json!(0)),
        ] {
            let bad = change_footer(&bytes, |m| m["blobs"][1][field] = value);
            assert!(deserialize(&bad).is_err(), "field {field}");
        }
        for property in ["cardinality", "referenced-data-file"] {
            let bad = change_footer(&bytes, |m| {
                m["blobs"][1]["properties"]
                    .as_object_mut()
                    .expect("props")
                    .remove(property);
            });
            assert!(deserialize(&bad).is_err());
        }
        let bad = change_footer(&bytes, |m| {
            m["blobs"][1]["properties"]["cardinality"] = "999".into()
        });
        assert!(deserialize(&bad).is_err());
        let overlap = change_footer(&bytes, |m| {
            m["blobs"][1]["offset"] = m["blobs"][0]["offset"].clone()
        });
        assert!(deserialize(&overlap).is_err());
        let crossing = change_footer(&bytes, |m| {
            m["blobs"][1]["length"] = serde_json::json!(bytes.len())
        });
        assert!(deserialize(&crossing).is_err());
    }

    #[test]
    fn reject_blob_checksum_length_and_bitmap_structure() {
        let bytes = serialize(&[vector("x", [1, 3, 5])]).expect("serialize");
        let mut bad_crc = bytes.clone();
        bad_crc[12] ^= 1;
        assert!(deserialize(&bad_crc)
            .expect_err("CRC")
            .to_string()
            .contains("CRC-32"));
        for value in [-1i32, i32::MAX] {
            let mut bad = bytes.clone();
            bad[4..8].copy_from_slice(&value.to_be_bytes());
            assert!(deserialize(&bad).is_err());
        }
        // DV bitmap count and key, followed by a no-run container header.
        for (start, data) in [
            (12, u64::MAX.to_le_bytes().to_vec()),
            (20, 0x80000000u32.to_le_bytes().to_vec()),
            (24, 0u32.to_le_bytes().to_vec()),
            (36, 0u32.to_le_bytes().to_vec()),
        ] {
            let mut bad = bytes.clone();
            bad[start..start + data.len()].copy_from_slice(&data);
            repair_first_checksum(&mut bad);
            assert!(deserialize(&bad).is_err(), "bitmap corruption at {start}");
        }
        let fixture =
            include_bytes!("../tests/fixtures/puffin/iceberg-1.10.0-all-containers.puffin");
        let mut bad_run = fixture.to_vec();
        // First run container's declared cardinality in the portable header.
        bad_run[35] ^= 1;
        repair_first_checksum(&mut bad_run);
        assert!(deserialize(&bad_run).is_err());
    }
}
