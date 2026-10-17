//! Optional document adapter over native semantic metadata operations.

use crate::parser::{escape_string, extract_string_content};
use crate::{BundleBuilderCommand, CommandParsing, Rule};
use bundlebase::BundleBuilder;
use bundlebase_common::BundlebaseError;

#[derive(Debug, Clone)]
pub struct ImportSemanticModelCommand {
    pub path: String,
    pub replace: bool,
}

impl CommandParsing for ImportSemanticModelCommand {
    fn rule() -> Rule {
        Rule::import_semantic_model_stmt
    }
    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut path = None;
        let mut replace = false;
        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::quoted_string => path = Some(extract_string_content(inner.as_str())?),
                Rule::semantic_import_mode => {
                    replace = inner.as_str().eq_ignore_ascii_case("replace")
                }
                _ => {}
            }
        }
        Ok(Self {
            path: path.ok_or("Missing semantic model input path")?,
            replace,
        })
    }
    fn to_statement(&self) -> String {
        format!(
            "IMPORT SEMANTIC MODEL FROM {} {}",
            escape_string(&self.path),
            if self.replace { "REPLACE" } else { "MERGE" }
        )
    }
}

impl BundleBuilderCommand for ImportSemanticModelCommand {
    type Output = String;
    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        Box::pin(builder.import_semantic_model(&self.path, self.replace)).await?;
        Ok(format!("Imported semantic model from {}", self.path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parser::parse_command, BundleCommand};

    #[test]
    fn import_modes_and_round_trip() -> Result<(), BundlebaseError> {
        for (sql, replace) in [
            ("IMPORT SEMANTIC MODEL FROM 'model.yaml'", false),
            ("IMPORT SEMANTIC MODEL FROM 'model.json' MERGE", false),
            ("IMPORT SEMANTIC MODEL FROM 'model.yaml' REPLACE", true),
        ] {
            if let BundleCommand::ImportSemanticModel(c) = parse_command(sql)? {
                assert_eq!(c.replace, replace);
                parse_command(&c.to_statement())?;
            } else {
                return Err("Expected import command".into());
            }
        }
        Ok(())
    }
}
