//! CreateSource command implementation.

use crate::parser::{extract_identifier, quote_identifier};
use crate::{CommandParsing, Rule};
use crate::parser::extract_string_content;
use bundlebase::bundle::operation::{AttachBlockOp, CreateSourceOp, SourceInfo};
use bundlebase::source::{FetchAction, SyncMode};
use bundlebase::{ExpectedColumn};
use bundlebase_data::attach_format::AttachFormat;
use bundlebase_data::ObjectId;
use bundlebase_common::BundlebaseError;
use bundlebase_common::arrow_types::parse_arrow_type_name;
use bundlebase_common::ColumnId;
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
    /// How to save fetched data (auto, copy, parquet, ref). None = auto.
    pub save_as: Option<String>,
    /// Optional size threshold in bytes for batching small files. None = no batching.
    pub batch_bytes: Option<usize>,
    /// Optional expected schema: list of (column_name, type_name) pairs.
    pub expected_schema: Option<Vec<(String, String)>>,
}

impl CreateSourceCommand {
    /// Create a new CreateSourceCommand.
    pub fn new(
        connector: impl Into<String>,
        mut args: HashMap<String, String>,
        pack: Option<String>,
    ) -> Self {
        // Extract save_as and batch_bytes from args (source-level options, not connector args).
        let save_as = args.remove("save_as");
        let batch_bytes = args
            .remove("batch_bytes")
            .and_then(|v| parse_size(&v))
            .or_else(|| default_batch_bytes(save_as.as_deref()));
        Self {
            connector: connector.into(),
            args,
            pack,
            save_as,
            batch_bytes,
            expected_schema: None,
        }
    }
}

/// Default batching threshold: 1GB. Many small files are a query-time
/// performance trap (each block adds per-file scan overhead), so we batch
/// by default whenever we're going to convert to parquet. SAVE AS COPY/REF
/// keep the original files intact and cannot be batched.
const DEFAULT_BATCH_BYTES: usize = 1024 * 1024 * 1024;

fn default_batch_bytes(save_as: Option<&str>) -> Option<usize> {
    match save_as.map(|s| s.to_lowercase()).as_deref() {
        None | Some("auto") | Some("parquet") => Some(DEFAULT_BATCH_BYTES),
        _ => None,
    }
}

/// Parse a size string like "10MB", "500KB", "1GB", or a plain number (bytes).
fn parse_size(s: &str) -> Option<usize> {
    let s = s.trim();
    let s_upper = s.to_uppercase();
    let (num_str, multiplier) = if let Some(n) = s_upper.strip_suffix("GB") {
        (n.trim(), 1_024 * 1_024 * 1_024)
    } else if let Some(n) = s_upper.strip_suffix("MB") {
        (n.trim(), 1_024 * 1_024)
    } else if let Some(n) = s_upper.strip_suffix("KB") {
        (n.trim(), 1_024)
    } else if let Some(n) = s_upper.strip_suffix("B") {
        (n.trim(), 1)
    } else {
        (s_upper.as_str(), 1)
    };
    num_str.trim().parse::<usize>().ok().map(|n| n * multiplier)
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
        let mut save_as = None;
        let mut expected_schema: Option<Vec<(String, String)>> = None;

        for inner_pair in pair.into_inner() {
            match inner_pair.as_rule() {
                Rule::dotted_identifier => {
                    dotted_name = Some(inner_pair.as_str().to_string());
                    has_dotted = true;
                }
                Rule::identifier => {
                    identifiers.push(extract_identifier(&inner_pair));
                }
                Rule::source_args => {
                    for arg_pair in inner_pair.into_inner() {
                        if arg_pair.as_rule() == Rule::source_arg_pair {
                            let mut key = None;
                            let mut value = None;
                            for part in arg_pair.into_inner() {
                                match part.as_rule() {
                                    Rule::identifier => {
                                        key = Some(extract_identifier(&part));
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
                Rule::save_as_value => {
                    save_as = Some(inner_pair.as_str().to_lowercase());
                }
                Rule::expected_schema_clause => {
                    let mut cols = Vec::new();
                    for col_pair in inner_pair.into_inner() {
                        if col_pair.as_rule() == Rule::expected_schema_column {
                            let mut parts = col_pair.into_inner();
                            if let (Some(name_part), Some(type_part)) = (parts.next(), parts.next()) {
                                cols.push((extract_identifier(&name_part), type_part.as_str().to_string()));
                            }
                        }
                    }
                    expected_schema = Some(cols);
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

        // Inject parser-extracted save_as into args so new() sees it when
        // computing the batch_bytes default. new() will remove it again.
        if let Some(ref s) = save_as {
            args.insert("save_as".to_string(), s.clone());
        }
        let mut cmd = CreateSourceCommand::new(connector, args, pack);
        cmd.expected_schema = expected_schema;
        Ok(cmd)
    }

    fn to_statement(&self) -> String {
        use crate::parser::escape_string;

        let pack_part = match &self.pack {
            Some(pack) if pack != "base" => format!(" FOR {}", quote_identifier(pack)),
            _ => String::new(),
        };

        let save_as_part = match &self.save_as {
            Some(s) => format!(" SAVE AS {}", s.to_uppercase()),
            None => String::new(),
        };

        let schema_part = match &self.expected_schema {
            Some(cols) if !cols.is_empty() => {
                let cols_str = cols
                    .iter()
                    .map(|(name, type_name)| format!("{} {}", quote_identifier(name), type_name.to_uppercase()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(" EXPECTED SCHEMA ({})", cols_str)
            }
            _ => String::new(),
        };

        if self.args.is_empty() {
            return format!("CREATE SOURCE{} USING {}{}{}", pack_part, self.connector, save_as_part, schema_part);
        }

        let mut args_str: Vec<String> = self
            .args
            .iter()
            .map(|(k, v)| format!("{} = {}", quote_identifier(k), escape_string(v)))
            .collect();
        args_str.sort();
        let args_joined = args_str.join(", ");
        format!(
            "CREATE SOURCE{} USING {} WITH ({}){}{}",
            pack_part, self.connector, args_joined, save_as_part, schema_part
        )
    }
}

impl BundleBuilderCommand for CreateSourceCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let pack_id = builder.resolve_pack_id(self.pack.as_deref())?;
        let source_id = ObjectId::generate();
        let connector_name = self.connector.clone();

        // Convert expected_schema from (name, type_name) pairs to ExpectedColumn list
        let expected_schema = self.expected_schema.as_ref()
            .map(|cols| {
                cols.iter()
                    .map(|(name, type_name)| {
                        parse_arrow_type_name(type_name)
                            .map(|data_type| ExpectedColumn {
                                id: ColumnId::generate(),
                                name: name.clone(),
                                data_type,
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?;

        let mut op = CreateSourceOp::setup(source_id, pack_id, self.connector.clone(), self.args.clone(), self.save_as.clone());
        op.expected_schema = expected_schema;
        op.batch_bytes = self.batch_bytes;

        builder.apply_operation(op.into()).await?;

        // Automatically fetch from the newly created source
        let source = builder
            .bundle()
            .get_source(&source_id)
            .ok_or_else(|| format!("Source '{}' not found after creation", source_id))?;

        let actions = source.fetch(builder, SyncMode::Add).await?;

        // Extract json_* args as reader-level read options (connector validation already skips them).
        let json_read_options = super::extract_json_opts(&self.args);

        // Prepare all AttachBlockOps first, then batch them if batch_size is configured
        let mut prepared_ops: Vec<(AttachBlockOp, String)> = Vec::new();
        for action in actions {
            match action {
                FetchAction::Add(data) => {
                    let (final_location, format, hash) = if let Some(ref opts) = json_read_options {
                        let (parquet_location, parquet_hash) = builder
                            .convert_json_attachment_to_parquet(&data.attach_location, opts)
                            .await?;
                        (parquet_location, AttachFormat::Parquet, Some(parquet_hash))
                    } else {
                        let temp_reader = builder.bundle().reader_factory
                            .detect(&data.attach_location, &bundlebase_data::BlockId::generate(), builder)
                            .await?;
                        (data.attach_location.clone(), temp_reader.format(), data.hash.clone())
                    };
                    let op = AttachBlockOp::setup(
                        &pack_id,
                        &final_location,
                        format,
                        hash.as_deref(),
                        Some(SourceInfo {
                            id: source_id,
                            location: data.source_location,
                            version: data.version,
                            batch_sources: None,
                        }),
                        None,
                        builder,
                    )
                    .await?;
                    let attach_location = op.location.clone();
                    prepared_ops.push((op, attach_location));
                }
                FetchAction::Replace { .. } | FetchAction::Remove { .. } => {
                    // These shouldn't happen on initial source creation
                }
            }
        }

        let final_ops = if let Some(batch_bytes) = self.batch_bytes {
            super::fetch::batch_small_ops_public(prepared_ops, batch_bytes, source_id, builder).await?
        } else {
            prepared_ops
        };

        let mut files_added = 0usize;
        let mut rows_added: Option<usize> = Some(0);
        for (op, _) in final_ops {
            files_added += 1;
            rows_added = match (rows_added, op.num_rows) {
                (Some(acc), Some(n)) => Some(acc + n),
                _ => None,
            };
            builder.apply_operation(op.into()).await?;
        }

        if files_added == 0 {
            Ok(format!(
                "Created source: {}. No data fetched — check that the URL is accessible and the connector args are correct.",
                connector_name
            ))
        } else {
            match rows_added {
                Some(rows) => Ok(format!("Created source: {}. Fetched {} row(s).", connector_name, rows)),
                None => Ok(format!("Created source: {}. Fetched {} file(s).", connector_name, files_added)),
            }
        }
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
    fn test_default_batch_bytes_when_save_as_unset() {
        let mut args = HashMap::new();
        args.insert("url".to_string(), "s3://bucket/".to_string());
        let cmd = CreateSourceCommand::new("remote_dir", args, None);
        assert_eq!(cmd.batch_bytes, Some(1024 * 1024 * 1024));
    }

    #[test]
    fn test_default_batch_bytes_when_save_as_parquet() {
        let mut args = HashMap::new();
        args.insert("url".to_string(), "s3://bucket/".to_string());
        args.insert("save_as".to_string(), "parquet".to_string());
        let cmd = CreateSourceCommand::new("remote_dir", args, None);
        assert_eq!(cmd.batch_bytes, Some(1024 * 1024 * 1024));
    }

    #[test]
    fn test_no_default_batch_bytes_when_save_as_copy() {
        let mut args = HashMap::new();
        args.insert("url".to_string(), "s3://bucket/".to_string());
        args.insert("save_as".to_string(), "copy".to_string());
        let cmd = CreateSourceCommand::new("remote_dir", args, None);
        assert_eq!(cmd.batch_bytes, None);
    }

    #[test]
    fn test_no_default_batch_bytes_when_save_as_ref() {
        let mut args = HashMap::new();
        args.insert("url".to_string(), "s3://bucket/".to_string());
        args.insert("save_as".to_string(), "ref".to_string());
        let cmd = CreateSourceCommand::new("remote_dir", args, None);
        assert_eq!(cmd.batch_bytes, None);
    }

    #[test]
    fn test_user_batch_bytes_overrides_default() {
        let mut args = HashMap::new();
        args.insert("url".to_string(), "s3://bucket/".to_string());
        args.insert("batch_bytes".to_string(), "100MB".to_string());
        let cmd = CreateSourceCommand::new("remote_dir", args, None);
        assert_eq!(cmd.batch_bytes, Some(100 * 1024 * 1024));
    }

    #[test]
    fn test_default_applied_after_save_as_clause_in_parser() {
        let input = "CREATE SOURCE USING remote_dir WITH (url = 's3://bucket/') SAVE AS COPY";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateSource(c) => {
                assert_eq!(c.save_as, Some("copy".to_string()));
                assert_eq!(c.batch_bytes, None, "copy must not get default batching");
            }
            _ => panic!("Expected CreateSource variant"),
        }
    }

    #[test]
    fn test_default_applied_after_save_as_parquet_in_parser() {
        let input = "CREATE SOURCE USING remote_dir WITH (url = 's3://bucket/') SAVE AS PARQUET";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateSource(c) => {
                assert_eq!(c.batch_bytes, Some(1024 * 1024 * 1024));
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

    #[test]
    fn test_parse_save_as_clause() {
        let input = "CREATE SOURCE USING http WITH (url = 'https://example.com/data.csv') SAVE AS COPY";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateSource(c) => {
                assert_eq!(c.connector, "http");
                assert_eq!(c.save_as, Some("copy".to_string()));
            }
            _ => panic!("Expected CreateSource variant"),
        }
    }

    #[test]
    fn test_parse_save_as_parquet() {
        let input = "CREATE SOURCE USING http WITH (url = 'https://example.com/data.xlsx') SAVE AS PARQUET";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateSource(c) => {
                assert_eq!(c.save_as, Some("parquet".to_string()));
            }
            _ => panic!("Expected CreateSource variant"),
        }
    }

    #[test]
    fn test_parse_save_as_ref() {
        let input = "CREATE SOURCE USING http WITH (url = 'https://example.com/data.csv') SAVE AS REF";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateSource(c) => {
                assert_eq!(c.save_as, Some("ref".to_string()));
            }
            _ => panic!("Expected CreateSource variant"),
        }
    }

    #[test]
    fn test_parse_no_save_as_defaults_to_none() {
        let input = "CREATE SOURCE USING http WITH (url = 'https://example.com/data.csv')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateSource(c) => {
                assert_eq!(c.save_as, None);
            }
            _ => panic!("Expected CreateSource variant"),
        }
    }

    #[test]
    fn test_round_trip_with_save_as() {
        let mut args = HashMap::new();
        args.insert("url".to_string(), "https://example.com/data.csv".to_string());
        let mut cmd = CreateSourceCommand::new("http", args, None);
        cmd.save_as = Some("copy".to_string());
        let statement = cmd.to_statement();
        assert_eq!(
            statement,
            "CREATE SOURCE USING http WITH (url = 'https://example.com/data.csv') SAVE AS COPY"
        );

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::CreateSource(c) => {
                assert_eq!(c.save_as, Some("copy".to_string()));
            }
            _ => panic!("Expected CreateSource variant"),
        }
    }

    #[test]
    fn test_parse_dollar_quoted_arg_value() {
        // Dollar-quoted body arg containing JSON with double quotes and single quotes
        let input = r#"CREATE SOURCE USING http WITH (url = 'https://api.example.com/query', method = 'POST', body = $${"key": "it's a value"}$$)"#;
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateSource(c) => {
                assert_eq!(c.args.get("body").map(|s| s.as_str()), Some(r#"{"key": "it's a value"}"#));
            }
            _ => panic!("Expected CreateSource variant"),
        }
    }

    #[test]
    fn test_parse_dollar_quoted_multiline_body() {
        let input = "CREATE SOURCE USING http WITH (url = 'https://api.example.com/query', method = 'POST', body = $${\n  \"key\": \"value\"\n}$$)";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateSource(c) => {
                assert_eq!(c.args.get("body").map(|s| s.as_str()), Some("{\n  \"key\": \"value\"\n}"));
            }
            _ => panic!("Expected CreateSource variant"),
        }
    }
}
