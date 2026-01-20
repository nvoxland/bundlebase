//! CreateFunction command implementation.

use crate::bundle::command::{Command, CommandContext};
use crate::bundle::operation::CreateFunctionOp;
use crate::functions::FunctionSignature;
use crate::BundlebaseError;
use async_trait::async_trait;

/// Command to create a custom function.
#[derive(Debug, Clone)]
pub struct CreateFunctionCommand {
    /// The function signature including name and output schema
    pub signature: FunctionSignature,
}

impl CreateFunctionCommand {
    /// Create a new CreateFunctionCommand.
    pub fn new(signature: FunctionSignature) -> Self {
        Self { signature }
    }
}

#[async_trait]
impl Command for CreateFunctionCommand {
    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        let op = CreateFunctionOp::setup(self.signature);
        ctx.apply_operation(op.into()).await?;
        Ok(())
    }

    fn to_statement(&self) -> String {
        // Basic representation - the full schema serialization is complex
        // and this is mainly used for change tracking/logging
        format!("CREATE FUNCTION {}", self.signature.name())
    }
}
