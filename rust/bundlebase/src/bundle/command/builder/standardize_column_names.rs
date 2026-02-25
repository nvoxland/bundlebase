//! StandardizeColumnNames command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::operation::RenameColumnOp;
use crate::bundle::BundleFacade;
use crate::BundlebaseError;
use arrow::datatypes::SchemaRef;
use async_trait::async_trait;
use std::collections::HashMap;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to standardize all column names to lowercase+underscore identifiers.
#[derive(Debug, Clone)]
pub struct StandardizeColumnNamesCommand;

impl CommandParsing for StandardizeColumnNamesCommand {
    fn rule() -> Rule {
        // This command is not parsed from SQL, but CommandParsing requires a rule.
        // Use an arbitrary rule; from_statement will never be called.
        Rule::identifier
    }

    fn from_statement(_pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        Err("STANDARDIZE COLUMN NAMES cannot be parsed from SQL".into())
    }

    fn to_statement(&self) -> String {
        "STANDARDIZE COLUMN NAMES".to_string()
    }
}

/// Standardize a single column name to a lowercase+underscore identifier.
fn standardize_single_name(name: &str) -> String {
    // 1. Convert to lowercase
    let lowered = name.to_lowercase();

    // 2. Replace non-alphanumeric, non-underscore characters with underscores
    let replaced: String = lowered
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
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

    // 5. If empty after standardization, use "column"
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
        let standardized = standardize_single_name(name);
        let count = counts.entry(standardized.clone()).or_insert(0);
        *count += 1;

        let final_name = if *count == 1 {
            standardized
        } else {
            format!("{}_{}", standardized, count)
        };

        // Only record renames where the name actually changed
        if *name != final_name {
            renames.push((name.to_string(), final_name));
        }
    }

    renames
}

#[async_trait]
impl BundleBuilderCommand for StandardizeColumnNamesCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let schema = builder.bundle().schema().await?;
        let renames = compute_renames(&schema);
        let count = renames.len();

        for (old_name, new_name) in &renames {
            let column_id = builder.column_id(old_name)
                .ok_or_else(|| BundlebaseError::from(format!("Column '{}' not found", old_name)))?;
            builder
                .apply_operation(
                    RenameColumnOp::setup(column_id, new_name).into(),
                )
                .await?;
        }

        Ok(format!("Standardized column names: {} columns renamed", count))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_standardize_single_name() {
        // Spaces become underscores
        assert_eq!(standardize_single_name("Customer Id"), "customer_id");
        assert_eq!(standardize_single_name("First Name"), "first_name");
        assert_eq!(standardize_single_name("Phone 1"), "phone_1");

        // Dots and dashes
        assert_eq!(standardize_single_name("file.name"), "file_name");
        assert_eq!(standardize_single_name("first-name"), "first_name");

        // Uppercase
        assert_eq!(standardize_single_name("UPPER_CASE"), "upper_case");
        assert_eq!(standardize_single_name("CamelCase"), "camelcase");

        // Leading digits
        assert_eq!(standardize_single_name("1st_column"), "_1st_column");

        // Empty
        assert_eq!(standardize_single_name(""), "column");
        assert_eq!(standardize_single_name("___"), "column");

        // Already simple
        assert_eq!(standardize_single_name("simple"), "simple");
        assert_eq!(standardize_single_name("already_good"), "already_good");

        // Consecutive special chars
        assert_eq!(standardize_single_name("a--b"), "a_b");
        assert_eq!(standardize_single_name("a  b"), "a_b");

        // Leading/trailing special chars
        assert_eq!(standardize_single_name(" name "), "name");
        assert_eq!(standardize_single_name("_name_"), "name");
    }

    #[test]
    fn test_compute_renames_dedup() {
        use arrow::datatypes::{Field, Schema, DataType};
        use std::sync::Arc;

        // No duplicates
        let schema = Arc::new(Schema::new(vec![
            Field::new("First Name", DataType::Utf8, false),
            Field::new("Last Name", DataType::Utf8, false),
        ]));
        let renames = compute_renames(&schema);
        assert_eq!(renames, vec![
            ("First Name".to_string(), "first_name".to_string()),
            ("Last Name".to_string(), "last_name".to_string()),
        ]);

        // Duplicates after standardization
        let schema = Arc::new(Schema::new(vec![
            Field::new("User Name", DataType::Utf8, false),
            Field::new("user-name", DataType::Utf8, false),
            Field::new("user.name", DataType::Utf8, false),
        ]));
        let renames = compute_renames(&schema);
        assert_eq!(renames, vec![
            ("User Name".to_string(), "user_name".to_string()),
            ("user-name".to_string(), "user_name_2".to_string()),
            ("user.name".to_string(), "user_name_3".to_string()),
        ]);

        // Already simple names should not appear in renames
        let schema = Arc::new(Schema::new(vec![
            Field::new("simple", DataType::Utf8, false),
            Field::new("already_good", DataType::Utf8, false),
        ]));
        let renames = compute_renames(&schema);
        assert!(renames.is_empty());
    }
}
