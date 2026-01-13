//! ObjectId module for unique object identification.

use parking_lot::RwLock;
use rand::Rng;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashSet;
use std::sync::OnceLock;

static KNOWN_IDS: OnceLock<RwLock<HashSet<u8>>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectId(u8);

impl ObjectId {
    /// Well-known ID for the base pack, always created when a bundle is created.
    pub const BASE_PACK: ObjectId = ObjectId(0);

    pub fn generate() -> ObjectId {
        let mut rng = rand::rng();
        let mut id = rng.random::<u8>();

        // Keep generating random IDs until we find one that's not in the known_ids set
        let known_ids = KNOWN_IDS.get();
        while known_ids.is_some_and(|x| x.read().contains(&id)) {
            id = rng.random::<u8>();
        }

        let return_id = ObjectId(id);
        Self::add_known(return_id);

        return_id
    }

    fn add_known(id: ObjectId) -> ObjectId {
        let ids = KNOWN_IDS.get_or_init(|| RwLock::new(HashSet::new()));

        if !ids.read().contains(&id.0) {
            ids.write().insert(id.0);
        }
        id
    }

    pub fn as_u8(self) -> u8 {
        self.0
    }

    pub fn saturating_add(self, rhs: u8) -> ObjectId {
        ObjectId(self.0.saturating_add(rhs))
    }
}

impl From<u8> for ObjectId {
    fn from(v: u8) -> Self {
        Self(v)
    }
}

impl From<ObjectId> for u8 {
    fn from(s: ObjectId) -> u8 {
        s.0
    }
}

impl From<ObjectId> for String {
    fn from(s: ObjectId) -> String {
        format!("{:02x}", s.0)
    }
}

impl TryFrom<String> for ObjectId {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        u8::from_str_radix(&s, 16)
            .map(ObjectId)
            .map_err(|e| format!("Invalid hex string: {}", e))
    }
}

impl TryFrom<&str> for ObjectId {
    type Error = String;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        u8::from_str_radix(s, 16)
            .map(ObjectId)
            .map_err(|e| format!("Invalid hex string: {}", e))
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
        match ObjectId::try_from(s) {
            Ok(id) => Ok(ObjectId::add_known(id)),
            Err(e) => Err(serde::de::Error::custom(e)),
        }
    }
}

impl std::fmt::Display for ObjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hex_string: String = (*self).into();
        write!(f, "{}", hex_string)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate() {
        let block_id1 = ObjectId::generate();
        let block_id2 = ObjectId::generate();
        let block_id3 = ObjectId::generate();

        assert_ne!(block_id1, block_id2);
        assert_ne!(block_id2, block_id3);
        assert_ne!(block_id1, block_id3);
    }

    #[test]
    fn test_serialize_hex() {
        let block_id = ObjectId(255);
        let json = serde_json::to_string(&block_id).unwrap();
        assert_eq!(json, "\"ff\"");
    }

    #[test]
    fn test_deserialize_hex() {
        let json = "\"ff\"";
        let block_id: ObjectId = serde_json::from_str(json).unwrap();
        assert_eq!(block_id, ObjectId(255));
    }

    #[test]
    fn test_roundtrip_serialization() {
        let original = ObjectId(200);
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: ObjectId = serde_json::from_str(&json).unwrap();
        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_from_block_id_to_string() {
        let block_id = ObjectId(255);
        let hex_string: String = block_id.into();
        assert_eq!(hex_string, "ff");
    }

    #[test]
    fn test_try_from_string_for_block_id() {
        let hex_string = "ff".to_string();
        let block_id: ObjectId = hex_string.try_into().unwrap();
        assert_eq!(block_id, ObjectId(255));
    }

    #[test]
    fn test_try_from_str_for_block_id() {
        let block_id: ObjectId = "a5".try_into().unwrap();
        assert_eq!(block_id, ObjectId(165));
    }

    #[test]
    fn test_try_from_invalid_hex() {
        let result: Result<ObjectId, _> = "zzzz".try_into();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid hex string"));
    }

    #[test]
    fn test_roundtrip_via_string() {
        let original = ObjectId(42);
        let hex_string: String = original.into();
        let recovered: ObjectId = hex_string.try_into().unwrap();
        assert_eq!(original, recovered);
    }
}
