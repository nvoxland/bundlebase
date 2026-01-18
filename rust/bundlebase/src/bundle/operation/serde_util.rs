use arrow_schema::{DataType, Field, Schema, TimeUnit};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_yaml_ng::{Mapping, Value};
use std::sync::Arc;

/// Helper struct for field serialization
#[derive(Serialize, Deserialize)]
pub struct SerializedField {
    pub name: String,
    pub data_type: Value,
    pub nullable: bool,
    pub dict_id: i32,
    pub dict_is_ordered: bool,
    pub metadata: Mapping,
}

/// Serialize a DataType to structured YAML format (avoiding YAML tags)
fn serialize_data_type(dt: &DataType) -> Result<Value, String> {
    match dt {
        // Only include branches for types that don't serialize correctly
        // Complex types with parameters - use structured format
        DataType::Timestamp(unit, tz) => {
            let mut map = Mapping::new();
            map.insert(
                Value::String("type".to_string()),
                Value::String("Timestamp".to_string()),
            );
            map.insert(
                Value::String("unit".to_string()),
                Value::String(format!("{:?}", unit)),
            );
            if let Some(tz_str) = tz {
                map.insert(
                    Value::String("timezone".to_string()),
                    Value::String(tz_str.as_ref().to_string()),
                );
            } else {
                map.insert(Value::String("timezone".to_string()), Value::Null);
            }
            Ok(Value::Mapping(map))
        }

        DataType::Time32(unit) => {
            let mut map = Mapping::new();
            map.insert(
                Value::String("type".to_string()),
                Value::String("Time32".to_string()),
            );
            map.insert(
                Value::String("unit".to_string()),
                Value::String(format!("{:?}", unit)),
            );
            Ok(Value::Mapping(map))
        }

        DataType::Time64(unit) => {
            let mut map = Mapping::new();
            map.insert(
                Value::String("type".to_string()),
                Value::String("Time64".to_string()),
            );
            map.insert(
                Value::String("unit".to_string()),
                Value::String(format!("{:?}", unit)),
            );
            Ok(Value::Mapping(map))
        }

        DataType::Duration(unit) => {
            let mut map = Mapping::new();
            map.insert(
                Value::String("type".to_string()),
                Value::String("Duration".to_string()),
            );
            map.insert(
                Value::String("unit".to_string()),
                Value::String(format!("{:?}", unit)),
            );
            Ok(Value::Mapping(map))
        }

        DataType::Interval(unit) => {
            let mut map = Mapping::new();
            map.insert(
                Value::String("type".to_string()),
                Value::String("Interval".to_string()),
            );
            map.insert(
                Value::String("unit".to_string()),
                Value::String(format!("{:?}", unit)),
            );
            Ok(Value::Mapping(map))
        }

        DataType::Decimal128(precision, scale) => {
            let mut map = Mapping::new();
            map.insert(
                Value::String("type".to_string()),
                Value::String("Decimal128".to_string()),
            );
            map.insert(
                Value::String("precision".to_string()),
                Value::Number((*precision).into()),
            );
            map.insert(
                Value::String("scale".to_string()),
                Value::Number((*scale).into()),
            );
            Ok(Value::Mapping(map))
        }

        DataType::Decimal256(precision, scale) => {
            let mut map = Mapping::new();
            map.insert(
                Value::String("type".to_string()),
                Value::String("Decimal256".to_string()),
            );
            map.insert(
                Value::String("precision".to_string()),
                Value::Number((*precision).into()),
            );
            map.insert(
                Value::String("scale".to_string()),
                Value::Number((*scale).into()),
            );
            Ok(Value::Mapping(map))
        }

        DataType::List(field) | DataType::LargeList(field) | DataType::FixedSizeList(field, _) => {
            let mut map = Mapping::new();
            let type_name = match dt {
                DataType::List(_) => "List",
                DataType::LargeList(_) => "LargeList",
                DataType::FixedSizeList(_, _) => "FixedSizeList",
                _ => unreachable!(),
            };
            map.insert(
                Value::String("type".to_string()),
                Value::String(type_name.to_string()),
            );

            // Serialize the field recursively
            let element_value = serde_yaml_ng::to_value(serialize_field_internal(field)?)
                .map_err(|e| e.to_string())?;
            map.insert(Value::String("element".to_string()), element_value);

            if let DataType::FixedSizeList(_, size) = dt {
                map.insert(
                    Value::String("size".to_string()),
                    Value::Number((*size).into()),
                );
            }
            Ok(Value::Mapping(map))
        }

        DataType::Struct(fields) => {
            let mut map = Mapping::new();
            map.insert(
                Value::String("type".to_string()),
                Value::String("Struct".to_string()),
            );

            let mut fields_value = Vec::new();
            for f in fields.iter() {
                let serialized_field = serialize_field_internal(f).expect("should deserialize");
                let field_value =
                    serde_yaml_ng::to_value(serialized_field).map_err(|e| e.to_string())?;
                fields_value.push(field_value);
            }

            let fields_array = Value::Sequence(fields_value);
            map.insert(Value::String("fields".to_string()), fields_array);
            Ok(Value::Mapping(map))
        }

        // For other complex types, use a simple string representation
        _ => serde_yaml_ng::to_value(dt).map_err(|e| e.to_string()),
    }
}

/// Deserialize a DataType from structured YAML format
fn deserialize_data_type(value: &Value) -> Result<DataType, String> {
    match value {
        Value::String(s) => {
            let dt: DataType = serde_yaml_ng::from_str(s).map_err(|e| e.to_string())?;
            Ok(dt)
        }
        Value::Mapping(map) => {
            let type_str = map
                .get(&Value::String("type".to_string()))
                .and_then(|v| v.as_str())
                .ok_or_else(|| "missing 'type' field".to_string())?;

            match type_str {
                "Timestamp" => {
                    let unit_str = map
                        .get(&Value::String("unit".to_string()))
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "missing 'unit' field in Timestamp".to_string())?;

                    let unit = match unit_str {
                        "Second" => TimeUnit::Second,
                        "Millisecond" => TimeUnit::Millisecond,
                        "Microsecond" => TimeUnit::Microsecond,
                        "Nanosecond" => TimeUnit::Nanosecond,
                        _ => return Err(format!("Invalid TimeUnit: {}", unit_str)),
                    };

                    let tz = map
                        .get(&Value::String("timezone".to_string()))
                        .and_then(|v| match v {
                            Value::Null => Some(None),
                            Value::String(s) => Some(Some(Arc::from(s.as_str()))),
                            _ => None,
                        })
                        .ok_or_else(|| "invalid 'timezone' field".to_string())?;

                    Ok(DataType::Timestamp(unit, tz))
                }
                "Time32" => {
                    let unit_str = map
                        .get(&Value::String("unit".to_string()))
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "missing 'unit' field in Time32".to_string())?;

                    let unit = match unit_str {
                        "Second" => TimeUnit::Second,
                        "Millisecond" => TimeUnit::Millisecond,
                        _ => return Err(format!("Invalid TimeUnit for Time32: {}", unit_str)),
                    };

                    Ok(DataType::Time32(unit))
                }
                "Time64" => {
                    let unit_str = map
                        .get(&Value::String("unit".to_string()))
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "missing 'unit' field in Time64".to_string())?;

                    let unit = match unit_str {
                        "Microsecond" => TimeUnit::Microsecond,
                        "Nanosecond" => TimeUnit::Nanosecond,
                        _ => return Err(format!("Invalid TimeUnit for Time64: {}", unit_str)),
                    };

                    Ok(DataType::Time64(unit))
                }
                "Duration" => {
                    let unit_str = map
                        .get(&Value::String("unit".to_string()))
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "missing 'unit' field in Duration".to_string())?;

                    let unit = match unit_str {
                        "Second" => TimeUnit::Second,
                        "Millisecond" => TimeUnit::Millisecond,
                        "Microsecond" => TimeUnit::Microsecond,
                        "Nanosecond" => TimeUnit::Nanosecond,
                        _ => return Err(format!("Unknown TimeUnit: {}", unit_str)),
                    };

                    Ok(DataType::Duration(unit))
                }
                "Interval" => {
                    let unit_str = map
                        .get(&Value::String("unit".to_string()))
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "missing 'unit' field in Interval".to_string())?;

                    let unit = match unit_str {
                        "YearMonth" => arrow_schema::IntervalUnit::YearMonth,
                        "DayTime" => arrow_schema::IntervalUnit::DayTime,
                        "MonthDayNano" => arrow_schema::IntervalUnit::MonthDayNano,
                        _ => return Err(format!("Unknown IntervalUnit: {}", unit_str)),
                    };

                    Ok(DataType::Interval(unit))
                }
                "Decimal128" => {
                    let precision = map
                        .get(&Value::String("precision".to_string()))
                        .and_then(|v| v.as_u64())
                        .ok_or_else(|| "missing 'precision' field in Decimal128".to_string())?
                        as u8;

                    let scale = map
                        .get(&Value::String("scale".to_string()))
                        .and_then(|v| v.as_i64())
                        .ok_or_else(|| "missing 'scale' field in Decimal128".to_string())?
                        as i8;

                    Ok(DataType::Decimal128(precision, scale))
                }
                "Decimal256" => {
                    let precision = map
                        .get(&Value::String("precision".to_string()))
                        .and_then(|v| v.as_u64())
                        .ok_or_else(|| "missing 'precision' field in Decimal256".to_string())?
                        as u8;

                    let scale = map
                        .get(&Value::String("scale".to_string()))
                        .and_then(|v| v.as_i64())
                        .ok_or_else(|| "missing 'scale' field in Decimal256".to_string())?
                        as i8;

                    Ok(DataType::Decimal256(precision, scale))
                }
                "List" => {
                    let element_value = map
                        .get(&Value::String("element".to_string()))
                        .ok_or_else(|| "missing 'element' field in List".to_string())?;

                    let field =
                        deserialize_field_internal(element_value).expect("should deserialize");
                    Ok(DataType::List(Arc::new(field)))
                }
                "LargeList" => {
                    let element_value = map
                        .get(&Value::String("element".to_string()))
                        .ok_or_else(|| "missing 'element' field in LargeList".to_string())?;

                    let field =
                        deserialize_field_internal(element_value).expect("should deserialize");
                    Ok(DataType::LargeList(Arc::new(field)))
                }
                "FixedSizeList" => {
                    let element_value = map
                        .get(&Value::String("element".to_string()))
                        .ok_or_else(|| "missing 'element' field in FixedSizeList".to_string())?;

                    let field =
                        deserialize_field_internal(element_value).expect("should deserialize");

                    let size = map
                        .get(&Value::String("size".to_string()))
                        .and_then(|v| v.as_i64())
                        .ok_or_else(|| "missing 'size' field in FixedSizeList".to_string())?
                        as i32;

                    Ok(DataType::FixedSizeList(Arc::new(field), size))
                }
                "Struct" => {
                    let fields_value = map
                        .get(&Value::String("fields".to_string()))
                        .ok_or_else(|| "missing 'fields' field in Struct".to_string())?;

                    let fields: Result<Vec<Field>, String> = match fields_value {
                        Value::Sequence(seq) => {
                            seq.iter().map(deserialize_field_internal).collect()
                        }
                        _ => Err("fields must be a sequence".to_string()),
                    };

                    let fields_vec = fields?;
                    Ok(DataType::Struct(arrow_schema::Fields::from(fields_vec)))
                }
                _ => Err(format!("Unknown DataType: {}", type_str)),
            }
        }
        _ => Err("DataType must be string or mapping".to_string()),
    }
}

fn serialize_field_internal(field: &Field) -> Result<SerializedField, String> {
    let metadata_map = serde_yaml_ng::to_value(field.metadata())
        .ok()
        .and_then(|v| v.as_mapping().cloned())
        .unwrap_or_default();

    Ok(SerializedField {
        name: field.name().clone(),
        data_type: serialize_data_type(field.data_type())?,
        nullable: field.is_nullable(),
        dict_id: 0,
        dict_is_ordered: false,
        metadata: metadata_map,
    })
}

fn deserialize_field_internal(value: &Value) -> Result<Field, String> {
    let map = value
        .as_mapping()
        .ok_or_else(|| "Field must be a mapping".to_string())?;

    let name = map
        .get(&Value::String("name".to_string()))
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing 'name' field".to_string())?
        .to_string();

    let data_type_value = map
        .get(&Value::String("data_type".to_string()))
        .ok_or_else(|| "missing 'data_type' field".to_string())?;

    let data_type = deserialize_data_type(data_type_value).expect("should deserialize");

    let nullable = map
        .get(&Value::String("nullable".to_string()))
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let metadata = map
        .get(&Value::String("metadata".to_string()))
        .and_then(|v| v.as_mapping())
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| {
                    k.as_str()
                        .and_then(|ks| v.as_str().map(|vs| (ks.to_string(), vs.to_string())))
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(Field::new(name, data_type, nullable).with_metadata(metadata))
}

/// Serialize Schema with custom DataType handling
pub fn serialize_schema<S>(schema: &Arc<Schema>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut map = Mapping::new();

    // Serialize fields
    let fields_values: Vec<Value> = schema
        .fields()
        .iter()
        .filter_map(|f| {
            serialize_field_internal(f)
                .ok()
                .and_then(|sf| serde_yaml_ng::to_value(sf).ok())
        })
        .collect();

    map.insert(
        Value::String("fields".to_string()),
        Value::Sequence(fields_values),
    );
    map.insert(
        Value::String("metadata".to_string()),
        Value::Mapping(Mapping::new()),
    );

    serde_yaml_ng::to_value(&map)
        .map_err(serde::ser::Error::custom)
        .and_then(|v| v.serialize(serializer))
}

/// Serialize Option<Arc<Schema>>
pub fn serialize_schema_option<S>(
    schema: &Option<Arc<Schema>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match schema {
        Some(s) => serialize_schema(s, serializer),
        None => serializer.serialize_none(),
    }
}

/// Deserialize Option<Arc<Schema>>
pub fn deserialize_schema_option<'de, D>(deserializer: D) -> Result<Option<Arc<Schema>>, D::Error>
where
    D: Deserializer<'de>,
{
    let value: Option<serde_yaml_ng::Value> =
        Option::deserialize(deserializer).map_err(serde::de::Error::custom)?;
    match value {
        Some(v) => deserialize_schema_internal(&v)
            .map(Some)
            .map_err(serde::de::Error::custom),
        None => Ok(None),
    }
}

fn deserialize_schema_internal(value: &serde_yaml_ng::Value) -> Result<Arc<Schema>, String> {
    let map = value
        .as_mapping()
        .ok_or_else(|| "Schema must be a mapping".to_string())?;

    let fields_value = map
        .get(&Value::String("fields".to_string()))
        .ok_or_else(|| "missing 'fields' field".to_string())?;

    let fields: Result<Vec<Field>, String> = match fields_value {
        Value::Sequence(seq) => seq.iter().map(deserialize_field_internal).collect(),
        _ => Err("fields must be a sequence".to_string()),
    };

    let fields_vec = fields?;
    Ok(Arc::new(Schema::new(fields_vec)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    /// Helper to roundtrip a DataType through serialization
    fn roundtrip_datatype(dt: &DataType) -> Result<DataType, String> {
        let serialized = serialize_data_type(dt).expect("should deserialize");
        deserialize_data_type(&serialized)
    }

    #[rstest]
    #[case(DataType::Null)]
    #[case(DataType::Boolean)]
    #[case(DataType::Int8)]
    #[case(DataType::Int16)]
    #[case(DataType::Int32)]
    #[case(DataType::Int64)]
    #[case(DataType::UInt8)]
    #[case(DataType::UInt16)]
    #[case(DataType::UInt32)]
    #[case(DataType::UInt64)]
    #[case(DataType::Float32)]
    #[case(DataType::Float64)]
    #[case(DataType::Utf8)]
    #[case(DataType::Utf8View)]
    #[case(DataType::Binary)]
    #[case(DataType::BinaryView)]
    #[case(DataType::LargeBinary)]
    #[case(DataType::LargeUtf8)]
    #[case(DataType::Date32)]
    #[case(DataType::Date64)]
    fn test_simple_type_roundtrip(#[case] dt: DataType) {
        let result = roundtrip_datatype(&dt).expect("should deserialize");
        assert_eq!(result, dt, "roundtrip should preserve DataType");
    }

    #[rstest]
    #[case(TimeUnit::Second)]
    #[case(TimeUnit::Millisecond)]
    #[case(TimeUnit::Microsecond)]
    #[case(TimeUnit::Nanosecond)]
    fn test_timestamp_roundtrip(#[case] unit: TimeUnit) {
        //no timezones
        let dt = DataType::Timestamp(unit, None);
        let result = roundtrip_datatype(&dt).expect("should deserialize");
        assert_eq!(result, dt);

        for tz in ["UTC", "America/New_York", "Europe/London", "Asia/Tokyo"] {
            let dt = DataType::Timestamp(TimeUnit::Nanosecond, Some(Arc::from(tz)));
            let result = roundtrip_datatype(&dt).expect("should deserialize");
            assert_eq!(result, dt);
        }
    }

    #[rstest]
    #[case(TimeUnit::Second)]
    #[case(TimeUnit::Millisecond)]
    fn test_time32_roundtrip(#[case] unit: TimeUnit) {
        let dt = DataType::Time32(unit);
        let result = roundtrip_datatype(&dt).expect("should deserialize");
        assert_eq!(result, dt);
    }

    #[rstest]
    #[case(TimeUnit::Microsecond)]
    #[case(TimeUnit::Nanosecond)]
    fn test_time64_roundtrip(#[case] unit: TimeUnit) {
        let dt = DataType::Time64(unit);
        let result = roundtrip_datatype(&dt).expect("should deserialize");
        assert_eq!(result, dt);
    }

    #[rstest]
    #[case(TimeUnit::Second)]
    #[case(TimeUnit::Millisecond)]
    #[case(TimeUnit::Microsecond)]
    #[case(TimeUnit::Nanosecond)]
    fn test_duration_roundtrip(#[case] unit: TimeUnit) {
        let dt = DataType::Duration(unit);
        let result = roundtrip_datatype(&dt).expect("should deserialize");
        assert_eq!(result, dt);
    }

    #[rstest]
    #[case(arrow_schema::IntervalUnit::YearMonth)]
    #[case(arrow_schema::IntervalUnit::DayTime)]
    #[case(arrow_schema::IntervalUnit::MonthDayNano)]
    fn test_interval_roundtrip(#[case] unit: arrow_schema::IntervalUnit) {
        let dt = DataType::Interval(unit);
        let result = roundtrip_datatype(&dt).expect("should deserialize");
        assert_eq!(result, dt);
    }

    #[rstest]
    #[case(10, 2)]
    #[case(38, 10)]
    #[case(19, 0)]
    fn test_decimal128_roundtrip(#[case] precision: u8, #[case] scale: i8) {
        let dt = DataType::Decimal128(precision, scale);
        let result = roundtrip_datatype(&dt).expect("should deserialize");
        assert_eq!(result, dt);
    }

    #[rstest]
    #[case(38, 18)]
    #[case(76, 38)]
    fn test_decimal256_roundtrip(#[case] precision: u8, #[case] scale: i8) {
        let dt = DataType::Decimal256(precision, scale);
        let result = roundtrip_datatype(&dt).expect("should deserialize");
        assert_eq!(result, dt);
    }

    #[test]
    fn test_list_roundtrip() {
        let inner_field = Field::new("element", DataType::Int32, true);
        let dt = DataType::List(Arc::new(inner_field));
        let result = roundtrip_datatype(&dt).unwrap();
        assert_eq!(result, dt);
    }

    #[test]
    fn test_large_list_roundtrip() {
        let inner_field = Field::new("element", DataType::Utf8, true);
        let dt = DataType::LargeList(Arc::new(inner_field));
        let result = roundtrip_datatype(&dt).unwrap();
        assert_eq!(result, dt);
    }

    #[test]
    fn test_fixed_size_list_roundtrip() {
        let inner_field = Field::new("element", DataType::Float64, true);
        let dt = DataType::FixedSizeList(Arc::new(inner_field), 5);
        let result = roundtrip_datatype(&dt).unwrap();
        assert_eq!(result, dt);
    }

    #[test]
    fn test_struct_simple_roundtrip() {
        let fields = vec![
            Field::new("id", DataType::Int32, false),
            Field::new("name", DataType::Utf8, true),
        ];
        let dt = DataType::Struct(arrow_schema::Fields::from(fields));
        let result = roundtrip_datatype(&dt).unwrap();
        assert_eq!(result, dt);
    }

    #[test]
    fn test_struct_nested_roundtrip() {
        let inner_fields = vec![Field::new("value", DataType::Float64, true)];
        let inner_struct = DataType::Struct(arrow_schema::Fields::from(inner_fields));

        let fields = vec![
            Field::new("id", DataType::Int32, false),
            Field::new("nested", inner_struct, true),
        ];
        let dt = DataType::Struct(arrow_schema::Fields::from(fields));
        let result = roundtrip_datatype(&dt).unwrap();
        assert_eq!(result, dt);
    }

    #[test]
    fn test_struct_with_timestamp_roundtrip() {
        let fields = vec![
            Field::new("id", DataType::Int32, false),
            Field::new(
                "created_at",
                DataType::Timestamp(TimeUnit::Nanosecond, Some(Arc::from("UTC"))),
                true,
            ),
        ];
        let dt = DataType::Struct(arrow_schema::Fields::from(fields));
        let result = roundtrip_datatype(&dt).unwrap();
        assert_eq!(result, dt);
    }

    #[test]
    fn test_field_roundtrip() {
        let field = Field::new("age", DataType::Int32, true);

        let serialized = serialize_field_internal(&field).expect("should serialize");
        let deserialized = deserialize_field_internal(
            &serde_yaml_ng::to_value(&serialized).expect("should serialize"),
        )
        .unwrap();

        assert_eq!(field.name(), deserialized.name());
        assert_eq!(field.data_type(), deserialized.data_type());
        assert_eq!(field.is_nullable(), deserialized.is_nullable());
    }

    #[test]
    fn test_field_with_metadata_roundtrip() {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("description".to_string(), "User age in years".to_string());
        metadata.insert("unit".to_string(), "years".to_string());

        let field = Field::new("age", DataType::Int32, true).with_metadata(metadata.clone());

        let serialized = serialize_field_internal(&field).expect("should serialize");
        let deserialized = deserialize_field_internal(
            &serde_yaml_ng::to_value(&serialized).expect("should serialize"),
        )
        .unwrap();

        assert_eq!(field.metadata(), deserialized.metadata());
    }

    #[test]
    fn test_field_complex_type_roundtrip() {
        let field = Field::new(
            "timestamp",
            DataType::Timestamp(TimeUnit::Nanosecond, Some(Arc::from("UTC"))),
            false,
        );

        let serialized = serialize_field_internal(&field).expect("should serialize");
        let deserialized = deserialize_field_internal(
            &serde_yaml_ng::to_value(&serialized).expect("should serialize"),
        )
        .unwrap();

        assert_eq!(field.name(), deserialized.name());
        assert_eq!(field.data_type(), deserialized.data_type());
        assert_eq!(field.is_nullable(), deserialized.is_nullable());
    }

    #[test]
    fn test_schema_simple_roundtrip() {
        let fields = vec![
            Field::new("id", DataType::Int32, false),
            Field::new("name", DataType::Utf8, true),
            Field::new("age", DataType::Int32, true),
        ];
        let schema = Arc::new(Schema::new(fields));

        let serialized = serde_yaml_ng::to_value(schema.clone()).expect("should serialize");
        let deserialized = deserialize_schema_internal(&serialized).unwrap();

        assert_eq!(schema.fields().len(), deserialized.fields().len());
        for (original, deserialized) in schema.fields().iter().zip(deserialized.fields().iter()) {
            assert_eq!(original.name(), deserialized.name());
            assert_eq!(original.data_type(), deserialized.data_type());
        }
    }

    #[test]
    fn test_schema_with_complex_types_roundtrip() {
        let fields = vec![
            Field::new("id", DataType::Int32, false),
            Field::new(
                "created_at",
                DataType::Timestamp(TimeUnit::Nanosecond, Some(Arc::from("UTC"))),
                true,
            ),
            Field::new("amount", DataType::Decimal128(10, 2), true),
        ];
        let schema = Arc::new(Schema::new(fields));

        // Manually serialize using serialize_schema_internal logic
        let mut map = Mapping::new();
        let fields_values: Vec<Value> = schema
            .fields()
            .iter()
            .filter_map(|f| {
                serialize_field_internal(f)
                    .ok()
                    .and_then(|sf| serde_yaml_ng::to_value(sf).ok())
            })
            .collect();
        map.insert(
            Value::String("fields".to_string()),
            Value::Sequence(fields_values),
        );
        map.insert(
            Value::String("metadata".to_string()),
            Value::Mapping(Mapping::new()),
        );
        let serialized = Value::Mapping(map);

        let deserialized = deserialize_schema_internal(&serialized).unwrap();

        assert_eq!(schema.fields().len(), deserialized.fields().len());
        for (original, deserialized) in schema.fields().iter().zip(deserialized.fields().iter()) {
            assert_eq!(original.name(), deserialized.name());
            assert_eq!(original.data_type(), deserialized.data_type());
        }
    }

    #[test]
    fn test_serialize_data_type_simple_produces_string() {
        let dt = DataType::Int32;
        let result = serialize_data_type(&dt).expect("should serialize");
        assert!(
            matches!(result, Value::String(_)),
            "simple types should serialize to strings"
        );
    }

    #[test]
    fn test_serialize_data_type_timestamp_produces_mapping() {
        let dt = DataType::Timestamp(TimeUnit::Nanosecond, Some(Arc::from("UTC")));
        let result = serialize_data_type(&dt).expect("should serialize");
        assert!(
            matches!(result, Value::Mapping(_)),
            "Timestamp should serialize to mapping"
        );

        let map = result.as_mapping().unwrap();
        assert!(map.contains_key(&Value::String("type".to_string())));
        assert!(map.contains_key(&Value::String("unit".to_string())));
        assert!(map.contains_key(&Value::String("timezone".to_string())));
    }

    #[test]
    fn test_deserialize_invalid_datatype_fails() {
        let value = Value::String("InvalidType".to_string());
        let result = deserialize_data_type(&value);
        assert!(result.is_err(), "unknown type should fail");
    }

    #[test]
    fn test_deserialize_timestamp_missing_unit_fails() {
        let mut map = Mapping::new();
        map.insert(
            Value::String("type".to_string()),
            Value::String("Timestamp".to_string()),
        );
        // missing unit field
        let value = Value::Mapping(map);
        let result = deserialize_data_type(&value);
        assert!(result.is_err(), "missing unit should fail");
    }

    #[test]
    fn test_deserialize_decimal128_invalid_precision_fails() {
        let mut map = Mapping::new();
        map.insert(
            Value::String("type".to_string()),
            Value::String("Decimal128".to_string()),
        );
        map.insert(
            Value::String("precision".to_string()),
            Value::String("not_a_number".to_string()),
        );
        map.insert(Value::String("scale".to_string()), Value::Number(2.into()));

        let value = Value::Mapping(map);
        let result = deserialize_data_type(&value);
        assert!(result.is_err(), "invalid precision should fail");
    }

    #[test]
    fn test_deserialize_field_missing_name_fails() {
        let mut map = Mapping::new();
        map.insert(
            Value::String("data_type".to_string()),
            Value::String("Int32".to_string()),
        );
        let value = Value::Mapping(map);
        let result = deserialize_field_internal(&value);
        assert!(result.is_err(), "missing name should fail");
    }

    #[test]
    fn test_list_of_lists_roundtrip() {
        let inner_inner_field = Field::new("element", DataType::Int32, true);
        let inner_list = DataType::List(Arc::new(inner_inner_field));
        let inner_field = Field::new("element", inner_list, true);
        let dt = DataType::List(Arc::new(inner_field));

        let result = roundtrip_datatype(&dt).unwrap();
        assert_eq!(result, dt);
    }

    #[test]
    fn test_serialize_null_schema_option() {
        let schema: Option<Arc<Schema>> = None;
        let value = serde_yaml_ng::to_value(&schema).expect("should serialize");
        assert!(value.is_null(), "None should serialize to null");
    }

    #[test]
    fn test_deserialize_null_schema_option() {
        let value = Value::Null;
        let result = deserialize_schema_internal(&value);
        assert!(result.is_err(), "null should fail schema deserialization");
    }
}
