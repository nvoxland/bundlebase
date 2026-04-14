//! Markdown parser that extracts report elements from markdown with YAML fenced blocks.
//!
//! Scans for ` ```bundlebase ` fenced code blocks, parses their YAML content,
//! and returns a sequence of report elements. The `type` field determines
//! whether it's a table (`type: table`) or a chart (any other type value).

use bundlebase_common::BundlebaseError;
use serde::Deserialize;
use std::fmt;

/// A parsed element from the report markdown.
#[derive(Debug, Clone)]
pub enum ReportElement {
    /// Regular markdown text (headings, paragraphs, lists, etc.)
    Text(String),
    /// A chart block with `type` set to a chart type (pie, bar, line, etc.)
    Chart(ChartBlock),
    /// A table block with `type: table`.
    Table(TableBlock),
}

/// Supported chart types.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChartType {
    Pie,
    Bar,
    Line,
    HorizontalBar,
    BoxWhisker,
    Pyramid,
    ErrorBar,
    Violin,
}

impl fmt::Display for ChartType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChartType::Pie => write!(f, "pie"),
            ChartType::Bar => write!(f, "bar"),
            ChartType::Line => write!(f, "line"),
            ChartType::HorizontalBar => write!(f, "horizontal_bar"),
            ChartType::BoxWhisker => write!(f, "box_whisker"),
            ChartType::Pyramid => write!(f, "pyramid"),
            ChartType::ErrorBar => write!(f, "error_bar"),
            ChartType::Violin => write!(f, "violin"),
        }
    }
}

/// A parsed chart block.
#[derive(Debug, Clone)]
pub struct ChartBlock {
    pub bundle: String,
    pub query: String,
    pub chart_type: ChartType,
    pub title: Option<String>,
    pub options: serde_yaml_ng::Value,
}

/// A parsed table block.
#[derive(Debug, Clone)]
pub struct TableBlock {
    pub bundle: String,
    pub query: String,
    pub title: Option<String>,
    pub options: serde_yaml_ng::Value,
}

/// Raw YAML structure for all bundlebase blocks.
/// The `type` field determines whether this is a table or a specific chart type.
#[derive(Deserialize)]
struct RawBlock {
    bundle: String,
    query: String,
    #[serde(rename = "type")]
    block_type: String,
    title: Option<String>,
    #[serde(default)]
    options: serde_yaml_ng::Value,
}

/// Parse report markdown into a sequence of elements.
///
/// Extracts ` ```bundlebase ` fenced code blocks, parsing their YAML content.
/// The `type` field determines whether each block is a table or chart.
/// Everything else is collected as `Text` elements.
pub fn parse_report(markdown: &str) -> Result<Vec<ReportElement>, BundlebaseError> {
    let mut elements = Vec::new();
    let mut text_accumulator = String::new();
    let mut block_body = String::new();
    let mut in_block = false;
    let mut block_index = 0usize;

    for line in markdown.lines() {
        if in_block {
            let trimmed = line.trim();
            if trimmed == "```" {
                // End of fenced block — parse the YAML body
                let element = parse_block(&block_body, block_index)?;
                elements.push(element);
                in_block = false;
            } else {
                block_body.push_str(line);
                block_body.push('\n');
            }
        } else {
            let trimmed = line.trim();
            if trimmed == "```bundlebase" {
                // Flush accumulated text
                flush_text(&mut text_accumulator, &mut elements);
                in_block = true;
                block_body.clear();
                block_index += 1;
            } else {
                text_accumulator.push_str(line);
                text_accumulator.push('\n');
            }
        }
    }

    // If we're still inside a block at EOF, that's an error
    if in_block {
        return Err(BundlebaseError::from(format!(
            "Unterminated bundlebase block (block #{}) — missing closing ```",
            block_index
        )));
    }

    // Flush any remaining text
    flush_text(&mut text_accumulator, &mut elements);

    Ok(elements)
}

/// Flush accumulated text into an element if non-empty.
fn flush_text(text: &mut String, elements: &mut Vec<ReportElement>) {
    let trimmed = text.trim();
    if !trimmed.is_empty() {
        elements.push(ReportElement::Text(trimmed.to_string()));
    }
    text.clear();
}

/// Parse a fenced block's YAML body into a ReportElement.
///
/// If `type: table`, produces a Table element. Otherwise, parses `type` as a
/// chart type and produces a Chart element.
fn parse_block(yaml_body: &str, block_index: usize) -> Result<ReportElement, BundlebaseError> {
    let raw: RawBlock = serde_yaml_ng::from_str(yaml_body).map_err(|e| {
        BundlebaseError::from(format!(
            "Invalid YAML in bundlebase block #{}: {}",
            block_index, e
        ))
    })?;

    if raw.block_type == "table" {
        Ok(ReportElement::Table(TableBlock {
            bundle: raw.bundle,
            query: raw.query,
            title: raw.title,
            options: raw.options,
        }))
    } else {
        let chart_type: ChartType = serde_yaml_ng::from_str(&format!("\"{}\"", raw.block_type))
            .map_err(|_| {
                BundlebaseError::from(format!(
                    "Unknown type '{}' in bundlebase block #{}. \
                     Valid types: table, pie, bar, line, horizontal_bar, \
                     box_whisker, pyramid, error_bar, violin",
                    raw.block_type, block_index
                ))
            })?;
        Ok(ReportElement::Chart(ChartBlock {
            bundle: raw.bundle,
            query: raw.query,
            chart_type,
            title: raw.title,
            options: raw.options,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_text_only() {
        let md = "# Hello\n\nSome paragraph text.\n\n- bullet one\n- bullet two\n";
        let elements = parse_report(md).expect("should parse");
        assert_eq!(elements.len(), 1);
        assert!(matches!(&elements[0], ReportElement::Text(t) if t.contains("# Hello")));
    }

    #[test]
    fn test_parse_chart_block() {
        let md = r#"# Report

```bundlebase
bundle: sales
query: SELECT region, SUM(revenue) FROM bundle GROUP BY region
type: pie
title: Revenue by Region
options:
  radius: 4
  inner_radius: 1
```

Some text after.
"#;
        let elements = parse_report(md).expect("should parse");
        assert_eq!(elements.len(), 3);

        assert!(matches!(&elements[0], ReportElement::Text(t) if t == "# Report"));

        if let ReportElement::Chart(chart) = &elements[1] {
            assert_eq!(chart.bundle, "sales");
            assert_eq!(chart.chart_type, ChartType::Pie);
            assert_eq!(chart.title.as_deref(), Some("Revenue by Region"));
            assert!(chart.query.contains("SELECT"));
            assert_eq!(
                chart.options["radius"],
                serde_yaml_ng::Value::Number(4.into())
            );
        } else {
            panic!("Expected Chart element");
        }

        assert!(matches!(&elements[2], ReportElement::Text(t) if t.contains("Some text after")));
    }

    #[test]
    fn test_parse_table_block() {
        let md = r##"```bundlebase
bundle: inventory
query: SELECT product, count FROM bundle
type: table
title: Product Counts
options:
  zebra: true
  header_fill: "#f0f4f8"
```
"##;
        let elements = parse_report(md).expect("should parse");
        assert_eq!(elements.len(), 1);

        if let ReportElement::Table(table) = &elements[0] {
            assert_eq!(table.bundle, "inventory");
            assert_eq!(table.title.as_deref(), Some("Product Counts"));
            assert_eq!(table.options["zebra"], serde_yaml_ng::Value::Bool(true));
        } else {
            panic!("Expected Table element");
        }
    }

    #[test]
    fn test_parse_multiple_blocks() {
        let md = r#"# Report

```bundlebase
bundle: data
query: SELECT x, y FROM bundle
type: line
```

Middle text.

```bundlebase
bundle: data
query: SELECT a, b FROM bundle
type: table
```

End text.
"#;
        let elements = parse_report(md).expect("should parse");
        assert_eq!(elements.len(), 5);
        assert!(matches!(&elements[0], ReportElement::Text(_)));
        assert!(matches!(&elements[1], ReportElement::Chart(_)));
        assert!(matches!(&elements[2], ReportElement::Text(_)));
        assert!(matches!(&elements[3], ReportElement::Table(_)));
        assert!(matches!(&elements[4], ReportElement::Text(_)));
    }

    #[test]
    fn test_parse_missing_type_field() {
        let md = r#"```bundlebase
bundle: sales
query: SELECT * FROM bundle
```
"#;
        let result = parse_report(md);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("bundlebase block"));
    }

    #[test]
    fn test_parse_all_chart_types() {
        let types = vec![
            ("pie", ChartType::Pie),
            ("bar", ChartType::Bar),
            ("line", ChartType::Line),
            ("horizontal_bar", ChartType::HorizontalBar),
            ("box_whisker", ChartType::BoxWhisker),
            ("pyramid", ChartType::Pyramid),
            ("error_bar", ChartType::ErrorBar),
            ("violin", ChartType::Violin),
        ];
        for (type_str, expected) in types {
            let md = format!(
                "```bundlebase\nbundle: data\nquery: SELECT * FROM bundle\ntype: {}\n```\n",
                type_str
            );
            let elements = parse_report(&md)
                .unwrap_or_else(|e| panic!("should parse type '{}': {}", type_str, e));
            if let ReportElement::Chart(chart) = &elements[0] {
                assert_eq!(chart.chart_type, expected, "type '{}' mismatch", type_str);
            } else {
                panic!("Expected Chart element for type '{}'", type_str);
            }
        }
    }

    #[test]
    fn test_parse_invalid_chart_type() {
        let md = r#"```bundlebase
bundle: sales
query: SELECT * FROM bundle
type: scatter
```
"#;
        let result = parse_report(md);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown type 'scatter'"));
    }

    #[test]
    fn test_parse_unterminated_block() {
        let md = r#"```bundlebase
bundle: sales
query: SELECT * FROM bundle
type: pie
"#;
        let result = parse_report(md);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unterminated"));
    }

    #[test]
    fn test_parse_no_title() {
        let md = r#"```bundlebase
bundle: sales
query: SELECT x, y FROM bundle
type: bar
```
"#;
        let elements = parse_report(md).expect("should parse");
        if let ReportElement::Chart(chart) = &elements[0] {
            assert!(chart.title.is_none());
        } else {
            panic!("Expected Chart element");
        }
    }

    #[test]
    fn test_parse_no_options() {
        let md = r#"```bundlebase
bundle: data
query: SELECT * FROM bundle
type: table
```
"#;
        let elements = parse_report(md).expect("should parse");
        if let ReportElement::Table(table) = &elements[0] {
            assert!(table.options.is_null());
        } else {
            panic!("Expected Table element");
        }
    }

    #[test]
    fn test_parse_empty_markdown() {
        let elements = parse_report("").expect("should parse");
        assert!(elements.is_empty());
    }

    #[test]
    fn test_parse_whitespace_only() {
        let elements = parse_report("   \n\n   \n").expect("should parse");
        assert!(elements.is_empty());
    }

    #[test]
    fn test_regular_code_blocks_are_text() {
        let md = "```rust\nfn main() {}\n```\n";
        let elements = parse_report(md).expect("should parse");
        // Regular code blocks should be treated as text
        assert_eq!(elements.len(), 1);
        assert!(matches!(&elements[0], ReportElement::Text(_)));
    }
}
