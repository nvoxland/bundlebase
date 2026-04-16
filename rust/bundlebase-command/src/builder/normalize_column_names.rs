//! NormalizeColumnNames command implementation.

use crate::BundleBuilderCommand;
use crate::{CommandParsing, Rule};
use arrow::datatypes::SchemaRef;
use bundlebase::bundle::operation::RenameColumnOp;
use bundlebase::bundle::BundleFacade;
use bundlebase::BundleBuilder;
use bundlebase_common::BundlebaseError;
use std::collections::HashMap;

/// Command to normalize all column names to lowercase+underscore identifiers.
#[derive(Debug, Clone)]
pub struct NormalizeColumnNamesCommand;

impl CommandParsing for NormalizeColumnNamesCommand {
    fn rule() -> Rule {
        Rule::normalize_column_names_stmt
    }

    fn from_statement(_pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        Ok(NormalizeColumnNamesCommand)
    }

    fn to_statement(&self) -> String {
        "NORMALIZE COLUMN NAMES".to_string()
    }
}

/// Normalize a single column name to a lowercase+underscore identifier.
fn normalize_single_name(name: &str) -> String {
    // 1. Convert to lowercase
    let lowered = name.to_lowercase();

    // 2. Replace non-alphanumeric, non-underscore characters with underscores
    let replaced: String = lowered
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();

    // 3. Collapse consecutive underscores into one
    let mut collapsed = String::with_capacity(replaced.len());
    let mut prev_underscore = false;
    for c in replaced.chars() {
        if c == '_' {
            if !prev_underscore {
                collapsed.push('_');
            }
            prev_underscore = true;
        } else {
            collapsed.push(c);
            prev_underscore = false;
        }
    }

    // 4. Strip leading/trailing underscores
    let trimmed = collapsed.trim_matches('_').to_string();

    // 5. If empty after normalization, use "column"
    if trimmed.is_empty() {
        return "column".to_string();
    }

    // 6. If starts with digit, prefix with underscore
    if trimmed.starts_with(|c: char| c.is_ascii_digit()) {
        return format!("_{}", trimmed);
    }

    trimmed
}

/// Compute renames for a schema, handling duplicates by appending `_2`, `_3`, etc.
fn compute_renames(schema: &SchemaRef) -> Vec<(String, String)> {
    let field_names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut renames = Vec::new();

    for name in &field_names {
        let normalized = normalize_single_name(name);
        let count = counts.entry(normalized.clone()).or_insert(0);
        *count += 1;

        let final_name = if *count == 1 {
            normalized
        } else {
            format!("{}_{}", normalized, count)
        };

        // Only record renames where the name actually changed
        if *name != final_name {
            renames.push((name.to_string(), final_name));
        }
    }

    renames
}

impl BundleBuilderCommand for NormalizeColumnNamesCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let schema = builder.bundle().schema().await?;
        let renames = compute_renames(&schema);
        let count = renames.len();

        for (old_name, new_name) in &renames {
            let column_id = builder
                .column_id(old_name)
                .ok_or_else(|| BundlebaseError::from(format!("Column '{}' not found", old_name)))?;
            builder
                .apply_operation(RenameColumnOp::setup(column_id, new_name).into())
                .await?;
        }

        Ok(format!(
            "Normalized column names: {} columns renamed",
            count
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_normalize_column_names() {
        use crate::parser::parse_command;
        use crate::BundleCommand;

        let cmd = parse_command("NORMALIZE COLUMN NAMES").expect("Should parse");
        assert!(matches!(cmd, BundleCommand::NormalizeColumnNames(_)));

        // Case insensitive
        let cmd = parse_command("normalize column names").expect("Should parse lowercase");
        assert!(matches!(cmd, BundleCommand::NormalizeColumnNames(_)));
    }

    #[test]
    fn test_roundtrip() {
        let cmd = NormalizeColumnNamesCommand;
        assert_eq!(cmd.to_statement(), "NORMALIZE COLUMN NAMES");
    }

    #[test]
    fn test_normalize_single_name() {
        // Spaces become underscores
        assert_eq!(normalize_single_name("Customer Id"), "customer_id");
        assert_eq!(normalize_single_name("First Name"), "first_name");
        assert_eq!(normalize_single_name("Phone 1"), "phone_1");

        // Dots and dashes
        assert_eq!(normalize_single_name("file.name"), "file_name");
        assert_eq!(normalize_single_name("first-name"), "first_name");

        // Uppercase
        assert_eq!(normalize_single_name("UPPER_CASE"), "upper_case");
        assert_eq!(normalize_single_name("CamelCase"), "camelcase");

        // Leading digits
        assert_eq!(normalize_single_name("1st_column"), "_1st_column");

        // Empty
        assert_eq!(normalize_single_name(""), "column");
        assert_eq!(normalize_single_name("___"), "column");

        // Already simple
        assert_eq!(normalize_single_name("simple"), "simple");
        assert_eq!(normalize_single_name("already_good"), "already_good");

        // Consecutive special chars
        assert_eq!(normalize_single_name("a--b"), "a_b");
        assert_eq!(normalize_single_name("a  b"), "a_b");

        // Leading/trailing special chars
        assert_eq!(normalize_single_name(" name "), "name");
        assert_eq!(normalize_single_name("_name_"), "name");
    }

    #[test]
    fn test_compute_renames_dedup() {
        use arrow::datatypes::{DataType, Field, Schema};
        use std::sync::Arc;

        // No duplicates
        let schema = Arc::new(Schema::new(vec![
            Field::new("First Name", DataType::Utf8, false),
            Field::new("Last Name", DataType::Utf8, false),
        ]));
        let renames = compute_renames(&schema);
        assert_eq!(
            renames,
            vec![
                ("First Name".to_string(), "first_name".to_string()),
                ("Last Name".to_string(), "last_name".to_string()),
            ]
        );

        // Duplicates after normalization
        let schema = Arc::new(Schema::new(vec![
            Field::new("User Name", DataType::Utf8, false),
            Field::new("user-name", DataType::Utf8, false),
            Field::new("user.name", DataType::Utf8, false),
        ]));
        let renames = compute_renames(&schema);
        assert_eq!(
            renames,
            vec![
                ("User Name".to_string(), "user_name".to_string()),
                ("user-name".to_string(), "user_name_2".to_string()),
                ("user.name".to_string(), "user_name_3".to_string()),
            ]
        );

        // Already simple names should not appear in renames
        let schema = Arc::new(Schema::new(vec![
            Field::new("simple", DataType::Utf8, false),
            Field::new("already_good", DataType::Utf8, false),
        ]));
        let renames = compute_renames(&schema);
        assert!(renames.is_empty());
    }
}
