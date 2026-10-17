//! One resolved semantic state, derived from ordinary bundle operations.
use super::operation::{AnyOperation, SemanticTarget};
use crate::object_id::ColumnId;
use crate::BundlebaseError;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticState {
    pub model_ai_context: Option<Value>,
    pub dataset_description: Option<String>,
    pub dataset_ai_context: Option<Value>,
    pub columns: BTreeMap<ColumnId, ColumnMetadata>,
    pub metrics: BTreeMap<String, MetricMetadata>,
    pub primary_key: Option<Vec<ColumnId>>,
    pub unique_keys: Vec<Vec<ColumnId>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnMetadata {
    pub description: Option<String>,
    pub label: Option<String>,
    pub ai_context: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricMetadata {
    /// Internal SQL, using stable column and function references.
    pub expression: String,
    pub description: Option<String>,
    pub ai_context: Option<Value>,
}

/// Validate the pinned Ossie AI context shape. Extension members are permitted;
/// known members retain their schema types, including explicit null rejection.
pub fn validate_ai_context(context: &Value) -> Result<(), BundlebaseError> {
    let object = match context {
        Value::String(_) => return Ok(()),
        Value::Object(object) => object,
        _ => return Err("AI context must be a string or an object".into()),
    };
    for (member, value) in object {
        let valid = match member.as_str() {
            "instructions" => value.is_string(),
            "synonyms" | "examples" => value
                .as_array()
                .is_some_and(|values| values.iter().all(Value::is_string)),
            _ => true,
        };
        if !valid {
            let expected = if member == "instructions" {
                "a string"
            } else {
                "an array of strings"
            };
            return Err(format!("AI context member '{}' must be {}", member, expected).into());
        }
    }
    Ok(())
}

fn same_members(left: &[ColumnId], right: &[ColumnId]) -> bool {
    let left: BTreeSet<_> = left.iter().copied().collect();
    let right: BTreeSet<_> = right.iter().copied().collect();
    left == right
}

impl SemanticState {
    /// Replay in manifest order, retaining only current columns and complete
    /// live key declarations. Missing metric setters never create a metric.
    pub fn resolve(operations: &[AnyOperation], live_columns: &BTreeSet<ColumnId>) -> Self {
        let mut state = Self::default();
        for operation in operations {
            match operation {
                AnyOperation::DefineMetric(op) => {
                    let metric =
                        state
                            .metrics
                            .entry(op.name.clone())
                            .or_insert_with(|| MetricMetadata {
                                expression: op.expression.clone(),
                                description: None,
                                ai_context: None,
                            });
                    metric.expression = op.expression.clone();
                }
                AnyOperation::DropMetric(op) => {
                    state.metrics.remove(&op.name);
                }
                AnyOperation::SetMetricExpression(op) => {
                    if let Some(metric) = state.metrics.get_mut(&op.name) {
                        metric.expression = op.expression.clone();
                    }
                }
                AnyOperation::SetMetricDescription(op) => {
                    if let Some(metric) = state.metrics.get_mut(&op.name) {
                        metric.description = op.description.clone();
                    }
                }
                AnyOperation::DescribeColumn(op) => {
                    state.columns.entry(op.id).or_default().description =
                        Some(op.description.clone());
                }
                AnyOperation::ClearColumnDescription(op) => {
                    state.columns.entry(op.id).or_default().description = None;
                }
                AnyOperation::SetColumnLabel(op) => {
                    state.columns.entry(op.id).or_default().label = op.label.clone();
                }
                AnyOperation::SetDatasetDescription(op) => {
                    state.dataset_description = op.description.clone();
                }
                AnyOperation::SetAiContext(op) => {
                    if let Some(context) = state.context_mut(&op.target) {
                        *context = op.context.clone();
                    }
                }
                AnyOperation::SetAiContextMember(op) => {
                    if let Some(context) = state.context_mut(&op.target) {
                        match &op.value {
                            Some(value) => {
                                if !matches!(context, Some(Value::Object(_))) {
                                    *context = Some(Value::Object(Map::new()));
                                }
                                if let Some(Value::Object(object)) = context {
                                    object.insert(op.member.clone(), value.clone());
                                }
                            }
                            None => {
                                if let Some(Value::Object(object)) = context {
                                    object.remove(&op.member);
                                }
                            }
                        }
                    }
                }
                AnyOperation::DeclareKey(op) => {
                    if op.unique {
                        if !state
                            .unique_keys
                            .iter()
                            .any(|key| same_members(key, &op.columns))
                        {
                            state.unique_keys.push(op.columns.clone());
                        }
                    } else {
                        state.primary_key = Some(op.columns.clone());
                    }
                }
                AnyOperation::DropKey(op) => {
                    if op.unique {
                        state
                            .unique_keys
                            .retain(|key| !same_members(key, &op.columns));
                    } else if op.columns.is_empty()
                        || state
                            .primary_key
                            .as_ref()
                            .is_some_and(|key| key == &op.columns)
                    {
                        state.primary_key = None;
                    }
                }
                _ => {}
            }
        }
        state.columns.retain(|id, metadata| {
            live_columns.contains(id)
                && (metadata.description.is_some()
                    || metadata.label.is_some()
                    || metadata.ai_context.is_some())
        });
        if state
            .primary_key
            .as_ref()
            .is_some_and(|key| key.iter().any(|id| !live_columns.contains(id)))
        {
            state.primary_key = None;
        }
        state
            .unique_keys
            .retain(|key| key.iter().all(|id| live_columns.contains(id)));
        state
    }

    fn context_mut(&mut self, target: &SemanticTarget) -> Option<&mut Option<Value>> {
        match target {
            SemanticTarget::Model => Some(&mut self.model_ai_context),
            SemanticTarget::Dataset => Some(&mut self.dataset_ai_context),
            SemanticTarget::Column(id) => {
                Some(&mut self.columns.entry(*id).or_default().ai_context)
            }
            SemanticTarget::Metric(name) => self
                .metrics
                .get_mut(name)
                .map(|metric| &mut metric.ai_context),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::operation::*;
    use serde_json::json;

    fn resolve(operations: Vec<AnyOperation>, live: &[ColumnId]) -> SemanticState {
        SemanticState::resolve(&operations, &live.iter().copied().collect())
    }

    #[test]
    fn independent_metric_properties_survive_both_orders() {
        for reverse in [false, true] {
            let mut edits: Vec<AnyOperation> = vec![
                SetMetricExpressionOp::setup("sales", "SUM(col_sales) * 2").into(),
                SetMetricDescriptionOp::setup("sales", Some("net sales")).into(),
            ];
            if reverse {
                edits.reverse();
            }
            let mut operations = vec![DefineMetricOp::setup("sales", "SUM(col_sales)").into()];
            operations.extend(edits);
            let state = resolve(operations, &[]);
            assert_eq!(state.metrics["sales"].expression, "SUM(col_sales) * 2");
            assert_eq!(
                state.metrics["sales"].description.as_deref(),
                Some("net sales")
            );
        }
    }

    #[test]
    fn metric_definition_preserves_other_properties_and_drop_clears_them() {
        let target = SemanticTarget::Metric("n".to_string());
        let operations = vec![
            DefineMetricOp::setup("n", "COUNT(*)").into(),
            SetMetricDescriptionOp::setup("n", Some("rows")).into(),
            SetAiContextOp::setup(target.clone(), Some(json!("count all rows"))).into(),
            DefineMetricOp::setup("n", "COUNT(*) * 2").into(),
        ];
        let state = resolve(operations.clone(), &[]);
        assert_eq!(state.metrics["n"].description.as_deref(), Some("rows"));
        assert_eq!(state.metrics["n"].ai_context, Some(json!("count all rows")));
        let mut operations = operations;
        operations.extend([
            DropMetricOp::setup("n").into(),
            SetMetricExpressionOp::setup("n", "1").into(),
            SetMetricDescriptionOp::setup("n", Some("ignored")).into(),
            SetAiContextOp::setup(target, Some(json!("ignored"))).into(),
        ]);
        assert!(resolve(operations.clone(), &[]).metrics.is_empty());
        operations.push(DefineMetricOp::setup("n", "2").into());
        let state = resolve(operations, &[]);
        assert_eq!(state.metrics["n"].description, None);
        assert_eq!(state.metrics["n"].ai_context, None);
    }

    #[test]
    fn last_property_write_wins_in_either_order_and_clear_is_explicit() {
        let id = ColumnId::generate();
        let first: AnyOperation = DescribeColumnOp::setup(id, "first").into();
        let second: AnyOperation = DescribeColumnOp::setup(id, "second").into();
        assert_eq!(
            resolve(vec![first.clone(), second.clone()], &[id]).columns[&id]
                .description
                .as_deref(),
            Some("second")
        );
        assert_eq!(
            resolve(vec![second, first.clone()], &[id]).columns[&id]
                .description
                .as_deref(),
            Some("first")
        );
        assert!(!resolve(
            vec![first.clone(), ClearColumnDescriptionOp::setup(id).into()],
            &[id]
        )
        .columns
        .contains_key(&id));
        assert_eq!(
            resolve(
                vec![ClearColumnDescriptionOp::setup(id).into(), first],
                &[id]
            )
            .columns[&id]
                .description
                .as_deref(),
            Some("first")
        );
    }

    #[test]
    fn ai_member_replay_handles_replacements_strings_and_clears() {
        let target = SemanticTarget::Model;
        let operations = vec![
            SetAiContextOp::setup(target.clone(), Some(json!("old string"))).into(),
            SetAiContextMemberOp::setup(target.clone(), "synonyms", None).into(),
        ];
        assert_eq!(
            resolve(operations.clone(), &[]).model_ai_context,
            Some(json!("old string"))
        );
        let mut operations = operations;
        operations.push(
            SetAiContextMemberOp::setup(target.clone(), "synonyms", Some(json!(["orders"]))).into(),
        );
        assert_eq!(
            resolve(operations.clone(), &[]).model_ai_context,
            Some(json!({"synonyms":["orders"]}))
        );
        operations.push(
            SetAiContextOp::setup(target.clone(), Some(json!({"instructions":"new"}))).into(),
        );
        operations.push(
            SetAiContextMemberOp::setup(target.clone(), "examples", Some(json!(["monthly sales"])))
                .into(),
        );
        assert_eq!(
            resolve(operations.clone(), &[]).model_ai_context,
            Some(json!({"instructions":"new","examples":["monthly sales"]}))
        );
        operations.push(SetAiContextOp::setup(target.clone(), None).into());
        operations.push(SetAiContextMemberOp::setup(target, "custom", Some(Value::Null)).into());
        assert_eq!(
            resolve(operations, &[]).model_ai_context,
            Some(json!({"custom":null}))
        );
    }

    #[test]
    fn dropped_fields_and_incomplete_keys_are_omitted() {
        let first = ColumnId::generate();
        let second = ColumnId::generate();
        let operations = vec![
            DescribeColumnOp::setup(first, "gone").into(),
            SetColumnLabelOp::setup(second, Some("kept")).into(),
            DeclareKeyOp::setup(vec![first, second], false).into(),
            DeclareKeyOp::setup(vec![first], true).into(),
            DeclareKeyOp::setup(vec![second], true).into(),
        ];
        let state = resolve(operations, &[second]);
        assert!(!state.columns.contains_key(&first));
        assert_eq!(state.columns[&second].label.as_deref(), Some("kept"));
        assert_eq!(state.primary_key, None);
        assert_eq!(state.unique_keys, vec![vec![second]]);
    }

    #[test]
    fn unique_keys_match_member_sets_while_primary_order_is_preserved() {
        let first = ColumnId::generate();
        let second = ColumnId::generate();
        let operations = vec![
            DeclareKeyOp::setup(vec![second, first], true).into(),
            DeclareKeyOp::setup(vec![first, second], true).into(),
            DeclareKeyOp::setup(vec![second, first], false).into(),
            DropKeyOp::setup(vec![first, second], false).into(),
        ];
        let state = resolve(operations.clone(), &[first, second]);
        assert_eq!(state.unique_keys, vec![vec![second, first]]);
        assert_eq!(state.primary_key, Some(vec![second, first]));
        let mut operations = operations;
        operations.push(DropKeyOp::setup(vec![first, second], true).into());
        assert!(resolve(operations, &[first, second]).unique_keys.is_empty());
    }

    #[test]
    fn context_schema_known_types_and_extensions() {
        for valid in [
            json!("plain"),
            json!({"instructions":"do this","synonyms":["a"],"examples":[],"custom":null}),
        ] {
            assert!(validate_ai_context(&valid).is_ok());
        }
        for invalid in [
            Value::Null,
            json!(1),
            json!([]),
            json!({"instructions":null}),
            json!({"synonyms":[1]}),
            json!({"examples":"bad"}),
        ] {
            assert!(validate_ai_context(&invalid).is_err());
        }
    }

    #[test]
    fn ordinary_operation_serialization_and_versions() {
        let id = ColumnId::generate();
        let operations: Vec<AnyOperation> = vec![
            SetMetricExpressionOp::setup("n", "COUNT(*)").into(),
            SetMetricDescriptionOp::setup("n", None).into(),
            SetColumnLabelOp::setup(id, Some("Revenue")).into(),
            ClearColumnDescriptionOp::setup(id).into(),
            SetAiContextOp::setup(SemanticTarget::Column(id), Some(json!("help"))).into(),
            SetAiContextMemberOp::setup(SemanticTarget::Dataset, "synonyms", None).into(),
            SetDatasetDescriptionOp::setup(None).into(),
            DeclareKeyOp::setup(vec![id], false).into(),
            DropKeyOp::setup(vec![id], true).into(),
            ClearDescriptionOp::setup().into(),
        ];
        for operation in operations {
            let yaml = serde_yaml_ng::to_string(&operation).expect("serialize operation");
            let roundtrip: AnyOperation =
                serde_yaml_ng::from_str(&yaml).expect("deserialize operation");
            assert_eq!(operation, roundtrip);
            assert_eq!(operation.version(), roundtrip.version());
            assert_eq!(operation.version().len(), 12);
        }
    }

    #[test]
    fn replay_checks_shape_and_does_not_require_live_targets() {
        let removed = ColumnId::generate();
        for operation in [
            SetMetricExpressionOp::setup("removed", "invalid_after_other_schema_changes").into(),
            SetMetricDescriptionOp::setup("removed", None).into(),
            SetColumnLabelOp::setup(removed, None).into(),
        ] {
            assert!(super::super::operation::check_semantic_replay(&operation)
                .expect("semantic operation")
                .is_ok());
        }
        let invalid =
            SetAiContextOp::setup(SemanticTarget::Model, Some(json!({"instructions":null}))).into();
        assert!(super::super::operation::check_semantic_replay(&invalid)
            .expect("semantic operation")
            .is_err());
    }

    #[tokio::test]
    async fn local_edits_validate_targets_and_values() -> Result<(), BundlebaseError> {
        let bundle = crate::Bundle::empty(None).await?;
        let id = ColumnId::generate();
        assert!(SetColumnLabelOp::setup(id, Some("missing"))
            .check(&bundle)
            .await
            .is_err());
        assert!(SetMetricDescriptionOp::setup("missing", Some("missing"))
            .check(&bundle)
            .await
            .is_err());
        assert!(DeclareKeyOp::setup(vec![id, id], true)
            .check(&bundle)
            .await
            .is_err());
        assert!(SetAiContextOp::setup(
            SemanticTarget::Model,
            Some(serde_json::json!({"synonyms": null}))
        )
        .check(&bundle)
        .await
        .is_err());
        assert!(DropKeyOp::setup(vec![], false).check(&bundle).await.is_ok());
        assert!(DropKeyOp::setup(vec![], true).check(&bundle).await.is_err());
        bundle
            .apply_operation(DefineMetricOp::setup("n", "COUNT(*)").into())
            .await?;
        assert!(SetMetricExpressionOp::setup("n", "COUNT(*) * 2")
            .check(&bundle)
            .await
            .is_ok());
        assert!(SetMetricExpressionOp::setup("n", "unknown_function(*)")
            .check(&bundle)
            .await
            .is_err());
        Ok(())
    }
}
