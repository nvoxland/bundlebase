//! Commands for individual semantic properties. Each command records one operation.

use crate::parser::{escape_string, extract_identifier, extract_string_content, quote_identifier};
use crate::{BundleBuilderCommand, CommandParsing, Rule};
use bundlebase::bundle::operation::{
    AnyOperation, ClearColumnDescriptionOp, DescribeColumnOp, SemanticTarget, SetAiContextMemberOp,
    SetAiContextOp, SetColumnLabelOp, SetDatasetDescriptionOp, SetMetricDescriptionOp,
    SetMetricExpressionOp,
};
use bundlebase::{BundleBuilder, BundleFacade};
use bundlebase_common::BundlebaseError;
use serde_json::Value;

/// A property edit carried by an ALTER command. Omission and clearing are distinct.
#[derive(Debug, Clone, PartialEq)]
pub enum SemanticPropertyChange {
    Description(Option<String>),
    Label(Option<String>),
    Expression(String),
    AiContext {
        member: Option<String>,
        value: Option<Value>,
    },
}

impl SemanticPropertyChange {
    fn from_pair(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let setting = matches!(
            pair.as_rule(),
            Rule::set_column_property
                | Rule::set_metric_property
                | Rule::set_model_property
                | Rule::set_dataset_property
        );
        let mut property = None;
        let mut member = None;
        let mut value = None;
        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::semantic_description
                | Rule::semantic_label
                | Rule::semantic_expression
                | Rule::semantic_ai_context => property = Some(inner.as_rule()),
                Rule::semantic_member => {
                    member = inner.into_inner().next().map(|p| extract_identifier(&p));
                }
                Rule::quoted_string => value = Some(extract_string_content(inner.as_str())?),
                _ => {}
            }
        }
        if member.is_some() && property != Some(Rule::semantic_ai_context) {
            return Err("MEMBER is only supported for AI CONTEXT".into());
        }
        if setting && value.is_none() {
            return Err("SET requires a property value".into());
        }
        match property {
            Some(Rule::semantic_description) => Ok(Self::Description(value)),
            Some(Rule::semantic_label) => Ok(Self::Label(value)),
            Some(Rule::semantic_expression) => Ok(Self::Expression(
                value.ok_or("Expression cannot be cleared")?,
            )),
            Some(Rule::semantic_ai_context) => Ok(Self::AiContext {
                member,
                value: value.map(parse_ai_context).transpose()?,
            }),
            _ => Err("Missing semantic property".into()),
        }
    }

    fn to_clause(&self) -> String {
        match self {
            Self::Description(value) => text_clause("DESCRIPTION", value.as_deref()),
            Self::Label(value) => text_clause("LABEL", value.as_deref()),
            Self::Expression(value) => text_clause("EXPRESSION", Some(value)),
            Self::AiContext { member, value } => {
                let property = match member {
                    Some(member) => format!("AI CONTEXT MEMBER {}", quote_identifier(member)),
                    None => "AI CONTEXT".to_string(),
                };
                text_clause(&property, value.as_ref().map(Value::to_string).as_deref())
            }
        }
    }

    fn ai_operation(self, target: SemanticTarget) -> Result<AnyOperation, BundlebaseError> {
        match self {
            Self::AiContext {
                member: Some(member),
                value,
            } => Ok(SetAiContextMemberOp::setup(target, &member, value).into()),
            Self::AiContext {
                member: None,
                value,
            } => Ok(SetAiContextOp::setup(target, value).into()),
            _ => Err("Only AI CONTEXT is supported for this target".into()),
        }
    }
}

fn parse_ai_context(text: String) -> Result<Value, BundlebaseError> {
    match serde_json::from_str(&text) {
        Ok(value) => Ok(value),
        Err(error) if matches!(text.trim_start().chars().next(), Some('{' | '[' | '"')) => {
            Err(format!("Invalid JSON in AI CONTEXT: {error}").into())
        }
        Err(_) => Ok(Value::String(text)),
    }
}

fn text_clause(property: &str, value: Option<&str>) -> String {
    match value {
        Some(value) => format!("SET {} {}", property, escape_string(value)),
        None => format!("DROP {}", property),
    }
}

fn parse_alter(
    pair: pest::iterators::Pair<Rule>,
) -> Result<(Option<String>, SemanticPropertyChange), BundlebaseError> {
    let mut name = None;
    let mut change = None;
    for inner in pair.into_inner() {
        if inner.as_rule() == Rule::identifier {
            name = Some(extract_identifier(&inner));
        } else {
            change = Some(SemanticPropertyChange::from_pair(inner)?);
        }
    }
    Ok((name, change.ok_or("Missing semantic property change")?))
}

#[derive(Debug, Clone)]
pub struct AlterColumnCommand {
    pub name: String,
    pub change: SemanticPropertyChange,
}

impl CommandParsing for AlterColumnCommand {
    fn rule() -> Rule {
        Rule::alter_column_stmt
    }
    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let (name, change) = parse_alter(pair)?;
        Ok(Self {
            name: name.ok_or("Missing column name")?,
            change,
        })
    }
    fn to_statement(&self) -> String {
        format!(
            "ALTER COLUMN {} {}",
            quote_identifier(&self.name),
            self.change.to_clause()
        )
    }
}

impl BundleBuilderCommand for AlterColumnCommand {
    type Output = String;
    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let id = builder
            .column_id(&self.name)
            .ok_or_else(|| BundlebaseError::from(format!("Column '{}' not found", self.name)))?;
        let op: AnyOperation = match self.change {
            SemanticPropertyChange::Description(Some(value)) => {
                DescribeColumnOp::setup(id, &value).into()
            }
            SemanticPropertyChange::Description(None) => ClearColumnDescriptionOp::setup(id).into(),
            SemanticPropertyChange::Label(value) => {
                SetColumnLabelOp::setup(id, value.as_deref()).into()
            }
            change => change.ai_operation(SemanticTarget::Column(id))?,
        };
        builder.apply_operation(op).await?;
        Ok(format!("Updated column semantics: {}", self.name))
    }
}

#[derive(Debug, Clone)]
pub struct AlterMetricCommand {
    pub name: String,
    pub change: SemanticPropertyChange,
}

impl CommandParsing for AlterMetricCommand {
    fn rule() -> Rule {
        Rule::alter_metric_stmt
    }
    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let (name, change) = parse_alter(pair)?;
        Ok(Self {
            name: name.ok_or("Missing metric name")?,
            change,
        })
    }
    fn to_statement(&self) -> String {
        format!(
            "ALTER METRIC {} {}",
            quote_identifier(&self.name),
            self.change.to_clause()
        )
    }
}

impl BundleBuilderCommand for AlterMetricCommand {
    type Output = String;
    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let op: AnyOperation = match self.change {
            SemanticPropertyChange::Expression(expression) => {
                SetMetricExpressionOp::setup(&self.name, &builder.translate_sql(&expression)).into()
            }
            SemanticPropertyChange::Description(value) => {
                SetMetricDescriptionOp::setup(&self.name, value.as_deref()).into()
            }
            change => change.ai_operation(SemanticTarget::Metric(self.name.clone()))?,
        };
        builder.apply_operation(op).await?;
        Ok(format!("Updated metric semantics: {}", self.name))
    }
}

#[derive(Debug, Clone)]
pub struct AlterSemanticModelCommand {
    pub change: SemanticPropertyChange,
}

impl CommandParsing for AlterSemanticModelCommand {
    fn rule() -> Rule {
        Rule::alter_semantic_model_stmt
    }
    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        Ok(Self {
            change: parse_alter(pair)?.1,
        })
    }
    fn to_statement(&self) -> String {
        format!("ALTER SEMANTIC MODEL {}", self.change.to_clause())
    }
}

impl BundleBuilderCommand for AlterSemanticModelCommand {
    type Output = String;
    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let operation = match self.change {
            SemanticPropertyChange::Description(Some(value)) => {
                bundlebase::bundle::operation::SetDescriptionOp::setup(&value).into()
            }
            SemanticPropertyChange::Description(None) => {
                bundlebase::bundle::operation::ClearDescriptionOp::setup().into()
            }
            change => change.ai_operation(SemanticTarget::Model)?,
        };
        builder.apply_operation(operation).await?;
        Ok("Updated semantic model".to_string())
    }
}

#[derive(Debug, Clone)]
pub struct AlterDatasetCommand {
    pub change: SemanticPropertyChange,
}

impl CommandParsing for AlterDatasetCommand {
    fn rule() -> Rule {
        Rule::alter_dataset_stmt
    }
    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        Ok(Self {
            change: parse_alter(pair)?.1,
        })
    }
    fn to_statement(&self) -> String {
        format!("ALTER DATASET bundle {}", self.change.to_clause())
    }
}

impl BundleBuilderCommand for AlterDatasetCommand {
    type Output = String;
    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let op = match self.change {
            SemanticPropertyChange::Description(value) => {
                SetDatasetDescriptionOp::setup(value.as_deref()).into()
            }
            change => change.ai_operation(SemanticTarget::Dataset)?,
        };
        builder.apply_operation(op).await?;
        Ok("Updated dataset semantics: bundle".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parser::parse_command, BundleCommand};

    #[test]
    fn semantic_alter_round_trips() -> Result<(), BundlebaseError> {
        for sql in [
            "ALTER COLUMN amount SET DESCRIPTION 'Gross amount'",
            "ALTER COLUMN amount DROP DESCRIPTION",
            "ALTER COLUMN amount SET LABEL 'Revenue'",
            "ALTER COLUMN amount DROP LABEL",
            "ALTER COLUMN amount SET AI CONTEXT '{\"unit\":\"USD\"}'",
            "ALTER COLUMN amount DROP AI CONTEXT MEMBER unit",
            "ALTER METRIC revenue SET EXPRESSION 'SUM(amount)'",
            "ALTER METRIC revenue SET DESCRIPTION 'Gross revenue'",
            "ALTER METRIC revenue DROP DESCRIPTION",
            "ALTER METRIC revenue SET AI CONTEXT MEMBER usage 'Always aggregate'",
            "ALTER METRIC revenue DROP AI CONTEXT",
            "ALTER SEMANTIC MODEL SET AI CONTEXT '{\"purpose\":\"Sales\"}'",
            "ALTER SEMANTIC MODEL DROP AI CONTEXT MEMBER purpose",
            "ALTER DATASET bundle SET DESCRIPTION 'Orders'",
            "ALTER DATASET bundle DROP AI CONTEXT",
        ] {
            let statement = match parse_command(sql)? {
                BundleCommand::AlterColumn(c) => c.to_statement(),
                BundleCommand::AlterMetric(c) => c.to_statement(),
                BundleCommand::AlterSemanticModel(c) => c.to_statement(),
                BundleCommand::AlterDataset(c) => c.to_statement(),
                _ => return Err(format!("Unexpected command: {}", sql).into()),
            };
            parse_command(&statement)?;
        }
        Ok(())
    }

    #[test]
    fn ai_context_rejects_malformed_json_looking_values() {
        for sql in [
            "ALTER SEMANTIC MODEL SET AI CONTEXT $${broken}$$",
            "ALTER COLUMN salary SET AI CONTEXT $$[broken]$$",
            "ALTER METRIC payroll SET AI CONTEXT MEMBER examples $$[\"missing close\"$$",
            "ALTER SEMANTIC MODEL SET AI CONTEXT $$\"unterminated$$",
        ] {
            assert!(parse_command(sql).is_err(), "Must reject {sql}");
        }
    }

    #[test]
    fn ai_context_preserves_json_and_plain_strings() -> Result<(), BundlebaseError> {
        if let BundleCommand::AlterMetric(c) =
            parse_command("ALTER METRIC revenue SET AI CONTEXT MEMBER hint 'Always aggregate'")?
        {
            assert_eq!(
                c.change,
                SemanticPropertyChange::AiContext {
                    member: Some("hint".to_string()),
                    value: Some(Value::String("Always aggregate".to_string()))
                }
            );
        } else {
            return Err("Expected ALTER METRIC".into());
        }
        assert!(parse_command("ALTER COLUMN amount SET DESCRIPTION MEMBER unit 'USD'").is_err());
        assert!(parse_command("ALTER METRIC revenue DROP EXPRESSION").is_err());
        Ok(())
    }
}
