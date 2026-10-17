//! Read and export the same semantic model used by native updates.

use crate::parser::{escape_string, extract_string_content};
use crate::{BundleFacadeCommand, CommandParsing, CommandResponse, OutputShape, Rule};
use arrow::datatypes::SchemaRef;
use bundlebase::BundleFacade;
use bundlebase_common::BundlebaseError;

#[derive(Debug, Clone)]
pub struct ShowSemanticModelCommand;

impl ShowSemanticModelCommand {
    pub fn output_schema() -> SchemaRef {
        String::schema()
    }
    pub fn output_shape() -> OutputShape {
        OutputShape::SingleValue
    }
}
impl CommandParsing for ShowSemanticModelCommand {
    fn rule() -> Rule {
        Rule::show_semantic_model_stmt
    }
    fn from_statement(_: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        Ok(Self)
    }
    fn to_statement(&self) -> String {
        "SHOW SEMANTIC MODEL".to_string()
    }
}
impl BundleFacadeCommand for ShowSemanticModelCommand {
    type Output = String;
    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<String, BundlebaseError> {
        Ok(serde_json::to_string_pretty(
            &facade.semantic_model().await?,
        )?)
    }
}

#[derive(Debug, Clone)]
pub struct ExportSemanticModelCommand {
    pub path: String,
}

impl ExportSemanticModelCommand {
    pub fn output_schema() -> SchemaRef {
        String::schema()
    }
    pub fn output_shape() -> OutputShape {
        OutputShape::SingleValue
    }
}
impl CommandParsing for ExportSemanticModelCommand {
    fn rule() -> Rule {
        Rule::export_semantic_model_stmt
    }
    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let path = pair
            .into_inner()
            .find(|p| p.as_rule() == Rule::quoted_string)
            .ok_or("Missing semantic model output path")?;
        Ok(Self {
            path: extract_string_content(path.as_str())?,
        })
    }
    fn to_statement(&self) -> String {
        format!("EXPORT SEMANTIC MODEL TO {}", escape_string(&self.path))
    }
}
impl BundleFacadeCommand for ExportSemanticModelCommand {
    type Output = String;
    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<String, BundlebaseError> {
        bundlebase::bundle::semantic_model::export_semantic_model(facade, &self.path).await?;
        Ok(format!("Exported semantic model to {}", self.path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parser::parse_command, BundleCommand};

    #[test]
    fn semantic_reads_are_facade_commands() -> Result<(), BundlebaseError> {
        let show = parse_command("SHOW SEMANTIC MODEL")?;
        assert!(show.is_facade_command());
        show.into_facade_command()?;
        let export = parse_command("EXPORT SEMANTIC MODEL TO 'model.yaml'")?;
        assert!(export.is_facade_command());
        if let BundleCommand::ExportSemanticModel(c) = export {
            assert_eq!(c.path, "model.yaml");
            parse_command(&c.to_statement())?.into_facade_command()?;
        } else {
            return Err("Expected export command".into());
        }
        Ok(())
    }
}
