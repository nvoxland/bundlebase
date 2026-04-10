//! SetMinVersion command implementation.

use crate::{CommandParsing, Rule};
use crate::parser::{escape_string, extract_string_content};
use bundlebase::bundle::operation::SetMinVersionOp;
use bundlebase_common::BundlebaseError;
use crate::BundleBuilderCommand;
use bundlebase::BundleBuilder;

#[derive(Debug, Clone)]
pub struct SetMinVersionCommand {
    pub version: String,
}

impl CommandParsing for SetMinVersionCommand {
    fn rule() -> Rule {
        Rule::set_min_version_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut version = None;

        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::quoted_string {
                version = Some(extract_string_content(inner.as_str())?);
            }
        }

        let version = version.ok_or_else(|| -> BundlebaseError {
            "SET MIN VERSION missing version".into()
        })?;

        Ok(Self { version })
    }

    fn to_statement(&self) -> String {
        format!("SET MIN VERSION {}", escape_string(&self.version))
    }
}

impl BundleBuilderCommand for SetMinVersionCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        builder
            .apply_operation(SetMinVersionOp::setup(&self.version).into())
            .await?;
        Ok(format!("Set min version to {}", self.version))
    }
}
