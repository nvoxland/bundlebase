//! CreateSource command implementation.

use crate::{CommandParsing, Rule};
use crate::parser::extract_string_content;
use bundlebase::bundle::operation::{AttachBlockOp, CreateSourceOp, SourceInfo};
use bundlebase::source::{FetchAction, SyncMode};
use bundlebase_data::ObjectId;
use bundlebase_common::BundlebaseError;
use std::collections::HashMap;
use crate::BundleBuilderCommand;
use bundlebase::BundleBuilder;

/// Command to create a data source for a pack.
#[derive(Debug, Clone)]
pub struct CreateSourceCommand {
    /// The connector name (e.g., "remote_dir" for built-in, "acme.weather" for custom)
    pub connector: String,
    /// Connector-specific arguments
    pub args: HashMap<String, String>,
    /// The pack to create the source for (None or "base" for base pack)
    pub pack: Option<String>,
}

impl CreateSourceCommand {
    /// Create a new CreateSourceCommand.
    pub fn new(
        connector: impl Into<String>,
        args: HashMap<String, String>,
        pack: Option<String>,
    ) -> Self {
        Self {
            connector: connector.into(),
            args,
            pack,
        }
    }
}

impl CommandParsing for CreateSourceCommand {
    fn rule() -> Rule {
        Rule::create_source_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut args = HashMap::new();
        let mut identifiers: Vec<String> = Vec::new();
        let mut has_dotted = false;
        let mut dotted_name = None;

        for inner_pair in pair.into_inner() {
            match inner_pair.as_rule() {
                Rule::dotted_identifier => {
                    // Dotted identifier is always the connector (after USING)
                    dotted_name = Some(inner_pair.as_str().to_string());
                    has_dotted = true;
                }
                Rule::identifier => {
                    identifiers.push(inner_pair.as_str().to_string());
                }
                Rule::source_args => {
                    for arg_pair in inner_pair.into_inner() {
                        if arg_pair.as_rule() == Rule::source_arg_pair {
                            let mut key = None;
                            let mut value = None;
                            for part in arg_pair.into_inner() {
                                match part.as_rule() {
                                    Rule::identifier => {
                                        key = Some(part.as_str().to_string());
                                    }
                                    Rule::quoted_string => {
                                        value = Some(extract_string_content(part.as_str())?);
                                    }
                                    _ => {}
                                }
                            }
                            if let (Some(k), Some(v)) = (key, value) {
                                args.insert(k, v);
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // TODO: why this complexity?
        // Resolve connector and pack from collected identifiers.
        // Grammar: CREATE SOURCE [FOR <pack>] USING <connector> [WITH ...]
        // If a dotted_identifier was found, that's always the connector.
        // Plain identifiers: if dotted connector exists, all plain ids are the pack (FOR clause).
        // If no dotted connector: last plain id is the connector, preceding ones are the pack.
        let (connector, pack) = if has_dotted {
            let pack = identifiers.into_iter().next(); // At most one pack from FOR clause
            (dotted_name.ok_or_else(|| BundlebaseError::from("CREATE SOURCE missing connector name after USING"))?, pack)
        } else {
            match identifiers.len() {
                0 => return Err("CREATE SOURCE missing connector name after USING".into()),
                1 => (identifiers.into_iter().next().expect("checked len"), None),
                2 => {
                    let mut iter = identifiers.into_iter();
                    let pack = iter.next();
                    let connector = iter.next().expect("checked len");
                    (connector, pack)
                }
                _ => return Err("CREATE SOURCE: unexpected number of identifiers".into()),
            }
        };

        // Built-in connectors require args; custom connectors (dotted names) don't
        if args.is_empty() && !connector.contains('.') {
            return Err("CREATE SOURCE requires at least one argument in WITH clause".into());
        }

        Ok(CreateSourceCommand::new(connector, args, pack))
    }

    fn to_statement(&self) -> String {
        use crate::parser::escape_string;

        let pack_part = match &self.pack {
            Some(pack) if pack != "base" => format!(" FOR {}", pack),
            _ => String::new(),
        };

        if self.args.is_empty() {
            return format!("CREATE SOURCE{} USING {}", pack_part, self.connector);
        }

        let mut args_str: Vec<String> = self
            .args
            .iter()
            .map(|(k, v)| format!("{} = {}", k, escape_string(v)))
            .collect();
        args_str.sort(); // Consistent ordering
        let args_joined = args_str.join(", ");
        format!(
            "CREATE SOURCE{} USING {} WITH ({})",
            pack_part, self.connector, args_joined
        )
    }
}

impl BundleBuilderCommand for CreateSourceCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let pack_id = builder.resolve_pack_id(self.pack.as_deref())?;
        let source_id = ObjectId::generate();
        let connector_name = self.connector.clone();
        let op = CreateSourceOp::setup(source_id, pack_id, self.connector.clone(), self.args.clone());

        builder.apply_operation(op.into()).await?;

        // Automatically fetch from the newly created source
        let source = builder
            .bundle()
            .get_source(&source_id)
            .ok_or_else(|| format!("Source '{}' not found after creation", source_id))?;

        let actions = source.fetch(builder, SyncMode::Add).await?;

        // Process fetch actions
        for action in actions {
            match action {
                FetchAction::Add(data) => {
                    let op = AttachBlockOp::setup(
                        &pack_id,
                        &data.attach_location,
                        data.hash.as_deref(),
                        Some(SourceInfo {
                            id: source_id,
                            location: data.source_location,
                            version: data.version,
                        }),
                        builder,
                    )
                    .await?;
                    builder.apply_operation(op.into()).await?;
                }
                FetchAction::Replace { .. } | FetchAction::Remove { .. } => {
                    // These shouldn't happen on initial source creation
                }
            }
        }

        Ok(format!("Created source: {}", connector_name))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::CommandParsing;
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_create_source_simple() {
        let input = "CREATE SOURCE USING remote_dir WITH (url = 's3://bucket/data/')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateSource(c) => {
                assert_eq!(c.connector, "remote_dir");
                assert_eq!(c.args.get("url"), Some(&"s3://bucket/data/".to_string()));
                assert_eq!(c.pack, None);
            }
            _ => panic!("Expected CreateSource variant"),
        }
    }

    #[test]
    fn test_parse_create_source_with_pack() {
        let input =
            "CREATE SOURCE FOR users USING remote_dir WITH (url = 's3://bucket/users/')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateSource(c) => {
                assert_eq!(c.connector, "remote_dir");
                assert_eq!(c.args.get("url"), Some(&"s3://bucket/users/".to_string()));
                assert_eq!(c.pack, Some("users".to_string()));
            }
            _ => panic!("Expected CreateSource variant"),
        }
    }

    #[test]
    fn test_parse_create_source_dotted_connector() {
        let input = "CREATE SOURCE USING acme.weather";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateSource(c) => {
                assert_eq!(c.connector, "acme.weather");
                assert!(c.args.is_empty());
                assert_eq!(c.pack, None);
            }
            _ => panic!("Expected CreateSource variant"),
        }
    }

    #[test]
    fn test_parse_create_source_dotted_with_pack() {
        let input = "CREATE SOURCE FOR users USING acme.weather WITH (region = 'us-east')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateSource(c) => {
                assert_eq!(c.connector, "acme.weather");
                assert_eq!(c.args.get("region"), Some(&"us-east".to_string()));
                assert_eq!(c.pack, Some("users".to_string()));
            }
            _ => panic!("Expected CreateSource variant"),
        }
    }

    #[test]
    fn test_parse_create_source_case_insensitive() {
        let input = "create source using remote_dir with (url = 's3://bucket/')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateSource(c) => {
                assert_eq!(c.connector, "remote_dir");
            }
            _ => panic!("Expected CreateSource variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let mut args = HashMap::new();
        args.insert("url".to_string(), "file:///data/".to_string());
        let cmd = CreateSourceCommand::new("remote_dir", args, None);
        let statement = cmd.to_statement();
        assert_eq!(
            statement,
            "CREATE SOURCE USING remote_dir WITH (url = 'file:///data/')"
        );

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::CreateSource(c) => {
                assert_eq!(c.connector, "remote_dir");
                assert_eq!(c.args.get("url"), Some(&"file:///data/".to_string()));
            }
            _ => panic!("Expected CreateSource variant"),
        }
    }

    #[test]
    fn test_round_trip_with_pack() {
        let mut args = HashMap::new();
        args.insert("url".to_string(), "s3://bucket/".to_string());
        let cmd = CreateSourceCommand::new("remote_dir", args, Some("users".to_string()));
        let statement = cmd.to_statement();
        assert_eq!(
            statement,
            "CREATE SOURCE FOR users USING remote_dir WITH (url = 's3://bucket/')"
        );

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::CreateSource(c) => {
                assert_eq!(c.connector, "remote_dir");
                assert_eq!(c.pack, Some("users".to_string()));
            }
            _ => panic!("Expected CreateSource variant"),
        }
    }
}
