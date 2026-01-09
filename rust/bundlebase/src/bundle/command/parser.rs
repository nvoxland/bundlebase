use crate::bundle::command::parser_pest::parse_custom_pest;
use crate::bundle::command::BundleCommand;
use crate::BundlebaseError;
use sqlparser::ast::{ObjectType, Statement};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;

/// Parse a command statement into a BundleCommand.
///
/// This is the main entry point for parsing command statements into BundleCommand that can be
/// executed on a BundleBuilder.
///
/// It handles:
/// 1. Parsing custom bundlebase syntax (FILTER, ATTACH, JOIN, REINDEX) using Pest
/// 2. Parsing standard SQL (SELECT, CREATE INDEX, etc.) using sqlparser-rs
/// 3. Converting parsed statements into BundleCommand variants
///
/// # Arguments
///
/// * `command_str` - The command statement string to parse
///
/// # Returns
///
/// * `Ok(BundleCommand)` - Successfully parsed command
/// * `Err(BundlebaseError)` - Parsing failed or statement type not supported
///
/// # Examples
///
/// ```ignore
/// use bundlebase::bundle::{parse_command, BundleCommand};
///
/// // Parse a FILTER statement
/// let cmd = parse_command("FILTER WHERE country = 'USA'").unwrap();
///
/// // Parse a SELECT statement
/// let cmd = parse_command("SELECT name, email FROM bundle").unwrap();
///
/// // Parse an ATTACH statement
/// let cmd = parse_command("ATTACH 'data.parquet'").unwrap();
///
/// // Execute on a BundleBuilder
/// cmd.execute(&mut bundle).await?;
/// ```
pub fn parse_command(command_str: &str) -> Result<BundleCommand, BundlebaseError> {
    // First, try Pest grammar for custom bundlebase syntax (FILTER, ATTACH, JOIN, REINDEX)
    if let Some(op) = parse_custom_pest(command_str)? {
        return Ok(op);
    }

    // Check for RENAME VIEW command: RENAME VIEW old_name TO new_name
    if command_str.trim().to_uppercase().starts_with("RENAME VIEW") {
        let parts: Vec<&str> = command_str.split_whitespace().collect();
        if parts.len() == 5 && parts[3].eq_ignore_ascii_case("TO") {
            return Ok(BundleCommand::RenameView {
                old_name: parts[2].trim_matches(|c| c == '"' || c == '\'').to_string(),
                new_name: parts[4].trim_matches(|c| c == '"' || c == '\'').to_string(),
            });
        } else {
            return Err(
                "Invalid RENAME VIEW syntax. Expected: RENAME VIEW old_name TO new_name".into(),
            );
        }
    }

    // Otherwise, use sqlparser-rs for standard SQL (SELECT, CREATE INDEX, etc.)
    let dialect = GenericDialect {};
    let ast = Parser::parse_sql(&dialect, command_str)
        .map_err(|e| -> BundlebaseError { format!("SQL parse error: {}", e).into() })?;

    if ast.is_empty() {
        return Err("Empty SQL statement".into());
    }

    if ast.len() > 1 {
        return Err(
            "Multiple statements not supported. Please execute one statement at a time.".into(),
        );
    }

    let stmt = &ast[0];

    // Dispatch to appropriate operation based on statement type
    dispatch_statement(stmt)
}

/// Dispatch a SQL statement to the appropriate BundleCommand.
///
/// This function examines the statement type and creates the appropriate BundleCommand variant
/// that will execute the corresponding BundleBuilder method.
fn dispatch_statement(stmt: &Statement) -> Result<BundleCommand, BundlebaseError> {
    match stmt {
        // SELECT statements -> Select
        Statement::Query(_query) => Ok(BundleCommand::Select {
            sql: stmt.to_string(),
            params: vec![],
        }),

        // CREATE INDEX -> Index
        Statement::CreateIndex { .. } => {
            // sqlparser 0.59 changed CreateIndex structure
            // For now, return error - use REINDEX or custom INDEX commands instead
            Err("CREATE INDEX via standard SQL is not yet supported. Use bundlebase INDEX command or REINDEX.".into())
        }

        // DROP INDEX -> DropIndex
        Statement::Drop {
            object_type: ObjectType::Index,
            names,
            ..
        } => {
            // Extract column name from index name
            let column = extract_column_from_index_name(names)?;
            Ok(BundleCommand::DropIndex { column })
        }

        // Unrecognized statement types
        _ => Err(format!("Unsupported SQL statement type: {:?}", stmt).into()),
    }
}

/// Extract column name from index name.
///
/// For now, we assume index names follow the pattern "idx_{column_name}"
/// or just use the first object name directly as the column.
fn extract_column_from_index_name(
    names: &[sqlparser::ast::ObjectName],
) -> Result<String, BundlebaseError> {
    let name = names
        .first()
        .ok_or_else(|| -> BundlebaseError { "DROP INDEX requires index name".into() })?;

    // Convert the entire object name to string
    // This handles the sqlparser 0.59 API changes
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sql_empty() {
        let result = parse_command("");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Empty"));
    }

    #[test]
    fn test_parse_sql_multiple_statements() {
        let result = parse_command("SELECT * FROM bundle; SELECT * FROM bundle2;");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Multiple statements"));
    }
}
