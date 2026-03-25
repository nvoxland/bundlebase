//! CastColumn command implementation.

use crate::parser::{extract_identifier, quote_identifier};
use crate::parser::{escape_string, extract_string_content};
use crate::{CommandParsing, Rule};
use bundlebase_common::arrow_types::parse_arrow_type_name;
use bundlebase::bundle::operation::CastColumnOp;
use bundlebase::bundle::BundleFacade;
use bundlebase_common::BundlebaseError;
use crate::BundleBuilderCommand;
use bundlebase::BundleBuilder;

/// Command to cast a column to a different data type.
#[derive(Debug, Clone)]
pub struct CastColumnCommand {
    /// The column name to cast
    pub name: String,
    /// The target type (e.g., "integer", "float", "string")
    pub new_type: String,
    /// Optional regex pattern to clean the column values before casting
    pub clean: Option<String>,
}

impl CastColumnCommand {
    /// Create a new CastColumnCommand.
    pub fn new(
        name: impl Into<String>,
        new_type: impl Into<String>,
        clean: Option<String>,
    ) -> Self {
        Self {
            name: name.into(),
            new_type: new_type.into(),
            clean,
        }
    }
}

impl CommandParsing for CastColumnCommand {
    fn rule() -> Rule {
        Rule::cast_column_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;
        let mut new_type = None;
        let mut clean = None;

        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::identifier => {
                    if name.is_none() {
                        name = Some(extract_identifier(&inner));
                    } else {
                        new_type = Some(inner.as_str().to_string());
                    }
                }
                Rule::quoted_string => {
                    clean = Some(extract_string_content(inner.as_str())?);
                }
                _ => {}
            }
        }

        let name = name.ok_or_else(|| -> BundlebaseError {
            "CAST COLUMN statement missing column name".into()
        })?;
        let new_type = new_type.ok_or_else(|| -> BundlebaseError {
            "CAST COLUMN statement missing target type".into()
        })?;

        Ok(CastColumnCommand::new(name, new_type, clean))
    }

    fn to_statement(&self) -> String {
        match &self.clean {
            Some(pattern) => format!(
                "CAST COLUMN {} TO {} CLEAN {}",
                quote_identifier(&self.name), self.new_type, escape_string(pattern)
            ),
            None => format!("CAST COLUMN {} TO {}", quote_identifier(&self.name), self.new_type),
        }
    }
}

impl BundleBuilderCommand for CastColumnCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let id = builder.column_id(&self.name)
            .ok_or_else(|| BundlebaseError::from(format!("Column '{}' not found", self.name)))?;

        let data_type = parse_arrow_type_name(&self.new_type)?;

        builder
            .apply_operation(
                CastColumnOp::setup(
                    id,
                    data_type,
                    self.clean.clone(),
                )
                .into(),
            )
            .await?;

        match &self.clean {
            Some(pattern) => Ok(format!(
                "Cast column {} to {} (clean: {})",
                self.name, self.new_type, pattern
            )),
            None => Ok(format!(
                "Cast column {} to {}",
                self.name, self.new_type
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::parse_command;
    use crate::{BundleCommand, CommandParsing};

    #[test]
    fn test_parse_cast_column() {
        let cmd = parse_command("CAST COLUMN price TO Int64").unwrap();
        match cmd {
            BundleCommand::CastColumn(c) => {
                assert_eq!(c.name, "price");
                assert_eq!(c.new_type, "Int64");
                assert_eq!(c.clean, None);
            }
            other => panic!("Expected CastColumn, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_cast_column_with_clean() {
        let cmd = parse_command("CAST COLUMN price TO Int64 CLEAN '[^0-9]'").unwrap();
        match cmd {
            BundleCommand::CastColumn(c) => {
                assert_eq!(c.name, "price");
                assert_eq!(c.new_type, "Int64");
                assert_eq!(c.clean, Some("[^0-9]".to_string()));
            }
            other => panic!("Expected CastColumn, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_cast_column_various_types() {
        for type_name in &["Float64", "Utf8", "Boolean", "Date32"] {
            let cmd = parse_command(&format!("CAST COLUMN value TO {}", type_name)).unwrap();
            match cmd {
                BundleCommand::CastColumn(c) => {
                    assert_eq!(c.new_type, *type_name);
                }
                other => panic!("Expected CastColumn, got {:?}", other),
            }
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = super::CastColumnCommand::new("price", "Int64", None);
        let statement = cmd.to_statement();
        assert_eq!(statement, "CAST COLUMN price TO Int64");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::CastColumn(c) => {
                assert_eq!(c.name, "price");
                assert_eq!(c.new_type, "Int64");
                assert_eq!(c.clean, None);
            }
            other => panic!("Expected CastColumn, got {:?}", other),
        }
    }

    #[test]
    fn test_round_trip_with_clean() {
        let cmd = super::CastColumnCommand::new("price", "Int64", Some("[^0-9]".to_string()));
        let statement = cmd.to_statement();

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::CastColumn(c) => {
                assert_eq!(c.name, "price");
                assert_eq!(c.new_type, "Int64");
                assert_eq!(c.clean, Some("[^0-9]".to_string()));
            }
            other => panic!("Expected CastColumn, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_quoted_column_name() {
        let cmd = parse_command(r#"CAST COLUMN "ResultMeasureValue" TO Float64"#).unwrap();
        match cmd {
            BundleCommand::CastColumn(c) => {
                assert_eq!(c.name, "ResultMeasureValue");
                assert_eq!(c.new_type, "Float64");
            }
            other => panic!("Expected CastColumn, got {:?}", other),
        }
    }

    #[test]
    fn test_round_trip_quoted() {
        let cmd = super::CastColumnCommand::new("column/with.special", "Utf8", None);
        let statement = cmd.to_statement();
        assert_eq!(statement, r#"CAST COLUMN "column/with.special" TO Utf8"#);

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::CastColumn(c) => {
                assert_eq!(c.name, "column/with.special");
                assert_eq!(c.new_type, "Utf8");
            }
            other => panic!("Expected CastColumn, got {:?}", other),
        }
    }
}
