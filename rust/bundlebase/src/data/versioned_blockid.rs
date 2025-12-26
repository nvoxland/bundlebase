use crate::data::ObjectId;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Represents a block and its version, serialized as "{block}@{version}"
#[derive(Debug, Clone, PartialEq)]
pub struct VersionedBlockId {
    pub block: ObjectId,
    pub version: String,
}

impl VersionedBlockId {
    pub fn new(block: ObjectId, version: String) -> Self {
        Self { block, version }
    }
}

impl std::fmt::Display for VersionedBlockId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.block, self.version)
    }
}

impl Serialize for VersionedBlockId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("{}@{}", self.block, self.version))
    }
}

impl<'de> Deserialize<'de> for VersionedBlockId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let parts: Vec<&str> = s.splitn(2, '@').collect();

        if parts.len() != 2 {
            return Err(serde::de::Error::custom(format!(
                "Invalid BlockAndVersion format: expected 'block@version', got '{}'",
                s
            )));
        }

        let block = ObjectId::try_from(parts[0]).map_err(|e| {
            serde::de::Error::custom(format!("Invalid ObjectId '{}': {}", parts[0], e))
        })?;
        let version = parts[1].to_string();

        Ok(VersionedBlockId { block, version })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_versioned_block_id_display() {
        let vb = VersionedBlockId::new(ObjectId::from(42), "v1.0".to_string());
        assert_eq!(format!("{}", vb), "2a@v1.0"); // 42 in hex is 2a
    }

    #[test]
    fn test_versioned_block_id_serialization() {
        let vb = VersionedBlockId::new(ObjectId::from(255), "v2.5".to_string());

        let json = serde_json::to_string(&vb).unwrap();
        assert_eq!(json, "\"ff@v2.5\""); // 255 in hex is ff

        let deserialized: VersionedBlockId = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, vb);
    }

    #[test]
    fn test_versioned_block_id_deserialization_error() {
        // Missing @ separator
        let result: Result<VersionedBlockId, _> = serde_json::from_str("\"abcdef\"");
        assert!(result.is_err());

        // Invalid ObjectId
        let result: Result<VersionedBlockId, _> = serde_json::from_str("\"zzzz@v1\"");
        assert!(result.is_err());
    }
}
