//! Typed representation of every object in the pinned Apache Ossie schema.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// A complete Ossie document, including its required specification version.
///
/// Optional collections retain the distinction between absent and empty values.
/// Use [`crate::parse_model`] or [`Self::validate`] at input boundaries: serde
/// alone does not enforce schema constraints such as nonempty names and arrays.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticModel {
    pub version: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_context: Option<AIContext>,
    pub datasets: Vec<Dataset>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relationships: Option<Vec<Relationship>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<Vec<Metric>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_extensions: Option<Vec<CustomExtension>>,
}

/// AI context may be free text or a structured object with arbitrary extra keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AIContext {
    Text(String),
    Structured(StructuredAIContext),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct StructuredAIContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synonyms: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<String>>,
    #[serde(flatten)]
    pub additional_properties: BTreeMap<String, Value>,
}

/// Vendor data is a string in the standard, typically containing serialized JSON.
/// Vendor names are open strings, rather than a fixed enum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomExtension {
    pub vendor_name: String,
    pub data: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dialect {
    #[serde(rename = "ANSI_SQL")]
    AnsiSql,
    #[serde(rename = "SNOWFLAKE")]
    Snowflake,
    #[serde(rename = "MDX")]
    Mdx,
    #[serde(rename = "TABLEAU")]
    Tableau,
    #[serde(rename = "DATABRICKS")]
    Databricks,
    #[serde(rename = "MAQL")]
    Maql,
    #[serde(rename = "BIGQUERY")]
    BigQuery,
    #[serde(rename = "SIGMA")]
    Sigma,
    #[serde(rename = "THOUGHTSPOT")]
    ThoughtSpot,
    #[serde(rename = "DAX")]
    Dax,
    #[serde(rename = "HOLISTICS_AQL")]
    HolisticsAql,
    #[serde(rename = "OSSIE_SQL_2026")]
    OssieSql2026,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DialectExpression {
    pub dialect: Dialect,
    pub expression: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expression {
    pub dialects: Vec<DialectExpression>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataType {
    String,
    Integer,
    Decimal,
    Float,
    Boolean,
    Date,
    Time,
    DateTime,
    DateTimeTz,
    Opaque,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Dimension {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_time: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub name: String,
    pub expression: Expression,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimension: Option<Dimension>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datatype: Option<DataType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_context: Option<AIContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_extensions: Option<Vec<CustomExtension>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dataset {
    pub name: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_key: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unique_keys: Option<Vec<Vec<String>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_context: Option<AIContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<Field>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_extensions: Option<Vec<CustomExtension>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Relationship {
    pub name: String,
    pub from: String,
    pub to: String,
    pub from_columns: Vec<String>,
    pub to_columns: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_context: Option<AIContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_extensions: Option<Vec<CustomExtension>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metric {
    pub name: String,
    pub expression: Expression,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datatype: Option<DataType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_context: Option<AIContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_extensions: Option<Vec<CustomExtension>>,
}
