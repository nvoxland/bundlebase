//! CreateSource command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::command::parser::extract_string_content;
use crate::bundle::operation::{AttachBlockOp, CreateSourceOp, SourceInfo};
use crate::data::ObjectId;
use crate::source::FetchAction;
use crate::BundlebaseError;
use async_trait::async_trait;
use std::collections::HashMap;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to create a data source for a pack.
#[derive(Debug, Clone)]
pub struct CreateSourceCommand {
    /// The source function name (e.g., "remote_dir")
    pub function: String,
    /// Function-specific arguments
    pub args: HashMap<String, String>,
    /// The pack to create the source for (None or "base" for base pack)
    pub pack: Option<String>,
}

impl CreateSourceCommand {
    /// Create a new CreateSourceCommand.
    pub fn new(
        function: impl Into<String>,
        args: HashMap<String, String>,
        pack: Option<String>,
    ) -> Self {
        Self {
            function: function.into(),
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
        let mut function = None;
        let mut args = HashMap::new();
        let mut pack = None;
        let mut seen_source_args = false;

        for inner_pair in pair.into_inner() {
            match inner_pair.as_rule() {
                Rule::identifier => {
                    if function.is_none() {
                        // First identifier is the function name
                        function = Some(inner_pair.as_str().to_string());
                    } else if seen_source_args {
                        // Identifier after source_args is the pack name (after ON)
                        pack = Some(inner_pair.as_str().to_string());
                    }
                }
                Rule::source_args => {
                    seen_source_args = true;
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

        let function = function.ok_or_else(|| -> BundlebaseError {
            "CREATE SOURCE missing function name".into()
        })?;

        if args.is_empty() {
            return Err("CREATE SOURCE requires at least one argument in WITH clause".into());
        }

        Ok(CreateSourceCommand::new(function, args, pack))
    }

    fn to_statement(&self) -> String {
        use crate::bundle::command::parser::escape_string;
        let mut args_str: Vec<String> = self
            .args
            .iter()
            .map(|(k, v)| format!("{} = {}", k, escape_string(v)))
            .collect();
        args_str.sort(); // Consistent ordering
        let args_joined = args_str.join(", ");
        match &self.pack {
            Some(pack) if pack != "base" => {
                format!("CREATE SOURCE {} WITH ({}) ON {}", self.function, args_joined, pack)
            }
            _ => format!("CREATE SOURCE {} WITH ({})", self.function, args_joined),
        }
    }
}

#[async_trait]
impl BundleBuilderCommand for CreateSourceCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let pack_id = builder.resolve_pack_id(self.pack.as_deref())?;
        let source_id = ObjectId::generate();
        let function = self.function.clone();
        let op = CreateSourceOp::setup(source_id, pack_id, self.function.clone(), self.args.clone());

        builder.apply_operation(op.into()).await?;

        // Automatically fetch from the newly created source
        let source = builder
            .bundle()
            .get_source(&source_id)
            .ok_or_else(|| format!("Source '{}' not found after creation", source_id))?;

        let actions = source.fetch(builder).await?;

        // Process fetch actions
        for action in actions {
            match action {
                FetchAction::Add(data) => {
                    let mut op = AttachBlockOp::setup_for_source(
                        &pack_id,
                        &data.attach_location,
                        &data.source_url,
                        &data.hash,
                        builder,
                    )
                    .await?;
                    op.source_info = Some(SourceInfo {
                        id: source_id,
                        location: data.source_location,
                        version: op.version.clone(),
                    });
                    builder.apply_operation(op.into()).await?;
                }
                FetchAction::Replace { .. } | FetchAction::Remove { .. } => {
                    // These shouldn't happen on initial source creation
                }
            }
        }

        Ok(format!("Created source: {}", function))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_create_source_simple() {
        let input = "CREATE SOURCE remote_dir WITH (url = 's3://bucket/data/')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateSource(c) => {
                assert_eq!(c.function, "remote_dir");
                assert_eq!(c.args.get("url"), Some(&"s3://bucket/data/".to_string()));
                assert_eq!(c.pack, None);
            }
            _ => panic!("Expected CreateSource variant"),
        }
    }

    #[test]
    fn test_parse_create_source_with_pack() {
        let input = "CREATE SOURCE remote_dir WITH (url = 's3://bucket/users/') ON users";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateSource(c) => {
                assert_eq!(c.function, "remote_dir");
                assert_eq!(c.args.get("url"), Some(&"s3://bucket/users/".to_string()));
                assert_eq!(c.pack, Some("users".to_string()));
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

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::CreateSource(c) => {
                assert_eq!(c.function, "remote_dir");
                assert_eq!(c.args.get("url"), Some(&"file:///data/".to_string()));
            }
            _ => panic!("Expected CreateSource variant"),
        }
    }
}
