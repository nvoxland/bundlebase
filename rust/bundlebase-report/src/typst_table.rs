//! Generate Typst table markup from table blocks and query results.

use crate::defaults;
use crate::parse::TableBlock;
use crate::query::BoundedQueryResult;
use bundlebase_common::BundlebaseError;

/// Generate Typst markup for a table block with its query results.
pub fn render_table(table: &TableBlock, data: &BoundedQueryResult) -> Result<String, BundlebaseError> {
    if data.columns.is_empty() {
        return Err(BundlebaseError::from("Table query returned no columns"));
    }

    let opts = &table.options;
    let num_cols = data.columns.len();

    let mut lines = Vec::new();

    // Wrap in figure if there's a title
    if table.title.is_some() {
        lines.push("#figure(".to_string());
    }

    // Table opening with column count
    let columns_spec = get_opt_str(opts, "columns")
        .unwrap_or_else(|| format!("({})", vec!["auto"; num_cols].join(", ")));
    lines.push(format!("#table("));
    lines.push(format!("  columns: {},", columns_spec));

    // Border/stroke
    let border = get_opt_str(opts, "border")
        .unwrap_or_else(|| defaults::TABLE_BORDER.to_string());
    lines.push(format!("  stroke: {},", border));

    // Alignment
    if let Some(align) = get_opt_str(opts, "align") {
        lines.push(format!("  align: {},", align));
    }

    // Cell padding
    if let Some(inset) = get_opt_str(opts, "inset") {
        lines.push(format!("  inset: {},", inset));
    } else {
        lines.push("  inset: (x: 8pt, y: 5pt),".to_string());
    }

    // Fill: handle zebra striping and header fill
    let zebra = get_opt_bool(opts, "zebra").unwrap_or(true);
    let header_fill = get_opt_str(opts, "header_fill")
        .unwrap_or_else(|| format!("rgb(\"{}\")", defaults::TABLE_HEADER_FILL));
    let zebra_color = get_opt_str(opts, "zebra_color")
        .unwrap_or_else(|| format!("rgb(\"{}\")", defaults::TABLE_ZEBRA_COLOR));

    if zebra {
        lines.push(format!(
            "  fill: (_, y) => if y == 0 {{ {} }} else if calc.rem(y, 2) == 0 {{ {} }},",
            header_fill, zebra_color
        ));
    } else {
        lines.push(format!("  fill: (_, y) => if y == 0 {{ {} }},", header_fill));
    }

    // Header row
    lines.push("  table.header(".to_string());
    for real_name in &data.columns {
        lines.push(format!("    [*{}*],", escape_typst(real_name)));
    }
    lines.push("  ),".to_string());

    // Data rows
    for row in &data.rows {
        for (col_idx, val) in row.iter().enumerate() {
            let cell_text = json_to_display(val);
            let _ = col_idx; // reserved for per-column formatting
            lines.push(format!("  [{}],", escape_typst(&cell_text)));
        }
    }

    lines.push(")".to_string());

    // Close figure with caption
    if let Some(title) = &table.title {
        lines.push(format!(", caption: [{}])", escape_typst(title)));
    }

    lines.push(String::new());

    Ok(lines.join("\n"))
}

/// Convert a JSON value to a display string for table cells.
fn json_to_display(val: &serde_json::Value) -> String {
    match val {
        serde_json::Value::Null => "".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(json_to_display).collect();
            items.join(", ")
        }
        serde_json::Value::Object(_) => "[object]".to_string(),
    }
}

/// Extract a string option from YAML, handling various value types.
fn get_opt_str(opts: &serde_yaml_ng::Value, key: &str) -> Option<String> {
    opts.get(key).map(|v| match v {
        serde_yaml_ng::Value::String(s) => s.clone(),
        serde_yaml_ng::Value::Bool(b) => b.to_string(),
        serde_yaml_ng::Value::Number(n) => n.to_string(),
        serde_yaml_ng::Value::Sequence(seq) => {
            let items: Vec<String> = seq
                .iter()
                .map(|item| match item {
                    serde_yaml_ng::Value::String(s) => s.clone(),
                    _ => format!("{:?}", item),
                })
                .collect();
            format!("({})", items.join(", "))
        }
        serde_yaml_ng::Value::Mapping(map) => {
            let entries: Vec<String> = map
                .iter()
                .map(|(k, v)| {
                    let key_str = match k {
                        serde_yaml_ng::Value::String(s) => s.replace('_', "-"),
                        _ => format!("{:?}", k),
                    };
                    let val_str = match v {
                        serde_yaml_ng::Value::String(s) => s.clone(),
                        serde_yaml_ng::Value::Number(n) => n.to_string(),
                        _ => format!("{:?}", v),
                    };
                    format!("{}: {}", key_str, val_str)
                })
                .collect();
            format!("({})", entries.join(", "))
        }
        _ => format!("{:?}", v),
    })
}

/// Extract a boolean option from YAML.
fn get_opt_bool(opts: &serde_yaml_ng::Value, key: &str) -> Option<bool> {
    opts.get(key).and_then(|v| match v {
        serde_yaml_ng::Value::Bool(b) => Some(*b),
        _ => None,
    })
}

/// Escape special Typst characters in text content.
fn escape_typst(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('#', "\\#")
        .replace('$', "\\$")
        .replace('@', "\\@")
        .replace('<', "\\<")
        .replace('>', "\\>")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_data() -> BoundedQueryResult {
        BoundedQueryResult {
            columns: vec!["product".to_string(), "units".to_string()],
            rows: vec![
                vec![serde_json::json!("Widget A"), serde_json::json!(1500)],
                vec![serde_json::json!("Widget B"), serde_json::json!(1200)],
            ],
        }
    }

    #[test]
    fn test_render_table_basic() {
        let table = TableBlock {
            bundle: "test".to_string(),
            query: "SELECT product, units FROM bundle".to_string(),
            title: None,
            options: serde_yaml_ng::Value::Null,
        };
        let result = render_table(&table, &sample_data()).expect("should render");
        assert!(result.contains("#table("));
        assert!(result.contains("[*product*]"));
        assert!(result.contains("[*units*]"));
        assert!(result.contains("[Widget A]"));
        assert!(result.contains("[1500]"));
        // Default zebra should be on
        assert!(result.contains("calc.rem"));
    }

    #[test]
    fn test_render_table_with_title() {
        let table = TableBlock {
            bundle: "test".to_string(),
            query: "SELECT product, units FROM bundle".to_string(),
            title: Some("Top Products".to_string()),
            options: serde_yaml_ng::Value::Null,
        };
        let result = render_table(&table, &sample_data()).expect("should render");
        assert!(result.contains("#figure("));
        assert!(result.contains("Top Products"));
    }

    #[test]
    fn test_render_table_no_zebra() {
        let opts: serde_yaml_ng::Value =
            serde_yaml_ng::from_str("zebra: false").expect("valid yaml");
        let table = TableBlock {
            bundle: "test".to_string(),
            query: "SELECT product, units FROM bundle".to_string(),
            title: None,
            options: opts,
        };
        let result = render_table(&table, &sample_data()).expect("should render");
        assert!(!result.contains("calc.rem"));
    }

    #[test]
    fn test_render_table_empty_columns() {
        let data = BoundedQueryResult {
            columns: vec![],
            rows: vec![],
        };
        let table = TableBlock {
            bundle: "test".to_string(),
            query: "SELECT 1".to_string(),
            title: None,
            options: serde_yaml_ng::Value::Null,
        };
        let result = render_table(&table, &data);
        assert!(result.is_err());
    }

    #[test]
    fn test_render_table_with_nulls() {
        let data = BoundedQueryResult {
            columns: vec!["name".to_string(), "value".to_string()],
            rows: vec![vec![
                serde_json::json!("test"),
                serde_json::Value::Null,
            ]],
        };
        let table = TableBlock {
            bundle: "test".to_string(),
            query: "SELECT name, value FROM bundle".to_string(),
            title: None,
            options: serde_yaml_ng::Value::Null,
        };
        let result = render_table(&table, &data).expect("should render");
        assert!(result.contains("[],"));
    }

    #[test]
    fn test_json_to_display() {
        assert_eq!(json_to_display(&serde_json::json!(42)), "42");
        assert_eq!(json_to_display(&serde_json::json!("hello")), "hello");
        assert_eq!(json_to_display(&serde_json::json!(true)), "true");
        assert_eq!(json_to_display(&serde_json::Value::Null), "");
    }
}
