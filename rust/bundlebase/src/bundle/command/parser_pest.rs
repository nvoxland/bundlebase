use crate::bundle::command::BundleCommand;
use crate::bundle::operation::JoinTypeOption;
use crate::BundlebaseError;
use pest::Parser;
use pest_derive::Parser;

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
            let statement = pairs.next().unwrap();

            // Get the inner statement type (filter_stmt, attach_stmt, etc.)
            let inner_stmt = statement.into_inner().next().unwrap();

            let cmd = match inner_stmt.as_rule() {
                Rule::filter_stmt => parse_filter_pest(inner_stmt)?,
                Rule::attach_stmt => parse_attach_pest(inner_stmt)?,
                Rule::attach_to_join_stmt => parse_attach_to_join_pest(inner_stmt)?,
                Rule::join_stmt => parse_join_pest(inner_stmt)?,
                Rule::reindex_stmt => parse_reindex_pest(inner_stmt)?,
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

    for inner_pair in pair.into_inner() {
        match inner_pair.as_rule() {
            Rule::quoted_string => {
                if path.is_none() {
                    path = Some(extract_string_content(inner_pair.as_str())?);
                }
            }
            Rule::identifier => {
                // AS alias - not used yet
            }
            Rule::with_options => {
                // WITH options - not used yet
            }
            _ => {}
        }
    }

    let path = path.ok_or_else(|| -> BundlebaseError { "ATTACH statement missing path".into() })?;

    Ok(BundleCommand::Attach { path })
}

fn parse_attach_to_join_pest(
    pair: pest::iterators::Pair<Rule>,
) -> Result<BundleCommand, BundlebaseError> {
    let mut path = None;
    let mut join = None;

    for inner_pair in pair.into_inner() {
        match inner_pair.as_rule() {
            Rule::quoted_string => {
                // First quoted string is the path, second is the join name
                if path.is_none() {
                    path = Some(extract_string_content(inner_pair.as_str())?);
                } else if join.is_none() {
                    join = Some(extract_string_content(inner_pair.as_str())?);
                }
            }
            _ => {}
        }
    }

    let path =
        path.ok_or_else(|| -> BundlebaseError { "ATTACH TO JOIN statement missing path".into() })?;
    let join = join.ok_or_else(|| -> BundlebaseError {
        "ATTACH TO JOIN statement missing join name".into()
    })?;

    Ok(BundleCommand::AttachToJoin { join, path })
}

fn parse_join_pest(pair: pest::iterators::Pair<Rule>) -> Result<BundleCommand, BundlebaseError> {
    let mut join_type = JoinTypeOption::Inner;
    let mut source = None;
    let mut name = None;
    let mut expression = None;

    for inner_pair in pair.into_inner() {
        match inner_pair.as_rule() {
            Rule::join_type => {
                join_type = parse_join_type(inner_pair.as_str())?;
            }
            Rule::quoted_string => {
                // First quoted string is the source file
                if source.is_none() {
                    source = Some(extract_string_content(inner_pair.as_str())?);
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

    let source =
        source.ok_or_else(|| -> BundlebaseError { "JOIN statement missing source file".into() })?;
    let name =
        name.ok_or_else(|| -> BundlebaseError { "JOIN statement missing AS name".into() })?;
    let expression = expression
        .ok_or_else(|| -> BundlebaseError { "JOIN statement missing ON expression".into() })?;

    if expression.is_empty() {
        return Err("JOIN ON expression cannot be empty".into());
    }

    Ok(BundleCommand::Join {
        name,
        source,
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

fn parse_with_options(
    pair: pest::iterators::Pair<Rule>,
) -> Result<std::collections::HashMap<String, String>, BundlebaseError> {
    let mut options = std::collections::HashMap::new();

    for inner_pair in pair.into_inner() {
        if inner_pair.as_rule() == Rule::option_pair {
            let mut key = None;
            let mut value = None;

            for opt_inner in inner_pair.into_inner() {
                match opt_inner.as_rule() {
                    Rule::identifier => {
                        if key.is_none() {
                            key = Some(opt_inner.as_str().to_string());
                        }
                    }
                    Rule::option_value => {
                        value = Some(extract_option_value(opt_inner)?);
                    }
                    _ => {}
                }
            }

            if let (Some(k), Some(v)) = (key, value) {
                options.insert(k, v);
            }
        }
    }

    Ok(options)
}

fn extract_option_value(pair: pest::iterators::Pair<Rule>) -> Result<String, BundlebaseError> {
    let inner = pair.into_inner().next().unwrap();

    match inner.as_rule() {
        Rule::quoted_string => extract_string_content(inner.as_str()),
        Rule::number | Rule::boolean | Rule::identifier => Ok(inner.as_str().to_string()),
        _ => Err(format!("Invalid option value: {}", inner.as_str()).into()),
    }
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
            Some(BundleCommand::Attach { path }) => {
                assert_eq!(path, "data.parquet");
            }
            _ => panic!("Expected Attach variant"),
        }
    }

    #[test]
    fn test_parse_attach_double_quotes() {
        let sql = "ATTACH \"data.csv\"";
        let result = parse_custom_pest(sql).unwrap();

        match result {
            Some(BundleCommand::Attach { path }) => {
                assert_eq!(path, "data.csv");
            }
            _ => panic!("Expected Attach variant"),
        }
    }

    #[test]
    fn test_parse_attach_with_escapes() {
        let sql = "ATTACH 'path/with\\'quote.csv'";
        let result = parse_custom_pest(sql).unwrap();

        match result {
            Some(BundleCommand::Attach { path }) => {
                assert_eq!(path, "path/with'quote.csv");
            }
            _ => panic!("Expected Attach variant"),
        }
    }

    #[test]
    fn test_parse_attach_to_join() {
        let sql = "ATTACH 'more_users.parquet' TO JOIN 'users'";
        let result = parse_custom_pest(sql).unwrap();

        match result {
            Some(BundleCommand::AttachToJoin { join, path }) => {
                assert_eq!(join, "users");
                assert_eq!(path, "more_users.parquet");
            }
            _ => panic!("Expected AttachToJoin variant"),
        }
    }

    #[test]
    fn test_parse_attach_to_join_double_quotes() {
        let sql = "ATTACH \"data.csv\" TO JOIN \"additional\"";
        let result = parse_custom_pest(sql).unwrap();

        match result {
            Some(BundleCommand::AttachToJoin { join, path }) => {
                assert_eq!(join, "additional");
                assert_eq!(path, "data.csv");
            }
            _ => panic!("Expected AttachToJoin variant"),
        }
    }

    #[test]
    fn test_parse_attach_to_join_case_insensitive() {
        let sql = "attach 'file.json' to join 'joined_data'";
        let result = parse_custom_pest(sql).unwrap();

        match result {
            Some(BundleCommand::AttachToJoin { join, path }) => {
                assert_eq!(join, "joined_data");
                assert_eq!(path, "file.json");
            }
            _ => panic!("Expected AttachToJoin variant"),
        }
    }

    #[test]
    fn test_parse_join_inner() {
        let sql = "JOIN 'other.csv' AS other ON id = other.id";
        let result = parse_custom_pest(sql).unwrap();

        match result {
            Some(BundleCommand::Join {
                name,
                source,
                expression,
                join_type,
            }) => {
                assert_eq!(name, "other");
                assert_eq!(source, "other.csv");
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
                source,
                expression,
                join_type,
            }) => {
                assert_eq!(name, "users");
                assert_eq!(source, "users.parquet");
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
                source,
                expression,
                join_type,
            }) => {
                assert_eq!(name, "data");
                assert_eq!(source, "data.json");
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
}
