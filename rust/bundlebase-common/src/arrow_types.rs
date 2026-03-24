//! Arrow type parsing utilities.
//!
//! Converts Arrow type name strings into `DataType` values for use in
//! function signatures and SQL commands.

use crate::BundlebaseError;
use arrow::datatypes::{DataType, Field, Fields};
use std::sync::Arc;

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
    let lower = trimmed.to_ascii_lowercase();

    // Check for parameterized types first (case-insensitive prefix matching)
    if lower.starts_with("list<") && trimmed.ends_with('>') {
        let inner = &trimmed[5..trimmed.len() - 1];
        let element_type = parse_arrow_type_name(inner)?;
        return Ok(DataType::List(Arc::new(Field::new(
            "item",
            element_type,
            true,
        ))));
    }

    if lower.starts_with("struct<") && trimmed.ends_with('>') {
        let inner = &trimmed[7..trimmed.len() - 1];
        let fields = parse_struct_fields(inner)?;
        return Ok(DataType::Struct(fields));
    }

    if lower.starts_with("map<") && trimmed.ends_with('>') {
        let inner = &trimmed[4..trimmed.len() - 1];
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

    if lower.starts_with("decimal128(") && trimmed.ends_with(')') {
        let inner = &trimmed[11..trimmed.len() - 1];
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

    // Simple types — case-insensitive canonical names + aliases
    match lower.as_str() {
        "boolean" | "bool" => Ok(DataType::Boolean),
        "int8" | "tinyint" | "byte" => Ok(DataType::Int8),
        "int16" | "short" | "smallint" => Ok(DataType::Int16),
        "int32" | "int" | "integer" => Ok(DataType::Int32),
        "int64" | "long" | "bigint" => Ok(DataType::Int64),
        "uint8" => Ok(DataType::UInt8),
        "uint16" => Ok(DataType::UInt16),
        "uint32" => Ok(DataType::UInt32),
        "uint64" => Ok(DataType::UInt64),
        "float16" => Ok(DataType::Float16),
        "float32" | "float" | "real" => Ok(DataType::Float32),
        "float64" | "double" => Ok(DataType::Float64),
        "utf8" | "string" | "text" | "varchar" => Ok(DataType::Utf8),
        "largeutf8" => Ok(DataType::LargeUtf8),
        "binary" | "bytes" | "blob" => Ok(DataType::Binary),
        "largebinary" => Ok(DataType::LargeBinary),
        "date32" | "date" => Ok(DataType::Date32),
        "date64" => Ok(DataType::Date64),
        "timestamp" => Ok(DataType::Timestamp(
            arrow::datatypes::TimeUnit::Microsecond,
            None,
        )),
        "decimal" => Ok(DataType::Decimal128(38, 10)),
        _ => Err(format!(
            "Unknown Arrow type name '{}'. Supported types: Boolean, Int8, Int16, Int32, Int64, \
             UInt8, UInt16, UInt32, UInt64, Float16, Float32, Float64, Utf8, LargeUtf8, \
             Binary, LargeBinary, Date32, Date64, Timestamp, List<T>, Struct<name:type,...>, \
             Map<K,V>, Decimal128(precision,scale). \
             Aliases: bool, string, text, varchar, int, integer, long, bigint, short, smallint, \
             tinyint, byte, float, real, double, date, bytes, blob, decimal",
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

    #[test]
    fn test_parse_case_insensitive_simple_types() {
        assert_eq!(parse_arrow_type_name("int64").unwrap(), DataType::Int64);
        assert_eq!(parse_arrow_type_name("INT64").unwrap(), DataType::Int64);
        assert_eq!(parse_arrow_type_name("Int64").unwrap(), DataType::Int64);
        assert_eq!(parse_arrow_type_name("utf8").unwrap(), DataType::Utf8);
        assert_eq!(parse_arrow_type_name("BOOLEAN").unwrap(), DataType::Boolean);
        assert_eq!(parse_arrow_type_name("float32").unwrap(), DataType::Float32);
        assert_eq!(parse_arrow_type_name("LARGEUTF8").unwrap(), DataType::LargeUtf8);
        assert_eq!(parse_arrow_type_name("timestamp").unwrap(), DataType::Timestamp(
            arrow::datatypes::TimeUnit::Microsecond, None,
        ));
    }

    #[test]
    fn test_parse_aliases() {
        assert_eq!(parse_arrow_type_name("bool").unwrap(), DataType::Boolean);
        assert_eq!(parse_arrow_type_name("string").unwrap(), DataType::Utf8);
        assert_eq!(parse_arrow_type_name("text").unwrap(), DataType::Utf8);
        assert_eq!(parse_arrow_type_name("varchar").unwrap(), DataType::Utf8);
        assert_eq!(parse_arrow_type_name("int").unwrap(), DataType::Int32);
        assert_eq!(parse_arrow_type_name("integer").unwrap(), DataType::Int32);
        assert_eq!(parse_arrow_type_name("long").unwrap(), DataType::Int64);
        assert_eq!(parse_arrow_type_name("bigint").unwrap(), DataType::Int64);
        assert_eq!(parse_arrow_type_name("short").unwrap(), DataType::Int16);
        assert_eq!(parse_arrow_type_name("smallint").unwrap(), DataType::Int16);
        assert_eq!(parse_arrow_type_name("tinyint").unwrap(), DataType::Int8);
        assert_eq!(parse_arrow_type_name("byte").unwrap(), DataType::Int8);
        assert_eq!(parse_arrow_type_name("float").unwrap(), DataType::Float32);
        assert_eq!(parse_arrow_type_name("real").unwrap(), DataType::Float32);
        assert_eq!(parse_arrow_type_name("double").unwrap(), DataType::Float64);
        assert_eq!(parse_arrow_type_name("date").unwrap(), DataType::Date32);
        assert_eq!(parse_arrow_type_name("bytes").unwrap(), DataType::Binary);
        assert_eq!(parse_arrow_type_name("blob").unwrap(), DataType::Binary);
        assert_eq!(parse_arrow_type_name("decimal").unwrap(), DataType::Decimal128(38, 10));
    }

    #[test]
    fn test_parse_aliases_case_insensitive() {
        assert_eq!(parse_arrow_type_name("STRING").unwrap(), DataType::Utf8);
        assert_eq!(parse_arrow_type_name("Bool").unwrap(), DataType::Boolean);
        assert_eq!(parse_arrow_type_name("DOUBLE").unwrap(), DataType::Float64);
        assert_eq!(parse_arrow_type_name("Integer").unwrap(), DataType::Int32);
    }

    #[test]
    fn test_parse_parameterized_case_insensitive() {
        // List
        let list_int = DataType::List(Arc::new(Field::new("item", DataType::Int64, true)));
        assert_eq!(parse_arrow_type_name("list<Int64>").unwrap(), list_int);
        assert_eq!(parse_arrow_type_name("LIST<Int64>").unwrap(), list_int);
        assert_eq!(parse_arrow_type_name("List<int64>").unwrap(), list_int);

        // List with alias
        let list_utf8 = DataType::List(Arc::new(Field::new("item", DataType::Utf8, true)));
        assert_eq!(parse_arrow_type_name("list<string>").unwrap(), list_utf8);
        assert_eq!(parse_arrow_type_name("LIST<STRING>").unwrap(), list_utf8);

        // Decimal128
        assert_eq!(
            parse_arrow_type_name("decimal128(10,2)").unwrap(),
            DataType::Decimal128(10, 2)
        );
        assert_eq!(
            parse_arrow_type_name("DECIMAL128(10,2)").unwrap(),
            DataType::Decimal128(10, 2)
        );
    }

    #[test]
    fn test_parse_map_with_aliases() {
        let result = parse_arrow_type_name("map<string,int>").unwrap();
        let expected_entries = Field::new(
            "entries",
            DataType::Struct(Fields::from(vec![
                Field::new("key", DataType::Utf8, false),
                Field::new("value", DataType::Int32, true),
            ])),
            false,
        );
        assert_eq!(result, DataType::Map(Arc::new(expected_entries), false));
    }

}
