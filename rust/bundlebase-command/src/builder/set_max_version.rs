//! SetMaxVersion command implementation.

use crate::{CommandParsing, Rule};
use crate::parser::{escape_string, extract_string_content};
use bundlebase::bundle::operation::SetMaxVersionOp;
use bundlebase_common::BundlebaseError;
use crate::BundleBuilderCommand;
use bundlebase::BundleBuilder;

#[derive(Debug, Clone)]
pub struct SetMaxVersionCommand {
    pub version: String,
}

impl CommandParsing for SetMaxVersionCommand {
    fn rule() -> Rule {
        Rule::set_max_version_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut version = None;

        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::quoted_string {
                version = Some(extract_string_content(inner.as_str())?);
            }
        }

        let version = version.ok_or_else(|| -> BundlebaseError {
            "SET MAX VERSION missing version".into()
        })?;

        Ok(Self { version })
    }

    fn to_statement(&self) -> String {
        format!("SET MAX VERSION {}", escape_string(&self.version))
    }
}

impl BundleBuilderCommand for SetMaxVersionCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        builder
            .apply_operation(SetMaxVersionOp::setup(&self.version).into())
            .await?;
        Ok(format!("Set max version to {}", self.version))
    }
}
