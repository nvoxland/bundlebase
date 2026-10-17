use crate::bundle::BundleFacade;
use crate::object_id::ColumnId;
use crate::{AnyOperation, Bundle, BundlebaseError};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

/// Stable internal address for a semantic metadata owner.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
pub enum SemanticTarget {
    Model,
    Dataset,
    Column(ColumnId),
    Metric(String),
}

impl SemanticTarget {
    pub(crate) fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        match self {
            Self::Model | Self::Dataset => Ok(()),
            Self::Column(id) => check_column(bundle, id),
            Self::Metric(name) => check_metric(bundle, name),
        }
    }

    fn check_structure(&self) -> Result<(), BundlebaseError> {
        if let Self::Metric(name) = self {
            check_metric_name(name)?;
        }
        Ok(())
    }
}

pub(super) fn check_metric_name(name: &str) -> Result<(), BundlebaseError> {
    if name.trim().is_empty() {
        return Err("Metric name cannot be empty".into());
    }
    Ok(())
}

pub(super) fn check_expression(name: &str, expression: &str) -> Result<(), BundlebaseError> {
    check_metric_name(name)?;
    if expression.trim().is_empty() {
        return Err(format!("Metric '{}' has an empty expression", name).into());
    }
    // Parsing does not resolve data-dependent names. A concurrently removed
    // column must not prevent replay, but malformed SQL remains an error.
    use datafusion::sql::sqlparser::{dialect::GenericDialect, parser::Parser};
    let statements = Parser::parse_sql(
        &GenericDialect {},
        &format!("SELECT ({expression}) FROM bundle"),
    )
    .map_err(|e| BundlebaseError::from(format!("Invalid expression for metric '{name}': {e}")))?;
    if statements.len() != 1 {
        return Err(format!("Metric '{name}' requires a single SQL expression").into());
    }
    Ok(())
}

pub(super) fn check_metric(bundle: &Bundle, name: &str) -> Result<(), BundlebaseError> {
    check_metric_name(name)?;
    if !bundle.semantic_state().metrics.contains_key(name) {
        return Err(format!("Metric '{}' not found", name).into());
    }
    Ok(())
}

pub(super) fn check_column(bundle: &Bundle, id: &ColumnId) -> Result<(), BundlebaseError> {
    if bundle.column_name(id).is_none() {
        return Err(format!("Column with ID '{}' not found", id).into());
    }
    Ok(())
}

fn check_key_structure(columns: &[ColumnId]) -> Result<(), BundlebaseError> {
    if columns.is_empty() {
        return Err("A key must contain at least one column".into());
    }
    let mut seen = BTreeSet::new();
    for id in columns {
        if !seen.insert(id) {
            return Err(format!("Column '{}' appears more than once in a key", id).into());
        }
    }
    Ok(())
}

pub(super) fn check_key(bundle: &Bundle, columns: &[ColumnId]) -> Result<(), BundlebaseError> {
    check_key_structure(columns)?;
    for id in columns {
        check_column(bundle, id)?;
    }
    Ok(())
}

pub(super) fn check_context_member(
    member: &str,
    value: Option<&Value>,
) -> Result<(), BundlebaseError> {
    if let Some(value) = value {
        let mut object = Map::new();
        object.insert(member.to_string(), value.clone());
        crate::bundle::semantic_state::validate_ai_context(&Value::Object(object))?;
    }
    Ok(())
}

/// Persisted semantic operations validate their shape, without revalidating
/// targets or planning against a schema changed by another writer.
/// `None` leaves ordinary operation validation unchanged.
pub(crate) fn check_semantic_replay(op: &AnyOperation) -> Option<Result<(), BundlebaseError>> {
    Some(match op {
        AnyOperation::DefineMetric(op) => check_expression(&op.name, &op.expression),
        AnyOperation::SetMetricExpression(op) => check_expression(&op.name, &op.expression),
        AnyOperation::SetMetricDescription(op) => check_metric_name(&op.name),
        AnyOperation::DropMetric(op) => check_metric_name(&op.name),
        AnyOperation::DescribeColumn(_)
        | AnyOperation::ClearColumnDescription(_)
        | AnyOperation::SetColumnLabel(_)
        | AnyOperation::SetDatasetDescription(_) => Ok(()),
        AnyOperation::DeclareKey(op) => check_key_structure(&op.columns),
        AnyOperation::DropKey(op) => {
            if !op.unique && op.columns.is_empty() {
                Ok(())
            } else {
                check_key_structure(&op.columns)
            }
        }
        AnyOperation::SetAiContext(op) => {
            op.target
                .check_structure()
                .and_then(|()| match &op.context {
                    Some(context) => crate::bundle::semantic_state::validate_ai_context(context),
                    None => Ok(()),
                })
        }
        AnyOperation::SetAiContextMember(op) => op
            .target
            .check_structure()
            .and_then(|()| check_context_member(&op.member, op.value.as_ref())),
        _ => return None,
    })
}
