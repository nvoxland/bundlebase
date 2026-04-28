//! Join command implementation.

use crate::parser::{extract_identifier, quote_identifier};
use crate::parser::{extract_string_content, parse_join_type};
use crate::BundleBuilderCommand;
use crate::{CommandParsing, Rule};
use bundlebase::bundle::operation::{AttachBlockOp, CreateJoinOp};
use bundlebase::bundle::BundleFacade;
use bundlebase::BundleBuilder;
use bundlebase::JoinTypeOption;
use bundlebase_common::BundlebaseError;
use bundlebase_data::ObjectId;

/// Command to join with another data source.
#[derive(Debug, Clone)]
pub struct JoinCommand {
    /// The name for the join
    pub name: String,
    /// The join expression
    pub expression: String,
    /// Optional location of data to attach to the join
    pub location: Option<String>,
    /// The type of join
    pub join_type: JoinTypeOption,
}

impl JoinCommand {
    /// Create a new JoinCommand.
    pub fn new(
        name: impl Into<String>,
        expression: impl Into<String>,
        location: Option<String>,
        join_type: JoinTypeOption,
    ) -> Self {
        Self {
            name: name.into(),
            expression: expression.into(),
            location,
            join_type,
        }
    }
}

impl CommandParsing for JoinCommand {
    fn rule() -> Rule {
        Rule::join_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut join_type = JoinTypeOption::Inner;
        let mut location = None;
        let mut name = None;
        let mut expression = None;

        for inner_pair in pair.into_inner() {
            match inner_pair.as_rule() {
                Rule::join_type => {
                    join_type = parse_join_type(inner_pair.as_str())?;
                }
                Rule::quoted_string => {
                    // First quoted string is the location file
                    if location.is_none() {
                        location = Some(extract_string_content(inner_pair.as_str())?);
                    }
                }
                Rule::identifier => {
                    // The AS name
                    name = Some(extract_identifier(&inner_pair));
                }
                Rule::join_condition => {
                    expression = Some(inner_pair.as_str().trim().to_string());
                }
                _ => {}
            }
        }

        let location = location
            .ok_or_else(|| -> BundlebaseError { "JOIN statement missing location file".into() })?;
        let name =
            name.ok_or_else(|| -> BundlebaseError { "JOIN statement missing AS name".into() })?;
        let expression = expression
            .ok_or_else(|| -> BundlebaseError { "JOIN statement missing ON expression".into() })?;

        if expression.is_empty() {
            return Err("JOIN ON expression cannot be empty".into());
        }

        Ok(JoinCommand::new(
            name,
            expression,
            Some(location),
            join_type,
        ))
    }

    fn to_statement(&self) -> String {
        use crate::parser::escape_string;
        let join_type_str = match self.join_type {
            JoinTypeOption::Inner => "",
            JoinTypeOption::Left => "LEFT ",
            JoinTypeOption::Right => "RIGHT ",
            JoinTypeOption::Full => "FULL OUTER ",
        };
        match &self.location {
            Some(loc) => format!(
                "{}JOIN {} AS {} ON {}",
                join_type_str,
                escape_string(loc),
                quote_identifier(&self.name),
                self.expression
            ),
            None => format!(
                "{}JOIN AS {} ON {}",
                join_type_str,
                quote_identifier(&self.name),
                self.expression
            ),
        }
    }
}

impl BundleBuilderCommand for JoinCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        // Translate user-visible column names in the join expression to stable internal name references.
        // Note: join expressions reference columns from both the base pack and the join pack.
        // At this point only base pack columns are known; join pack columns will be added
        // to the translation at dataframe_join time.
        let bundle_schema = builder.bundle_schema();
        let translated_expression = builder.translate_sql(&self.expression);

        // Step 1: Create a new pack with join metadata (stored with internal name names)
        let join_pack_id = ObjectId::generate();
        builder
            .apply_operation(
                CreateJoinOp::setup(
                    &join_pack_id,
                    &self.name,
                    &translated_expression,
                    self.join_type,
                )
                .await?
                .into(),
            )
            .await?;

        // Step 2: Attach the location data to the join pack (if provided)
        if let Some(ref loc) = self.location {
            let temp_reader = builder
                .bundle()
                .reader_factory
                .detect(loc, &bundlebase_data::BlockId::generate(), builder)
                .await?;
            let format = temp_reader.format();
            let op =
                AttachBlockOp::setup(&join_pack_id, loc, format, None, None, None, builder, None)
                    .await?;
            builder.apply_operation(op.into()).await?;
        }

        Ok(match &self.location {
            Some(loc) => format!("Joined with {} on {}", loc, self.expression),
            None => format!("Created join point \"{}\" (no initial data)", self.name),
        })
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;
    use crate::CommandParsing;

    #[test]
    fn test_parse_join_inner() {
        let input = "JOIN 'other.csv' AS other ON id = other.id";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Join(c) => {
                assert_eq!(c.name, "other");
                assert_eq!(c.location, Some("other.csv".to_string()));
                assert_eq!(c.expression, "id = other.id");
                assert_eq!(c.join_type, JoinTypeOption::Inner);
            }
            _ => panic!("Expected Join variant"),
        }
    }

    #[test]
    fn test_parse_join_left() {
        let input = "LEFT JOIN 'users.parquet' AS users ON user_id = users.id";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Join(c) => {
                assert_eq!(c.name, "users");
                assert_eq!(c.location, Some("users.parquet".to_string()));
                assert_eq!(c.expression, "user_id = users.id");
                assert_eq!(c.join_type, JoinTypeOption::Left);
            }
            _ => panic!("Expected Join variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = JoinCommand::new(
            "orders",
            "customer_id = orders.cust_id",
            Some("orders.parquet".to_string()),
            JoinTypeOption::Inner,
        );
        let statement = cmd.to_statement();
        assert_eq!(
            statement,
            "JOIN 'orders.parquet' AS orders ON customer_id = orders.cust_id"
        );

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::Join(c) => {
                assert_eq!(c.name, "orders");
                assert_eq!(c.location, Some("orders.parquet".to_string()));
                assert_eq!(c.expression, "customer_id = orders.cust_id");
                assert_eq!(c.join_type, JoinTypeOption::Inner);
            }
            _ => panic!("Expected Join variant"),
        }
    }

    #[test]
    fn test_round_trip_left_join() {
        let cmd = JoinCommand::new(
            "items",
            "item_id = items.id",
            Some("items.csv".to_string()),
            JoinTypeOption::Left,
        );
        let statement = cmd.to_statement();
        assert_eq!(
            statement,
            "LEFT JOIN 'items.csv' AS items ON item_id = items.id"
        );

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::Join(c) => {
                assert_eq!(c.name, "items");
                assert_eq!(c.join_type, JoinTypeOption::Left);
            }
            _ => panic!("Expected Join variant"),
        }
    }

    #[test]
    fn test_parse_quoted_alias() {
        let input = r#"JOIN 'data.csv' AS "my join" ON bundle.id = "my join".id"#;
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Join(c) => {
                assert_eq!(c.name, "my join");
                assert_eq!(c.location, Some("data.csv".to_string()));
            }
            _ => panic!("Expected Join variant"),
        }
    }
}
