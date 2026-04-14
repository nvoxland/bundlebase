//! ObjectId module for unique object identification.

use rand::Rng;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A globally unique 64-bit identifier for objects (blocks, packs, indexes, views, etc.).
///
/// Stored as `[u8; 8]` and serialized as 16 lowercase hex characters.
/// Values 0–999 are reserved for well-known constants; `generate()` always produces values ≥ 1000.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectId([u8; 8]);

impl ObjectId {
    /// Well-known ID for the base pack, always created when a bundle is created.
    pub const BASE_PACK: ObjectId = ObjectId([0, 0, 0, 0, 0, 0, 0, 1]);

    /// Generate a new unique ObjectId with a random 64-bit value ≥ 1000.
    pub fn generate() -> Self {
        let mut rng = rand::rng();
        loop {
            let bytes: [u8; 8] = rng.random();
            let val = u64::from_be_bytes(bytes);
            if val >= 1000 {
                return ObjectId(bytes);
            }
        }
    }
}

impl From<ObjectId> for String {
    fn from(s: ObjectId) -> String {
        hex::encode(s.0)
    }
}

impl TryFrom<String> for ObjectId {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::try_from(s.as_str())
    }
}

impl TryFrom<&str> for ObjectId {
    type Error = String;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        let bytes = hex::decode(s).map_err(|e| format!("Invalid hex string: {}", e))?;
        if bytes.len() != 8 {
            return Err(format!(
                "Invalid hex string: expected 16 hex chars (8 bytes), got {} chars",
                s.len()
            ));
        }
        let mut arr = [0u8; 8];
        arr.copy_from_slice(&bytes);
        Ok(ObjectId(arr))
    }
}

impl Serialize for ObjectId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let hex_string: String = (*self).into();
        serializer.serialize_str(&hex_string)
    }
}

impl<'de> Deserialize<'de> for ObjectId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        ObjectId::try_from(s).map_err(serde::de::Error::custom)
    }
}

impl std::fmt::Display for ObjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hex_string: String = (*self).into();
        write!(f, "{}", hex_string)
    }
}

/// A type-safe wrapper around ObjectId specifically for block identifiers.
///
/// Provides compile-time distinction between block IDs and other object IDs
/// (packs, indexes, views, sources, joins).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockId(ObjectId);

impl BlockId {
    /// Generate a new unique BlockId.
    pub fn generate() -> Self {
        Self(ObjectId::generate())
    }

    /// Access the inner ObjectId.
    pub fn as_object_id(&self) -> &ObjectId {
        &self.0
    }
}

impl From<BlockId> for ObjectId {
    fn from(id: BlockId) -> ObjectId {
        id.0
    }
}

impl From<ObjectId> for BlockId {
    fn from(id: ObjectId) -> BlockId {
        BlockId(id)
    }
}

impl From<BlockId> for String {
    fn from(id: BlockId) -> String {
        id.0.into()
    }
}

impl TryFrom<String> for BlockId {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        ObjectId::try_from(s).map(BlockId)
    }
}

impl TryFrom<&str> for BlockId {
    type Error = String;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        ObjectId::try_from(s).map(BlockId)
    }
}

impl Serialize for BlockId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for BlockId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        ObjectId::deserialize(deserializer).map(BlockId)
    }
}

impl std::fmt::Display for BlockId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// A type-safe wrapper around ObjectId specifically for column identifiers.
///
/// Provides compile-time distinction between column IDs and other object IDs
/// (blocks, packs, indexes, views, sources, joins).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ColumnId(ObjectId);

impl ColumnId {
    /// Generate a new unique ColumnId.
    pub fn generate() -> Self {
        Self(ObjectId::generate())
    }

    /// Access the inner ObjectId.
    pub fn as_object_id(&self) -> &ObjectId {
        &self.0
    }
}

impl From<ColumnId> for ObjectId {
    fn from(id: ColumnId) -> ObjectId {
        id.0
    }
}

impl From<ObjectId> for ColumnId {
    fn from(id: ObjectId) -> ColumnId {
        ColumnId(id)
    }
}

impl From<ColumnId> for String {
    fn from(id: ColumnId) -> String {
        id.0.into()
    }
}

impl TryFrom<String> for ColumnId {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        ObjectId::try_from(s).map(ColumnId)
    }
}

impl TryFrom<&str> for ColumnId {
    type Error = String;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        ObjectId::try_from(s).map(ColumnId)
    }
}

impl Serialize for ColumnId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ColumnId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        ObjectId::deserialize(deserializer).map(ColumnId)
    }
}

impl std::fmt::Display for ColumnId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// A compact u16 reference to an ObjectId, used for bit-packing in RowId.
///
/// ObjectIdAlias values are only meaningful within the context that defines them
/// (e.g., a specific IndexBlocksOp). They are NOT global or per-bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObjectIdAlias(u16);

impl ObjectIdAlias {
    pub fn as_u16(self) -> u16 {
        self.0
    }
}

impl From<u16> for ObjectIdAlias {
    fn from(v: u16) -> Self {
        Self(v)
    }
}

impl std::fmt::Display for ObjectIdAlias {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04x}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate() {
        let id1 = ObjectId::generate();
        let id2 = ObjectId::generate();
        let id3 = ObjectId::generate();

        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_generate_above_reserved() {
        for _ in 0..100 {
            let id = ObjectId::generate();
            let val = u64::from_be_bytes(id.0);
            assert!(
                val >= 1000,
                "Generated ObjectId value {} is below 1000",
                val
            );
        }
    }

    #[test]
    fn test_base_pack() {
        let hex_string: String = ObjectId::BASE_PACK.into();
        assert_eq!(hex_string, "0000000000000001");
    }

    #[test]
    fn test_serialize_hex() {
        let block_id = ObjectId([0, 0, 0, 0, 0, 0, 0, 255]);
        let json = serde_json::to_string(&block_id).unwrap();
        assert_eq!(json, "\"00000000000000ff\"");
    }

    #[test]
    fn test_deserialize_hex() {
        let json = "\"00000000000000ff\"";
        let block_id: ObjectId = serde_json::from_str(json).unwrap();
        assert_eq!(block_id, ObjectId([0, 0, 0, 0, 0, 0, 0, 255]));
    }

    #[test]
    fn test_roundtrip_serialization() {
        let original = ObjectId::generate();
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: ObjectId = serde_json::from_str(&json).unwrap();
        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_from_object_id_to_string() {
        let block_id = ObjectId([0, 0, 0, 0, 0, 0, 0, 255]);
        let hex_string: String = block_id.into();
        assert_eq!(hex_string, "00000000000000ff");
    }

    #[test]
    fn test_try_from_string_for_object_id() {
        let hex_string = "00000000000000ff".to_string();
        let block_id: ObjectId = hex_string.try_into().unwrap();
        assert_eq!(block_id, ObjectId([0, 0, 0, 0, 0, 0, 0, 255]));
    }

    #[test]
    fn test_try_from_str_for_object_id() {
        let block_id: ObjectId = "00000000000000a5".try_into().unwrap();
        assert_eq!(block_id, ObjectId([0, 0, 0, 0, 0, 0, 0, 165]));
    }

    #[test]
    fn test_try_from_invalid_hex() {
        let result: Result<ObjectId, _> = "zzzzzzzzzzzzzzzz".try_into();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid hex string"));
    }

    #[test]
    fn test_try_from_wrong_length() {
        let result: Result<ObjectId, _> = "00ff".try_into();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected 16 hex chars"));
    }

    #[test]
    fn test_roundtrip_via_string() {
        let original = ObjectId::generate();
        let hex_string: String = original.into();
        let recovered: ObjectId = hex_string.try_into().unwrap();
        assert_eq!(original, recovered);
    }

    #[test]
    fn test_display_format() {
        let hex_string = format!("{}", ObjectId::BASE_PACK);
        assert_eq!(hex_string, "0000000000000001");
    }

    #[test]
    fn test_object_id_ref() {
        let r = ObjectIdAlias::from(42u16);
        assert_eq!(r.as_u16(), 42);
        assert_eq!(format!("{}", r), "002a");
    }

    #[test]
    fn test_object_id_ref_serde() {
        let r = ObjectIdAlias::from(255u16);
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, "255");
        let deserialized: ObjectIdAlias = serde_json::from_str(&json).unwrap();
        assert_eq!(r, deserialized);
    }
}
