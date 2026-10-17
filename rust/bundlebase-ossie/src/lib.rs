#![deny(clippy::unwrap_used)]

//! Apache Ossie documents, backed by an immutable, vendored upstream schema.
//!
//! This crate has no Bundlebase core, Arrow, or DataFusion dependency. Parsing,
//! validation, and serialization operate offline; no schema is fetched at build
//! time or runtime. Validation covers the full standard rather than any adapter's
//! supported execution subset. It does not interpret or execute expressions.

pub mod model;
pub use model::*;

use serde_json::Value;
use std::fmt::{Display, Formatter};
use std::sync::OnceLock;

pub const SPEC_VERSION: &str = "0.2.0.dev0";
pub const UPSTREAM_COMMIT: &str = "64a1ae44035f5360dcd53401a29bcd900fdce349";
pub const SCHEMA_JSON: &str = include_str!("../vendor/ossie-schema.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentFormat {
    Json,
    Yaml,
    /// JSON-looking documents are parsed strictly as JSON; all others as YAML.
    Auto,
}

/// A schema violation with RFC 6901 pointers into the input and pinned schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    /// The empty string identifies the root document.
    pub instance_path: String,
    pub schema_path: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OssieError {
    Parse(String),
    Schema(String),
    Validation(Vec<ValidationIssue>),
    Serialization(String),
}

impl Display for OssieError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(message) => write!(f, "Cannot parse Ossie document: {message}"),
            Self::Schema(message) => write!(f, "Cannot compile pinned Ossie schema: {message}"),
            Self::Serialization(message) => {
                write!(f, "Cannot serialize Ossie document: {message}")
            }
            Self::Validation(issues) => {
                write!(f, "Invalid Ossie document")?;
                for issue in issues {
                    write!(
                        f,
                        "; JSON pointer {:?}: {} (schema {})",
                        issue.instance_path, issue.message, issue.schema_path
                    )?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for OssieError {}

fn validator() -> Result<&'static jsonschema::Validator, OssieError> {
    static VALIDATOR: OnceLock<Result<jsonschema::Validator, String>> = OnceLock::new();
    VALIDATOR
        .get_or_init(|| {
            let schema: Value = serde_json::from_str(SCHEMA_JSON).map_err(|e| e.to_string())?;
            jsonschema::options()
                .offline()
                .build(&schema)
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| OssieError::Schema(e.clone()))
}

/// Validate against the complete pinned upstream schema, without network access.
///
/// In particular this rejects unknown versions, unknown object properties,
/// invalid dialects, and incorrectly shaped vendor extensions. Arbitrary AI
/// context keys and arbitrary vendor names are permitted by the standard.
pub fn validate_document(document: &Value) -> Result<(), OssieError> {
    let mut issues = Vec::new();
    for error in validator()?.iter_errors(document) {
        issues.push(ValidationIssue {
            instance_path: error.instance_path().to_string(),
            schema_path: error.schema_path().to_string(),
            message: error.to_string(),
        });
    }
    if issues.is_empty() {
        Ok(())
    } else {
        Err(OssieError::Validation(issues))
    }
}

/// Parse JSON or YAML and validate before returning the complete document.
pub fn parse_document(text: &str, format: DocumentFormat) -> Result<Value, OssieError> {
    let format = match format {
        DocumentFormat::Auto if text.trim_start().starts_with(['{', '[']) => DocumentFormat::Json,
        DocumentFormat::Auto => DocumentFormat::Yaml,
        explicit => explicit,
    };
    let document = match format {
        DocumentFormat::Json => {
            serde_json::from_str(text).map_err(|e| OssieError::Parse(format!("JSON: {e}")))?
        }
        DocumentFormat::Yaml | DocumentFormat::Auto => {
            serde_yaml_ng::from_str(text).map_err(|e| OssieError::Parse(format!("YAML: {e}")))?
        }
    };
    validate_document(&document)?;
    Ok(document)
}

/// Validate and pretty-print the full document as JSON.
pub fn to_json(document: &Value) -> Result<String, OssieError> {
    validate_document(document)?;
    serde_json::to_string_pretty(document).map_err(|e| OssieError::Serialization(e.to_string()))
}

/// Validate and serialize the full document as YAML.
pub fn to_yaml(document: &Value) -> Result<String, OssieError> {
    validate_document(document)?;
    serde_yaml_ng::to_string(document).map_err(|e| OssieError::Serialization(e.to_string()))
}

/// Parse and validate a typed document without discarding standard fields.
pub fn parse_model(text: &str, format: DocumentFormat) -> Result<SemanticModel, OssieError> {
    SemanticModel::from_value(parse_document(text, format)?)
}

impl SemanticModel {
    pub fn from_value(document: Value) -> Result<Self, OssieError> {
        validate_document(&document)?;
        serde_json::from_value(document).map_err(|e| OssieError::Parse(e.to_string()))
    }

    pub fn to_value(&self) -> Result<Value, OssieError> {
        let document =
            serde_json::to_value(self).map_err(|e| OssieError::Serialization(e.to_string()))?;
        validate_document(&document)?;
        Ok(document)
    }

    pub fn validate(&self) -> Result<(), OssieError> {
        self.to_value().map(|_| ())
    }

    pub fn to_json(&self) -> Result<String, OssieError> {
        to_json(&self.to_value()?)
    }

    pub fn to_yaml(&self) -> Result<String, OssieError> {
        to_yaml(&self.to_value()?)
    }
}
