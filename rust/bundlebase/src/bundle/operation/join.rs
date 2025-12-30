use crate::bundle::operation::Operation;
use crate::data::{ObjectId, PackJoin};
use crate::{Bundle, BundleBuilder, BundlebaseError};
use async_trait::async_trait;
use datafusion::common::DataFusionError;
use datafusion::prelude::JoinType;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JoinTypeOption {
    Inner,
    Left,
    Right,
    Full,
}

impl JoinTypeOption {
    pub fn to_datafusion(&self) -> JoinType {
        match self {
            JoinTypeOption::Inner => JoinType::Inner,
            JoinTypeOption::Left => JoinType::Left,
            JoinTypeOption::Right => JoinType::Right,
            JoinTypeOption::Full => JoinType::Full,
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "left" => JoinTypeOption::Left,
            "right" => JoinTypeOption::Right,
            "full" | "outer" => JoinTypeOption::Full,
            _ => JoinTypeOption::Inner,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            JoinTypeOption::Inner => "inner",
            JoinTypeOption::Left => "left",
            JoinTypeOption::Right => "right",
            JoinTypeOption::Full => "full",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinOp {
    pub name: String,
    pub pack_id: ObjectId,
    pub join_type: JoinTypeOption,
    pub expression: String,
}

impl PartialEq for JoinOp {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.pack_id == other.pack_id
            && self.join_type == other.join_type
            && self.expression == other.expression
            && self.pack_id == other.pack_id
    }
}

impl JoinOp {
    pub async fn setup(
        name: &str,
        pack_id: ObjectId,
        expression: &str,
        join_type: JoinTypeOption,
        _bundle: &BundleBuilder,
    ) -> Result<Self, BundlebaseError> {
        Ok(Self {
            name: name.to_string(),
            pack_id,
            expression: expression.to_string(),
            join_type,
        })
    }
}

#[async_trait]
impl Operation for JoinOp {
    fn describe(&self) -> String {
        format!(
            "JOIN {} on {} ({})",
            self.name,
            self.expression,
            self.join_type.as_str(),
        )
    }

    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        Ok(())
    }

    async fn apply(&self, bundle: &mut Bundle) -> Result<(), DataFusionError> {
        bundle.joins.insert(
            self.name.clone(),
            PackJoin::new(&self.pack_id, &self.name, &self.join_type, &self.expression),
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_describe() {
        let op = JoinOp {
            name: "test".to_string(),
            pack_id: ObjectId::from(1),
            join_type: JoinTypeOption::Inner,
            expression: "id = other.id".to_string(),
        };

        assert_eq!(op.describe(), "JOIN test on id = other.id (inner)");
    }

    #[test]
    fn test_join_type_from_str() {
        assert_eq!(JoinTypeOption::from_str("inner"), JoinTypeOption::Inner);
        assert_eq!(JoinTypeOption::from_str("left"), JoinTypeOption::Left);
        assert_eq!(JoinTypeOption::from_str("right"), JoinTypeOption::Right);
        assert_eq!(JoinTypeOption::from_str("full"), JoinTypeOption::Full);
        assert_eq!(JoinTypeOption::from_str("outer"), JoinTypeOption::Full);
        assert_eq!(JoinTypeOption::from_str("unknown"), JoinTypeOption::Inner);
    }

    #[test]
    fn test_join_type_as_str() {
        assert_eq!(JoinTypeOption::Inner.as_str(), "inner");
        assert_eq!(JoinTypeOption::Left.as_str(), "left");
        assert_eq!(JoinTypeOption::Right.as_str(), "right");
        assert_eq!(JoinTypeOption::Full.as_str(), "full");
    }

    #[test]
    fn test_join_type_to_datafusion() {
        assert_eq!(JoinTypeOption::Inner.to_datafusion(), JoinType::Inner);
        assert_eq!(JoinTypeOption::Left.to_datafusion(), JoinType::Left);
        assert_eq!(JoinTypeOption::Right.to_datafusion(), JoinType::Right);
        assert_eq!(JoinTypeOption::Full.to_datafusion(), JoinType::Full);
    }

    #[test]
    fn test_join_type_eq() {
        assert_eq!(JoinTypeOption::Inner, JoinTypeOption::Inner);
        assert_ne!(JoinTypeOption::Inner, JoinTypeOption::Left);
        assert_ne!(JoinTypeOption::Full, JoinTypeOption::Right);
    }

    #[tokio::test]
    async fn test_version() {
        let op = JoinOp {
            name: "test".to_string(),
            pack_id: ObjectId::from(3),
            join_type: JoinTypeOption::Inner,
            expression: "users.id = orders.user_id".to_string(),
        };

        let version = op.version();

        // Exact value for this specific config (first 12 chars of SHA256)
        assert_eq!(version, "3e6cd5d2e901");
    }
}
