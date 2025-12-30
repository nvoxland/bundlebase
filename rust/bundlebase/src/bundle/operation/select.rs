use crate::bundle::facade::BundleFacade;
use crate::bundle::operation::Operation;
use crate::bundle::sql::with_temp_table;
use crate::metrics::{start_span, OperationCategory, OperationOutcome, OperationTimer};
use crate::{Bundle, BundlebaseError};
use async_trait::async_trait;
use datafusion::common::DataFusionError;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use datafusion::scalar::ScalarValue;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Serializable representation of a select parameter value
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
                let mut value: Option<serde_yaml::Value> = None;

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SelectOp {
    pub sql: String,
    pub parameters: Vec<ParameterValue>,
}

impl SelectOp {
    pub async fn setup(sql: String, parameters: Vec<ScalarValue>) -> Result<Self, BundlebaseError> {
        // Substitute parameters into SQL for schema inference
        let mut substituted_sql = sql.clone();
        for (i, param) in parameters.iter().enumerate() {
            let placeholder = format!("${}", i + 1);
            let value_str = crate::bundle::scalar_value_to_sql_literal(param);
            substituted_sql = substituted_sql.replace(&placeholder, &value_str);
        }

        Ok(Self {
            sql,
            parameters: parameters.into_iter().map(ParameterValue::from).collect(),
        })
    }
}

#[async_trait]
impl Operation for SelectOp {
    fn describe(&self) -> String {
        format!("{}", self.sql)
    }

    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        Ok(())
    }

    async fn apply(&self, _bundle: &mut Bundle) -> Result<(), DataFusionError> {
        Ok(())
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        ctx: Arc<SessionContext>,
    ) -> Result<DataFrame, BundlebaseError> {
        let mut span = start_span(OperationCategory::Select, "sql");
        span.set_attribute("sql", &self.sql);
        span.set_attribute("param_count", &self.parameters.len().to_string());

        let timer = OperationTimer::start(OperationCategory::Select, "sql");

        let user_sql = self.sql.clone();
        let parameters = self.parameters.clone();
        let ctx_for_closure = ctx.clone();

        let result = with_temp_table(&ctx, df, |table_name| {
            async move {
                // Substitute parameters into SQL
                let mut sql = user_sql;
                for (i, param) in parameters.iter().enumerate() {
                    let placeholder = format!("${}", i + 1);
                    let value_str =
                        crate::bundle::scalar_value_to_sql_literal(&param.to_scalar_value());
                    sql = sql.replace(&placeholder, &value_str);
                }

                // Replace "data" references with table_name in user SQL
                sql = sql.replace("data", &table_name);

                // Execute the SQL query
                ctx_for_closure
                    .sql(&sql)
                    .await
                    .map_err(|e| Box::new(e) as BundlebaseError)
            }
        })
        .await;

        match &result {
            Ok(_) => {
                span.set_outcome(OperationOutcome::Success);
                timer.finish(OperationOutcome::Success);
            }
            Err(e) => {
                span.record_error(&e.to_string());
                timer.finish(OperationOutcome::Error);
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow_schema::SchemaRef;

    fn create_test_schema() -> SchemaRef {
        SchemaRef::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, true),
            Field::new("salary", DataType::Float64, true),
        ]))
    }

    #[test]
    fn test_describe() {
        let sql = "SELECT * FROM data WHERE salary > $1";
        let op = SelectOp {
            sql: sql.to_string(),
            parameters: vec![ParameterValue::Float64(50000.0)],
        };
        assert_eq!(op.describe(), format!("{}", sql));
    }

    #[test]
    fn test_config_serialization() {
        let sql = "SELECT * FROM data WHERE salary > $1 AND name = $2";
        let config = SelectOp {
            sql: sql.to_string(),
            parameters: vec![
                ParameterValue::Float64(50000.0),
                ParameterValue::String("USA".to_string()),
            ],
        };

        // Verify serialization is possible
        let serialized = serde_yaml::to_string(&config).expect("Failed to serialize");
        assert!(serialized.contains("sql"));
        assert!(serialized.contains("parameters"));
        assert!(serialized.contains("float64") || serialized.contains("50000"));
        assert!(serialized.contains("string") || serialized.contains("USA"));

        // Verify we can deserialize back
        let deserialized: SelectOp =
            serde_yaml::from_str(&serialized).expect("Failed to deserialize");
        assert_eq!(deserialized.sql, sql);
        assert_eq!(deserialized.parameters.len(), 2);
    }

    #[test]
    fn test_version() {
        let op = SelectOp {
            sql: "SELECT * FROM data".to_string(),
            parameters: vec![],
        };
        let version = op.version();
        // Just verify it returns a version string
        assert!(!version.is_empty());
        assert_eq!(version.len(), 12); // SHA256 short hash format
    }
}
