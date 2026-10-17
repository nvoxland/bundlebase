use super::Operation;
use super::SemanticTarget;
use crate::{Bundle, BundlebaseError};
use datafusion::common::DataFusionError;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// A property-level semantic metadata operation, resolved from bundle history.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SetAiContextMemberOp {
    pub target: SemanticTarget,
    pub member: String,
    // Omission means removal; explicit JSON null is a permitted extension value.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_value"
    )]
    pub value: Option<Value>,
}

fn present_value<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

impl SetAiContextMemberOp {
    pub fn setup(target: SemanticTarget, member: &str, value: Option<Value>) -> Self {
        Self {
            target,
            member: member.to_string(),
            value,
        }
    }
}

impl Operation for SetAiContextMemberOp {
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        self.target.check(bundle)?;
        super::semantic_validation::check_context_member(&self.member, self.value.as_ref())
    }

    async fn apply(&self, _bundle: &Bundle) -> Result<(), DataFusionError> {
        Ok(())
    }

    fn describe(&self) -> String {
        format!(
            "SET AI CONTEXT MEMBER: {:?}.{} = {:?}",
            self.target, self.member, self.value
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::operation::AnyOperation;

    #[test]
    fn explicit_null_and_removal_have_distinct_persisted_forms() {
        let target = SemanticTarget::Model;
        let set: AnyOperation =
            SetAiContextMemberOp::setup(target.clone(), "custom", Some(Value::Null)).into();
        let remove: AnyOperation = SetAiContextMemberOp::setup(target, "custom", None).into();
        for operation in [&set, &remove] {
            let json = serde_json::to_string(operation).expect("serialize JSON");
            let back: AnyOperation = serde_json::from_str(&json).expect("deserialize JSON");
            assert_eq!(&back, operation);
            let yaml = serde_yaml_ng::to_string(operation).expect("serialize YAML");
            let back: AnyOperation = serde_yaml_ng::from_str(&yaml).expect("deserialize YAML");
            assert_eq!(&back, operation);
        }
        assert_ne!(set.version(), remove.version());
        assert_eq!(
            serde_json::to_value(&set).expect("serialize set")["value"],
            Value::Null
        );
        assert!(serde_json::to_value(&remove)
            .expect("serialize remove")
            .get("value")
            .is_none());
    }
}
