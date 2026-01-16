use crate::bundle::command::BundleCommand;
use crate::bundle::pack::JoinTypeOption;
use crate::BundlebaseError;
use pest::Parser;
use pest_derive::Parser;
use std::collections::HashMap;

#[derive(Parser)]
#[grammar = "bundle/command/commands.pest"]
pub struct BundlebaseParser;

/// Parse custom bundlebase syntax using Pest grammar
pub fn parse_custom_pest(sql: &str) -> Result<Option<BundleCommand>, BundlebaseError> {
    // Try to parse with Pest grammar
    let parse_result = BundlebaseParser::parse(Rule::statement, sql);

    match parse_result {
        Ok(mut pairs) => {
            // Get the top-level statement rule
            let statement = pairs.next().ok_or_else(|| {
                BundlebaseError::from("Parser produced empty result")
            })?;

            // Get the inner statement type (filter_stmt, attach_stmt, etc.)
            let inner_stmt = statement.into_inner().next().ok_or_else(|| {
                BundlebaseError::from("Parser produced empty inner statement")
            })?;

            let cmd = match inner_stmt.as_rule() {
                Rule::filter_stmt => parse_filter_pest(inner_stmt)?,
                Rule::attach_stmt => parse_attach_pest(inner_stmt)?,
                Rule::join_stmt => parse_join_pest(inner_stmt)?,
                Rule::reindex_stmt => parse_reindex_pest(inner_stmt)?,
                Rule::create_source_stmt => parse_create_source_pest(inner_stmt)?,
                Rule::fetch_stmt => parse_fetch_pest(inner_stmt)?,
                Rule::drop_join_stmt => parse_drop_join_pest(inner_stmt)?,
                Rule::rename_join_stmt => parse_rename_join_pest(inner_stmt)?,
                _ => return Err("Unexpected statement type".into()),
            };
            Ok(Some(cmd))
        }
        Err(e) => {
            // Not custom syntax or parse error
            // Return None to let sqlparser-rs handle it
            if is_likely_custom_syntax(sql) {
                // If it looks like custom syntax but failed to parse, report error
                Err(format_pest_error(e, sql))
            } else {
                // Not custom syntax, return None
                Ok(None)
            }
        }
    }
}

fn is_likely_custom_syntax(sql: &str) -> bool {
    let upper = sql.trim().to_uppercase();
    upper.starts_with("FILTER")
        || upper.starts_with("ATTACH")
        || upper.starts_with("REINDEX")
        || upper.starts_with("JOIN")
        || upper.starts_with("LEFT JOIN")
        || upper.starts_with("RIGHT JOIN")
        || upper.starts_with("FULL JOIN")
        || upper.starts_with("INNER JOIN")
        || upper.starts_with("CREATE SOURCE")
        || upper.starts_with("FETCH")
        || upper.starts_with("DROP JOIN")
        || upper.starts_with("RENAME JOIN")
}

fn format_pest_error(error: pest::error::Error<Rule>, sql: &str) -> BundlebaseError {
    // Pest provides detailed error info with line/column
    let (line, col) = match &error.line_col {
        pest::error::LineColLocation::Pos((l, c)) => (*l, *c),
        pest::error::LineColLocation::Span((l, c), _) => (*l, *c),
    };

    format!(
        "Syntax error at line {}, column {}:\n{}\n\nSQL:\n{}",
        line, col, error, sql
    )
    .into()
}

fn parse_filter_pest(pair: pest::iterators::Pair<Rule>) -> Result<BundleCommand, BundlebaseError> {
    let mut where_clause = None;

    for inner_pair in pair.into_inner() {
        match inner_pair.as_rule() {
            Rule::where_condition => {
                where_clause = Some(inner_pair.as_str().trim().to_string());
            }
            _ => {}
        }
    }

    let where_clause = where_clause
        .ok_or_else(|| -> BundlebaseError { "FILTER statement missing WHERE clause".into() })?;

    if where_clause.is_empty() {
        return Err("FILTER WHERE clause cannot be empty".into());
    }

    Ok(BundleCommand::Filter {
        where_clause,
        params: vec![],
    })
}

fn parse_attach_pest(pair: pest::iterators::Pair<Rule>) -> Result<BundleCommand, BundlebaseError> {
    let mut path = None;
    let mut pack = None;
    let raw = pair.as_str().to_string();

    for inner_pair in pair.into_inner() {
        match inner_pair.as_rule() {
            Rule::quoted_string => {
                if path.is_none() {
                    path = Some(extract_string_content(inner_pair.as_str())?);
                }
            }
            Rule::identifier => {
                // The identifier after TO is the pack name
                if pack.is_none() {
                    pack = Some(inner_pair.as_str().to_string());
                }
            }
            Rule::with_options => {
                // WITH options - not used yet
            }
            _ => {}
        }
    }

    // If pack wasn't captured from inner pairs, try to extract from raw string
    // (the grammar consumes "to" as a keyword, not a captured rule)
    if pack.is_none() {
        let upper = raw.to_uppercase();
        if let Some(to_pos) = upper.find(" TO ") {
            let after_to = raw[to_pos + 4..].trim_start();
            let pack_name: String = after_to
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !pack_name.is_empty() {
                pack = Some(pack_name);
            }
        }
    }

    let path = path.ok_or_else(|| -> BundlebaseError { "ATTACH statement missing path".into() })?;

    Ok(BundleCommand::Attach { path, pack })
}

fn parse_join_pest(pair: pest::iterators::Pair<Rule>) -> Result<BundleCommand, BundlebaseError> {
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
                name = Some(inner_pair.as_str().to_string());
            }
            Rule::join_condition => {
                expression = Some(inner_pair.as_str().trim().to_string());
            }
            _ => {}
        }
    }

    let location =
        location.ok_or_else(|| -> BundlebaseError { "JOIN statement missing location file".into() })?;
    let name =
        name.ok_or_else(|| -> BundlebaseError { "JOIN statement missing AS name".into() })?;
    let expression = expression
        .ok_or_else(|| -> BundlebaseError { "JOIN statement missing ON expression".into() })?;

    if expression.is_empty() {
        return Err("JOIN ON expression cannot be empty".into());
    }

    Ok(BundleCommand::Join {
        name,
        location: Some(location),
        expression,
        join_type,
    })
}

fn parse_reindex_pest(
    _pair: pest::iterators::Pair<Rule>,
) -> Result<BundleCommand, BundlebaseError> {
    // For now, just return Reindex (rebuild all indexes)
    // TODO: Support column-specific reindexing if needed
    Ok(BundleCommand::Reindex)
}

fn parse_create_source_pest(
    pair: pest::iterators::Pair<Rule>,
) -> Result<BundleCommand, BundlebaseError> {
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

    let function = function
        .ok_or_else(|| -> BundlebaseError { "CREATE SOURCE missing function name".into() })?;

    if args.is_empty() {
        return Err("CREATE SOURCE requires at least one argument in WITH clause".into());
    }

    Ok(BundleCommand::CreateSource {
        function,
        args,
        pack,
    })
}

fn parse_fetch_pest(
    pair: pest::iterators::Pair<Rule>,
) -> Result<BundleCommand, BundlebaseError> {
    // Check if it's FETCH ALL, FETCH <pack>, or just FETCH (base pack)
    let raw = pair.as_str().to_uppercase();

    for inner_pair in pair.into_inner() {
        if inner_pair.as_rule() == Rule::identifier {
            let ident = inner_pair.as_str();
            // Check if it's "all" (case insensitive)
            if ident.eq_ignore_ascii_case("all") {
                return Ok(BundleCommand::FetchAll);
            }
            // Otherwise it's a pack name
            return Ok(BundleCommand::Fetch {
                pack: Some(ident.to_string()),
            });
        }
    }

    // If we get here with just "FETCH ALL" where "all" was parsed as keyword
    if raw.contains("ALL") {
        return Ok(BundleCommand::FetchAll);
    }

    // Just "FETCH" - fetch from base pack
    Ok(BundleCommand::Fetch { pack: None })
}

fn parse_drop_join_pest(
    pair: pest::iterators::Pair<Rule>,
) -> Result<BundleCommand, BundlebaseError> {
    let mut name = None;

    for inner_pair in pair.into_inner() {
        if inner_pair.as_rule() == Rule::identifier {
            name = Some(inner_pair.as_str().to_string());
        }
    }

    let name = name
        .ok_or_else(|| -> BundlebaseError { "DROP JOIN statement missing join name".into() })?;

    Ok(BundleCommand::DropJoin { name })
}

fn parse_rename_join_pest(
    pair: pest::iterators::Pair<Rule>,
) -> Result<BundleCommand, BundlebaseError> {
    let mut old_name = None;
    let mut new_name = None;

    for inner_pair in pair.into_inner() {
        if inner_pair.as_rule() == Rule::identifier {
            if old_name.is_none() {
                old_name = Some(inner_pair.as_str().to_string());
            } else {
                new_name = Some(inner_pair.as_str().to_string());
            }
        }
    }

    let old_name = old_name
        .ok_or_else(|| -> BundlebaseError { "RENAME JOIN statement missing old join name".into() })?;
    let new_name = new_name
        .ok_or_else(|| -> BundlebaseError { "RENAME JOIN statement missing new join name".into() })?;

    Ok(BundleCommand::RenameJoin { old_name, new_name })
}

// Helper functions

fn extract_string_content(quoted: &str) -> Result<String, BundlebaseError> {
    let trimmed = quoted.trim();

    // Remove surrounding quotes
    let content = if trimmed.starts_with('\'') && trimmed.ends_with('\'') {
        &trimmed[1..trimmed.len() - 1]
    } else if trimmed.starts_with('"') && trimmed.ends_with('"') {
        &trimmed[1..trimmed.len() - 1]
    } else {
        return Err(format!("Invalid quoted string: {}", quoted).into());
    };

    // Process escape sequences
    Ok(process_escapes(content))
}

fn process_escapes(s: &str) -> String {
    s.replace("\\\\", "\\")
        .replace("\\'", "'")
        .replace("\\\"", "\"")
        .replace("\\n", "\n")
        .replace("\\r", "\r")
        .replace("\\t", "\t")
}

fn parse_join_type(s: &str) -> Result<JoinTypeOption, BundlebaseError> {
    let normalized = s.trim().to_lowercase();
    Ok(match normalized.as_str() {
        "inner" => JoinTypeOption::Inner,
        "left" => JoinTypeOption::Left,
        "right" => JoinTypeOption::Right,
        "full" | "outer" | "full outer" => JoinTypeOption::Full,
        _ => return Err(format!("Unknown join type: {}", s).into()),
    })
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_filter_simple() {
        let sql = "FILTER WHERE country = 'USA'";
        let result = parse_custom_pest(sql).unwrap();

        match result {
            Some(BundleCommand::Filter { where_clause, .. }) => {
                assert_eq!(where_clause, "country = 'USA'");
            }
            _ => panic!("Expected Filter variant"),
        }
    }

    #[test]
    fn test_parse_filter_complex() {
        let sql = "FILTER WHERE age > 21 AND (city = 'NYC' OR city = 'LA')";
        let result = parse_custom_pest(sql).unwrap();

        match result {
            Some(BundleCommand::Filter { where_clause, .. }) => {
                assert_eq!(where_clause, "age > 21 AND (city = 'NYC' OR city = 'LA')");
            }
            _ => panic!("Expected Filter variant"),
        }
    }

    #[test]
    fn test_parse_filter_case_insensitive() {
        let sql = "filter where id > 100";
        let result = parse_custom_pest(sql).unwrap();

        match result {
            Some(BundleCommand::Filter { where_clause, .. }) => {
                assert_eq!(where_clause, "id > 100");
            }
            _ => panic!("Expected Filter variant"),
        }
    }

    #[test]
    fn test_parse_attach_simple() {
        let sql = "ATTACH 'data.parquet'";
        let result = parse_custom_pest(sql).unwrap();

        match result {
            Some(BundleCommand::Attach { path, pack }) => {
                assert_eq!(path, "data.parquet");
                assert_eq!(pack, None);
            }
            _ => panic!("Expected Attach variant"),
        }
    }

    #[test]
    fn test_parse_attach_double_quotes() {
        let sql = "ATTACH \"data.csv\"";
        let result = parse_custom_pest(sql).unwrap();

        match result {
            Some(BundleCommand::Attach { path, pack }) => {
                assert_eq!(path, "data.csv");
                assert_eq!(pack, None);
            }
            _ => panic!("Expected Attach variant"),
        }
    }

    #[test]
    fn test_parse_attach_with_escapes() {
        let sql = "ATTACH 'path/with\\'quote.csv'";
        let result = parse_custom_pest(sql).unwrap();

        match result {
            Some(BundleCommand::Attach { path, pack }) => {
                assert_eq!(path, "path/with'quote.csv");
                assert_eq!(pack, None);
            }
            _ => panic!("Expected Attach variant"),
        }
    }

    #[test]
    fn test_parse_attach_to_pack() {
        let sql = "ATTACH 'more_users.parquet' TO users";
        let result = parse_custom_pest(sql).unwrap();

        match result {
            Some(BundleCommand::Attach { path, pack }) => {
                assert_eq!(path, "more_users.parquet");
                assert_eq!(pack, Some("users".to_string()));
            }
            _ => panic!("Expected Attach variant"),
        }
    }

    #[test]
    fn test_parse_attach_to_pack_case_insensitive() {
        let sql = "attach 'file.json' to joined_data";
        let result = parse_custom_pest(sql).unwrap();

        match result {
            Some(BundleCommand::Attach { path, pack }) => {
                assert_eq!(path, "file.json");
                assert_eq!(pack, Some("joined_data".to_string()));
            }
            _ => panic!("Expected Attach variant"),
        }
    }

    #[test]
    fn test_parse_join_inner() {
        let sql = "JOIN 'other.csv' AS other ON id = other.id";
        let result = parse_custom_pest(sql).unwrap();

        match result {
            Some(BundleCommand::Join {
                name,
                location,
                expression,
                join_type,
            }) => {
                assert_eq!(name, "other");
                assert_eq!(location, Some("other.csv".to_string()));
                assert_eq!(expression, "id = other.id");
                assert_eq!(join_type, JoinTypeOption::Inner);
            }
            _ => panic!("Expected Join variant"),
        }
    }

    #[test]
    fn test_parse_join_left() {
        let sql = "LEFT JOIN 'users.parquet' AS users ON user_id = users.id";
        let result = parse_custom_pest(sql).unwrap();

        match result {
            Some(BundleCommand::Join {
                name,
                location,
                expression,
                join_type,
            }) => {
                assert_eq!(name, "users");
                assert_eq!(location, Some("users.parquet".to_string()));
                assert_eq!(expression, "user_id = users.id");
                assert_eq!(join_type, JoinTypeOption::Left);
            }
            _ => panic!("Expected Join variant"),
        }
    }

    #[test]
    fn test_parse_join_full() {
        let sql = "FULL OUTER JOIN 'data.json' AS data ON key = data.key";
        let result = parse_custom_pest(sql).unwrap();

        match result {
            Some(BundleCommand::Join {
                name,
                location,
                expression,
                join_type,
            }) => {
                assert_eq!(name, "data");
                assert_eq!(location, Some("data.json".to_string()));
                assert_eq!(expression, "key = data.key");
                assert_eq!(join_type, JoinTypeOption::Full);
            }
            _ => panic!("Expected Join variant"),
        }
    }

    #[test]
    fn test_error_missing_where() {
        let sql = "FILTER country = 'USA'";
        let result = parse_custom_pest(sql);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Syntax error"));
    }

    #[test]
    fn test_error_missing_on() {
        let sql = "JOIN AS other id = other.id";
        let result = parse_custom_pest(sql);

        assert!(result.is_err());
    }

    #[test]
    fn test_error_position_info() {
        // Test that missing WHERE keyword produces error with line/column info
        let sql = "FILTER country = 'USA'";
        let result = parse_custom_pest(sql);

        assert!(result.is_err());
        let err = result.unwrap_err();
        // Should contain line/column information in the Pest error
        assert!(err.to_string().contains("line") || err.to_string().contains("column"));
    }

    #[test]
    fn test_parse_create_source_simple() {
        let sql = "CREATE SOURCE remote_dir WITH (url = 's3://bucket/data/')";
        let result = parse_custom_pest(sql).unwrap();

        match result {
            Some(BundleCommand::CreateSource {
                function,
                args,
                pack,
            }) => {
                assert_eq!(function, "remote_dir");
                assert_eq!(args.get("url"), Some(&"s3://bucket/data/".to_string()));
                assert_eq!(pack, None);
            }
            _ => panic!("Expected CreateSource variant"),
        }
    }

    #[test]
    fn test_parse_create_source_with_patterns() {
        let sql = "CREATE SOURCE remote_dir WITH (url = 's3://bucket/data/', patterns = '**/*.parquet')";
        let result = parse_custom_pest(sql).unwrap();

        match result {
            Some(BundleCommand::CreateSource {
                function,
                args,
                pack,
            }) => {
                assert_eq!(function, "remote_dir");
                assert_eq!(args.get("url"), Some(&"s3://bucket/data/".to_string()));
                assert_eq!(args.get("patterns"), Some(&"**/*.parquet".to_string()));
                assert_eq!(pack, None);
            }
            _ => panic!("Expected CreateSource variant"),
        }
    }

    #[test]
    fn test_parse_create_source_with_pack() {
        let sql = "CREATE SOURCE remote_dir WITH (url = 's3://bucket/users/') ON users";
        let result = parse_custom_pest(sql).unwrap();

        match result {
            Some(BundleCommand::CreateSource {
                function,
                args,
                pack,
            }) => {
                assert_eq!(function, "remote_dir");
                assert_eq!(args.get("url"), Some(&"s3://bucket/users/".to_string()));
                assert_eq!(pack, Some("users".to_string()));
            }
            _ => panic!("Expected CreateSource variant"),
        }
    }

    #[test]
    fn test_parse_create_source_case_insensitive() {
        let sql = "create source remote_dir with (url = 'file:///data/')";
        let result = parse_custom_pest(sql).unwrap();

        match result {
            Some(BundleCommand::CreateSource {
                function,
                args,
                pack,
            }) => {
                assert_eq!(function, "remote_dir");
                assert_eq!(args.get("url"), Some(&"file:///data/".to_string()));
                assert_eq!(pack, None);
            }
            _ => panic!("Expected CreateSource variant"),
        }
    }

    #[test]
    fn test_parse_fetch_pack() {
        let sql = "FETCH users";
        let result = parse_custom_pest(sql).unwrap();

        match result {
            Some(BundleCommand::Fetch { pack }) => {
                assert_eq!(pack, Some("users".to_string()));
            }
            _ => panic!("Expected Fetch variant"),
        }
    }

    #[test]
    fn test_parse_fetch_base() {
        let sql = "FETCH";
        let result = parse_custom_pest(sql).unwrap();

        match result {
            Some(BundleCommand::Fetch { pack }) => {
                assert_eq!(pack, None);
            }
            _ => panic!("Expected Fetch variant"),
        }
    }

    #[test]
    fn test_parse_fetch_all() {
        let sql = "FETCH ALL";
        let result = parse_custom_pest(sql).unwrap();

        match result {
            Some(BundleCommand::FetchAll) => {}
            _ => panic!("Expected FetchAll variant"),
        }
    }

    #[test]
    fn test_parse_fetch_case_insensitive() {
        let sql = "fetch users";
        let result = parse_custom_pest(sql).unwrap();

        match result {
            Some(BundleCommand::Fetch { pack }) => {
                assert_eq!(pack, Some("users".to_string()));
            }
            _ => panic!("Expected Fetch variant"),
        }
    }

    #[test]
    fn test_parse_fetch_all_case_insensitive() {
        let sql = "fetch all";
        let result = parse_custom_pest(sql).unwrap();

        match result {
            Some(BundleCommand::FetchAll) => {}
            _ => panic!("Expected FetchAll variant"),
        }
    }
}
