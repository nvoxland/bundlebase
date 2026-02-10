use crate::bundle::operation::Operation;
use crate::bundle::pack::JoinTypeOption;
use crate::bundle::Pack;
use crate::io::ObjectId;
use crate::{Bundle, BundlebaseError};
use async_trait::async_trait;
use datafusion::error::DataFusionError;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CreateJoinOp {
    pub id: ObjectId,
    pub name: String,
    pub join_type: JoinTypeOption,
    pub expression: String,
}

impl CreateJoinOp {
    pub async fn setup(
        id: &ObjectId,
        name: &str,
        expression: &str,
        join_type: JoinTypeOption,
    ) -> Result<Self, BundlebaseError> {
        Ok(Self {
            id: *id,
            name: name.to_string(),
            join_type,
            expression: expression.to_string(),
        })
    }
}

#[async_trait]
impl Operation for CreateJoinOp {
    fn describe(&self) -> String {
        format!("CREATE JOIN '{}' ON {}", &self.name, &self.expression)
    }

    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        Ok(())
    }

    fn allowed_on_view(&self) -> bool {
        false
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        let pack = Arc::new(Pack::new(
            self.id,
            &self.name,
            &self.expression,
            self.join_type,
        ));

        bundle.add_pack(self.id, pack);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let op = CreateJoinOp {
            id: ObjectId::generate(),
            name: "customers".to_string(),
            join_type: JoinTypeOption::Left,
            expression: "base.id = customers.id".to_string(),
        };
        assert!(op.describe().contains("CREATE JOIN 'customers'"));
    }

    #[test]
    fn test_serialization() {
        let pack_id: ObjectId = "a5".try_into().unwrap();
        let op = CreateJoinOp {
            id: pack_id,
            name: "customers".to_string(),
            join_type: JoinTypeOption::Left,
            expression: "base.id = customers.id".to_string(),
        };

        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        assert!(serialized.contains("id: a5"));
        assert!(serialized.contains("name: customers"));
        assert!(serialized.contains("joinType: left"));
        assert!(serialized.contains("expression: base.id = customers.id"));
    }
}
