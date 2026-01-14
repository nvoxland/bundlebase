use crate::bundle::JoinTypeOption;
use crate::data::ObjectId;

/// Describes a join between base pack and another pack
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Join {
    pack: ObjectId,
    name: String,
    join_type: JoinTypeOption,
    expression: String,
}

impl Join {
    /// Create a new PackJoin
    pub fn new(
        pack: &ObjectId,
        name: &str,
        join_type: &JoinTypeOption,
        expression: &str,
    ) -> Self {
        Self {
            pack: pack.clone(),
            name: name.to_string(),
            join_type: join_type.clone(),
            expression: expression.to_string(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the pack ID
    pub fn pack(&self) -> &ObjectId {
        &self.pack
    }

    /// Get the join type
    pub fn join_type(&self) -> &JoinTypeOption {
        &self.join_type
    }

    /// Get the join expression
    pub fn expression(&self) -> &str {
        &self.expression
    }
}
