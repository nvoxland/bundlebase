//! Shared parameter value type for SQL operations with parameterized queries.
//!
//! This module provides a serializable representation of parameter values
//! that can be converted to/from DataFusion's ScalarValue type.

use datafusion::scalar::ScalarValue;
use serde::{Deserialize, Serialize};

/// Serializable representation of a parameter value for SQL operations.
///
/// This enum represents the common parameter types that can be used in
/// parameterized SQL queries (filter, select, etc.).
#[derive(Debug, Clone, PartialEq)]
pub enum ParameterValue {
    Null,
    Boolean(bool),
    Int64(i64),
    Float64(f64),
    String(String),
}

impl Serialize for ParameterValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;

        let mut map = serializer.serialize_map(Some(2))?;
        match self {
            ParameterValue::Null => {
                map.serialize_entry("type", "null")?;
                map.serialize_entry("value", &None::<String>)?;
            }
            ParameterValue::Boolean(b) => {
                map.serialize_entry("type", "boolean")?;
                map.serialize_entry("value", b)?;
            }
            ParameterValue::Int64(i) => {
                map.serialize_entry("type", "int64")?;
                map.serialize_entry("value", i)?;
            }
            ParameterValue::Float64(f) => {
                map.serialize_entry("type", "float64")?;
                map.serialize_entry("value", &f.to_string())?;
            }
            ParameterValue::String(s) => {
                map.serialize_entry("type", "string")?;
                map.serialize_entry("value", s)?;
            }
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for ParameterValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::MapAccess;

        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "lowercase")]
        enum Field {
            Type,
            Value,
        }

        struct ValueVisitor;

        impl<'de> serde::de::Visitor<'de> for ValueVisitor {
            type Value = ParameterValue;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a parameter value object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut type_str: Option<String> = None;
                let mut value: Option<serde_yaml_ng::Value> = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Type => {
                            type_str = Some(map.next_value()?);
                        }
                        Field::Value => {
                            value = Some(map.next_value()?);
                        }
                    }
                }

                let type_str = type_str.ok_or_else(|| serde::de::Error::missing_field("type"))?;

                match type_str.as_str() {
                    "null" => Ok(ParameterValue::Null),
                    "boolean" => {
                        let v = value.ok_or_else(|| serde::de::Error::missing_field("value"))?;
                        Ok(ParameterValue::Boolean(v.as_bool().ok_or_else(|| {
                            serde::de::Error::custom("invalid boolean")
                        })?))
                    }
                    "int64" => {
                        let v = value.ok_or_else(|| serde::de::Error::missing_field("value"))?;
                        Ok(ParameterValue::Int64(v.as_i64().ok_or_else(|| {
                            serde::de::Error::custom("invalid int64")
                        })?))
                    }
                    "float64" => {
                        let v = value.ok_or_else(|| serde::de::Error::missing_field("value"))?;
                        let f_str = v
                            .as_str()
                            .ok_or_else(|| serde::de::Error::custom("invalid float64"))?;
                        let f = f_str.parse::<f64>().map_err(serde::de::Error::custom)?;
                        Ok(ParameterValue::Float64(f))
                    }
                    "string" => {
                        let v = value.ok_or_else(|| serde::de::Error::missing_field("value"))?;
                        Ok(ParameterValue::String(
                            v.as_str()
                                .ok_or_else(|| serde::de::Error::custom("invalid string"))?
                                .to_string(),
                        ))
                    }
                    _ => Err(serde::de::Error::custom("unknown parameter type")),
                }
            }
        }

        deserializer.deserialize_map(ValueVisitor)
    }
}

impl ParameterValue {
    /// Convert to DataFusion ScalarValue
    pub fn to_scalar_value(&self) -> ScalarValue {
        match self {
            ParameterValue::Null => ScalarValue::Null,
            ParameterValue::Boolean(b) => ScalarValue::Boolean(Some(*b)),
            ParameterValue::Int64(i) => ScalarValue::Int64(Some(*i)),
            ParameterValue::Float64(f) => ScalarValue::Float64(Some(*f)),
            ParameterValue::String(s) => ScalarValue::Utf8(Some(s.clone())),
        }
    }
}

impl From<ScalarValue> for ParameterValue {
    fn from(sv: ScalarValue) -> Self {
        match sv {
            ScalarValue::Null => ParameterValue::Null,
            ScalarValue::Boolean(Some(b)) => ParameterValue::Boolean(b),
            ScalarValue::Boolean(None) => ParameterValue::Null,
            ScalarValue::Int64(Some(i)) => ParameterValue::Int64(i),
            ScalarValue::Int64(None) => ParameterValue::Null,
            ScalarValue::Float64(Some(f)) => ParameterValue::Float64(f),
            ScalarValue::Float64(None) => ParameterValue::Null,
            ScalarValue::Utf8(Some(s)) => ParameterValue::String(s),
            ScalarValue::Utf8(None) => ParameterValue::Null,
            // For other types, convert to string representation then store as String
            other => ParameterValue::String(other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parameter_value_roundtrip() {
        let values = vec![
            ParameterValue::Null,
            ParameterValue::Boolean(true),
            ParameterValue::Int64(42),
            ParameterValue::Float64(3.14),
            ParameterValue::String("hello".to_string()),
        ];

        for value in values {
            let serialized = serde_yaml_ng::to_string(&value).expect("serialize");
            let deserialized: ParameterValue =
                serde_yaml_ng::from_str(&serialized).expect("deserialize");
            assert_eq!(value, deserialized);
        }
    }

    #[test]
    fn test_scalar_value_conversion() {
        let scalar = ScalarValue::Int64(Some(42));
        let param = ParameterValue::from(scalar);
        assert_eq!(param, ParameterValue::Int64(42));

        let back = param.to_scalar_value();
        assert_eq!(back, ScalarValue::Int64(Some(42)));
    }
}
