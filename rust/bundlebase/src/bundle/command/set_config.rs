//! SetConfig command implementation.

use crate::bundle::command::{Command, CommandContext};
use crate::bundle::command::parser::escape_string;
use crate::bundle::operation::SetConfigOp;
use crate::BundlebaseError;
use async_trait::async_trait;

/// Command to set a configuration value.
#[derive(Debug, Clone)]
pub struct SetConfigCommand {
    /// Configuration key
    pub key: String,
    /// Configuration value
    pub value: String,
    /// Optional URL prefix for URL-specific config
    pub url_prefix: Option<String>,
}

impl SetConfigCommand {
    /// Create a new SetConfigCommand.
    pub fn new(
        key: impl Into<String>,
        value: impl Into<String>,
        url_prefix: Option<String>,
    ) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
            url_prefix,
        }
    }
}

#[async_trait]
impl Command for SetConfigCommand {
    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        let op = SetConfigOp::setup(&self.key, &self.value, self.url_prefix.as_deref());
        ctx.apply_operation(op.into()).await?;
        Ok(())
    }

    fn to_statement(&self) -> String {
        match &self.url_prefix {
            Some(prefix) => format!(
                "SET CONFIG {} = {} FOR {}",
                self.key,
                escape_string(&self.value),
                escape_string(prefix)
            ),
            None => format!("SET CONFIG {} = {}", self.key, escape_string(&self.value)),
        }
    }
}
