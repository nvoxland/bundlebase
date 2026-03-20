//! Arrow type parsing and serialization utilities.
//!
//! Converts between Arrow `DataType` and string representations used in
//! function signatures and YAML configuration files.

use crate::BundlebaseError;
use arrow::datatypes::{DataType, Field, Fields};
use std::sync::Arc;

/// Custom serde for Arrow `DataType` ↔ string (e.g., `"Int64"` in YAML).
pub mod arrow_type_serde {
    use super::{arrow_type_to_name, parse_arrow_type_name, DataType};
    use serde::{self, Deserialize, Deserializer, Serializer};

    /// Serde helpers for a single `DataType` field.
    pub mod single {
        use super::*;

        pub fn serialize<S>(dt: &DataType, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serializer.serialize_str(&arrow_type_to_name(dt))
        }

        pub fn deserialize<'de, D>(deserializer: D) -> Result<DataType, D::Error>
        where
            D: Deserializer<'de>,
        {
            let s = String::deserialize(deserializer)?;
            parse_arrow_type_name(&s).map_err(serde::de::Error::custom)
        }
    }

    /// Serde helpers for a `Vec<DataType>` field.
    pub mod vec {
        use super::*;
        use serde::ser::SerializeSeq;

        pub fn serialize<S>(types: &[DataType], serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let mut seq = serializer.serialize_seq(Some(types.len()))?;
            for dt in types {
                seq.serialize_element(&arrow_type_to_name(dt))?;
            }
            seq.end()
        }

        pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<DataType>, D::Error>
        where
            D: Deserializer<'de>,
        {
            let strings: Vec<String> = Vec::<String>::deserialize(deserializer)?;
            strings
                .iter()
                .map(|s| parse_arrow_type_name(s).map_err(serde::de::Error::custom))
                .collect()
        }
    }
}

/// Parse an Arrow type name string into a DataFusion `DataType`.
///
/// Supports common Arrow type names used in function signatures,
/// including complex/nested types.
///
/// # Examples
/// - `"Int64"` → `DataType::Int64`
/// - `"Utf8"` → `DataType::Utf8`
/// - `"List<Int64>"` → `DataType::List(Field::new("item", DataType::Int64, true))`
/// - `"Struct<x:Int64,y:Float64>"` → `DataType::Struct(...)`
/// - `"Map<Utf8,Int64>"` → `DataType::Map(...)`
/// - `"Decimal128(38,10)"` → `DataType::Decimal128(38, 10)`
pub fn parse_arrow_type_name(type_name: &str) -> Result<DataType, BundlebaseError> {
    let trimmed = type_name.trim();

    // Check for parameterized types first
    if let Some(inner) = trimmed.strip_prefix("List<").and_then(|s| s.strip_suffix('>')) {
        let element_type = parse_arrow_type_name(inner)?;
        return Ok(DataType::List(Arc::new(Field::new(
            "item",
            element_type,
            true,
        ))));
    }

    if let Some(inner) = trimmed
        .strip_prefix("Struct<")
        .and_then(|s| s.strip_suffix('>'))
    {
        let fields = parse_struct_fields(inner)?;
        return Ok(DataType::Struct(fields));
    }

    if let Some(inner) = trimmed.strip_prefix("Map<").and_then(|s| s.strip_suffix('>')) {
        let (key_str, value_str) = split_top_level_comma(inner).ok_or_else(|| {
            BundlebaseError::from(format!(
                "Invalid Map type '{}'. Expected format: Map<KeyType,ValueType>",
                trimmed
            ))
        })?;
        let key_type = parse_arrow_type_name(key_str.trim())?;
        let value_type = parse_arrow_type_name(value_str.trim())?;
        let entries_field = Field::new(
            "entries",
            DataType::Struct(Fields::from(vec![
                Field::new("key", key_type, false),
                Field::new("value", value_type, true),
            ])),
            false,
        );
        return Ok(DataType::Map(Arc::new(entries_field), false));
    }

    if let Some(inner) = trimmed
        .strip_prefix("Decimal128(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let parts: Vec<&str> = inner.split(',').collect();
        if parts.len() != 2 {
            return Err(format!(
                "Invalid Decimal128 type '{}'. Expected format: Decimal128(precision,scale)",
                trimmed
            )
            .into());
        }
        let precision: u8 = parts[0].trim().parse().map_err(|_| {
            BundlebaseError::from(format!(
                "Invalid Decimal128 precision '{}'. Must be a number 1-38.",
                parts[0].trim()
            ))
        })?;
        let scale: i8 = parts[1].trim().parse().map_err(|_| {
            BundlebaseError::from(format!(
                "Invalid Decimal128 scale '{}'. Must be a number.",
                parts[1].trim()
            ))
        })?;
        return Ok(DataType::Decimal128(precision, scale));
    }

    // Simple types
    match trimmed {
        "Boolean" => Ok(DataType::Boolean),
        "Int8" => Ok(DataType::Int8),
        "Int16" => Ok(DataType::Int16),
        "Int32" => Ok(DataType::Int32),
        "Int64" => Ok(DataType::Int64),
        "UInt8" => Ok(DataType::UInt8),
        "UInt16" => Ok(DataType::UInt16),
        "UInt32" => Ok(DataType::UInt32),
        "UInt64" => Ok(DataType::UInt64),
        "Float16" => Ok(DataType::Float16),
        "Float32" => Ok(DataType::Float32),
        "Float64" => Ok(DataType::Float64),
        "Utf8" => Ok(DataType::Utf8),
        "LargeUtf8" => Ok(DataType::LargeUtf8),
        "Binary" => Ok(DataType::Binary),
        "LargeBinary" => Ok(DataType::LargeBinary),
        "Date32" => Ok(DataType::Date32),
        "Date64" => Ok(DataType::Date64),
        "Timestamp" => Ok(DataType::Timestamp(
            arrow::datatypes::TimeUnit::Microsecond,
            None,
        )),
        _ => Err(format!(
            "Unknown Arrow type name '{}'. Supported types: Boolean, Int8, Int16, Int32, Int64, \
             UInt8, UInt16, UInt32, UInt64, Float16, Float32, Float64, Utf8, LargeUtf8, \
             Binary, LargeBinary, Date32, Date64, Timestamp, List<T>, Struct<name:type,...>, \
             Map<K,V>, Decimal128(precision,scale)",
            trimmed
        )
        .into()),
    }
}

/// Split a string at the first top-level comma, respecting nested `<>` and `()`.
///
/// Returns `None` if no top-level comma is found.
fn split_top_level_comma(s: &str) -> Option<(&str, &str)> {
    let mut depth = 0usize;
    for (i, ch) in s.char_indices() {
        match ch {
            '<' | '(' => depth += 1,
            '>' | ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                return Some((&s[..i], &s[i + 1..]));
            }
            _ => {}
        }
    }
    None
}

/// Parse struct field definitions like `"x:Int64,y:Float64"` into Arrow Fields.
///
/// Supports nested types in field definitions (e.g., `"data:List<Int64>,name:Utf8"`).
fn parse_struct_fields(fields_str: &str) -> Result<Fields, BundlebaseError> {
    let mut fields = Vec::new();
    let mut remaining = fields_str;

    while !remaining.is_empty() {
        // Find the colon separating field name from type
        let colon_pos = remaining.find(':').ok_or_else(|| {
            BundlebaseError::from(format!(
                "Invalid struct field '{}'. Expected format: name:type",
                remaining
            ))
        })?;
        let field_name = remaining[..colon_pos].trim();
        let after_colon = &remaining[colon_pos + 1..];

        // Find the end of this field's type (next top-level comma or end of string)
        let (type_str, rest) = match split_top_level_comma(after_colon) {
            Some((type_part, remainder)) => (type_part.trim(), remainder.trim()),
            None => (after_colon.trim(), ""),
        };

        let field_type = parse_arrow_type_name(type_str)?;
        fields.push(Field::new(field_name, field_type, true));
        remaining = rest;
    }

    Ok(Fields::from(fields))
}

/// Convert an Arrow `DataType` to its canonical string representation.
///
/// This is the inverse of `parse_arrow_type_name` and ensures roundtrip fidelity
/// for both simple and complex types.
pub fn arrow_type_to_name(dt: &DataType) -> String {
    match dt {
        DataType::Boolean => "Boolean".to_string(),
        DataType::Int8 => "Int8".to_string(),
        DataType::Int16 => "Int16".to_string(),
        DataType::Int32 => "Int32".to_string(),
        DataType::Int64 => "Int64".to_string(),
        DataType::UInt8 => "UInt8".to_string(),
        DataType::UInt16 => "UInt16".to_string(),
        DataType::UInt32 => "UInt32".to_string(),
        DataType::UInt64 => "UInt64".to_string(),
        DataType::Float16 => "Float16".to_string(),
        DataType::Float32 => "Float32".to_string(),
        DataType::Float64 => "Float64".to_string(),
        DataType::Utf8 => "Utf8".to_string(),
        DataType::LargeUtf8 => "LargeUtf8".to_string(),
        DataType::Binary => "Binary".to_string(),
        DataType::LargeBinary => "LargeBinary".to_string(),
        DataType::Date32 => "Date32".to_string(),
        DataType::Date64 => "Date64".to_string(),
        DataType::Timestamp(_, _) => "Timestamp".to_string(),
        DataType::Decimal128(precision, scale) => {
            format!("Decimal128({},{})", precision, scale)
        }
        DataType::List(field) => {
            format!("List<{}>", arrow_type_to_name(field.data_type()))
        }
        DataType::Struct(fields) => {
            let field_strs: Vec<String> = fields
                .iter()
                .map(|f| format!("{}:{}", f.name(), arrow_type_to_name(f.data_type())))
                .collect();
            format!("Struct<{}>", field_strs.join(","))
        }
        DataType::Map(field, _) => {
            // Map entries field is a Struct with "key" and "value" fields
            if let DataType::Struct(entries) = field.data_type() {
                if entries.len() == 2 {
                    let key_type = arrow_type_to_name(entries[0].data_type());
                    let value_type = arrow_type_to_name(entries[1].data_type());
                    return format!("Map<{},{}>", key_type, value_type);
                }
            }
            // Fallback for non-standard map shapes
            format!("Map<{}>", arrow_type_to_name(field.data_type()))
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_arrow_type_common() {
        assert_eq!(parse_arrow_type_name("Int64").unwrap(), DataType::Int64);
        assert_eq!(parse_arrow_type_name("Utf8").unwrap(), DataType::Utf8);
        assert_eq!(parse_arrow_type_name("Float64").unwrap(), DataType::Float64);
        assert_eq!(parse_arrow_type_name("Boolean").unwrap(), DataType::Boolean);
    }

    #[test]
    fn test_parse_arrow_type_all_int_types() {
        assert_eq!(parse_arrow_type_name("Int8").unwrap(), DataType::Int8);
        assert_eq!(parse_arrow_type_name("Int16").unwrap(), DataType::Int16);
        assert_eq!(parse_arrow_type_name("Int32").unwrap(), DataType::Int32);
        assert_eq!(parse_arrow_type_name("UInt8").unwrap(), DataType::UInt8);
        assert_eq!(parse_arrow_type_name("UInt16").unwrap(), DataType::UInt16);
        assert_eq!(parse_arrow_type_name("UInt32").unwrap(), DataType::UInt32);
        assert_eq!(parse_arrow_type_name("UInt64").unwrap(), DataType::UInt64);
    }

    #[test]
    fn test_parse_arrow_type_invalid() {
        let result = parse_arrow_type_name("NotAType");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown Arrow type name"));
    }
}
