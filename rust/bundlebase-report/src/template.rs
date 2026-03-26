//! Typst document template for report generation.
//!
//! Provides the page layout, font configuration, and styling rules
//! that give reports a consistent, pleasant appearance.

/// Generate the Typst template preamble.
///
/// This is prepended to the rendered report content and sets up:
/// - Page layout (US letter, 1-inch margins)
/// - Font configuration
/// - Heading styles
/// - cetz and cetz-plot package imports
/// - Optional "Created by Bundlebase" footer
pub fn template_preamble(show_branding: bool) -> String {
    let mut preamble = String::new();

    preamble.push_str("#import \"@preview/cetz:0.4.2\" as cetz\n");
    preamble.push_str("#import \"@preview/cetz-plot:0.1.3\" as cetz-plot\n\n");

    preamble.push_str("#set page(\n");
    preamble.push_str("  paper: \"us-letter\",\n");
    preamble.push_str("  margin: (x: 1in, y: 1in),\n");

    if show_branding {
        preamble.push_str("  footer: context {\n");
        preamble.push_str("    align(center)[\n");
        preamble.push_str("      #text(size: 8pt, fill: rgb(\"#999999\"))[Created by Bundlebase]\n");
        preamble.push_str("    ]\n");
        preamble.push_str("  },\n");
    }

    preamble.push_str(")\n\n");

    preamble.push_str("#set text(\n");
    preamble.push_str("  size: 11pt,\n");
    preamble.push_str(")\n\n");

    preamble.push_str("#set heading(numbering: none)\n\n");

    preamble.push_str("#show heading.where(level: 1): set text(size: 18pt)\n");
    preamble.push_str("#show heading.where(level: 2): set text(size: 15pt)\n");
    preamble.push_str("#show heading.where(level: 3): set text(size: 13pt)\n\n");

    preamble.push_str("#show figure: set block(breakable: true)\n\n");

    preamble
}

/// Convert a markdown text block to Typst markup.
///
/// Handles common markdown elements:
/// - Headings (# → =)
/// - Bold (**text** → *text*)
/// - Italic (*text* → _text_)
/// - Bullet lists (- → -)
/// - Numbered lists (1. → 1.)
pub fn markdown_to_typst(markdown: &str) -> String {
    let mut output = String::new();

    for line in markdown.lines() {
        let converted = convert_line(line);
        output.push_str(&converted);
        output.push('\n');
    }

    output
}

/// Convert a single markdown line to Typst.
fn convert_line(line: &str) -> String {
    // Headings: # → =
    if let Some(rest) = line.strip_prefix("##### ") {
        return format!("===== {}", convert_inline(rest));
    }
    if let Some(rest) = line.strip_prefix("#### ") {
        return format!("==== {}", convert_inline(rest));
    }
    if let Some(rest) = line.strip_prefix("### ") {
        return format!("=== {}", convert_inline(rest));
    }
    if let Some(rest) = line.strip_prefix("## ") {
        return format!("== {}", convert_inline(rest));
    }
    if let Some(rest) = line.strip_prefix("# ") {
        return format!("= {}", convert_inline(rest));
    }

    // Bullet lists: keep as-is (Typst uses same syntax)
    if line.starts_with("- ") || line.starts_with("  - ") {
        return format!("{}", convert_inline(line));
    }

    // Numbered lists: keep as-is
    if line.chars().next().map_or(false, |c| c.is_ascii_digit()) && line.contains(". ") {
        return convert_inline(line);
    }

    // Regular text
    convert_inline(line)
}

/// Convert inline markdown formatting to Typst.
fn convert_inline(text: &str) -> String {
    let mut result = String::new();
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Bold: **text** → *text*
        if i + 1 < len && chars[i] == '*' && chars[i + 1] == '*' {
            if let Some(end) = find_closing(&chars, i + 2, &['*', '*']) {
                result.push('*');
                let inner: String = chars[i + 2..end].iter().collect();
                result.push_str(&inner);
                result.push('*');
                i = end + 2;
                continue;
            }
        }

        // Italic: *text* → _text_ (single asterisk, not double)
        if chars[i] == '*' && (i + 1 >= len || chars[i + 1] != '*') {
            if let Some(end) = find_closing_single(&chars, i + 1, '*') {
                result.push('_');
                let inner: String = chars[i + 1..end].iter().collect();
                result.push_str(&inner);
                result.push('_');
                i = end + 1;
                continue;
            }
        }

        // Escape Typst-special characters in regular text
        match chars[i] {
            '#' => result.push_str("\\#"),
            '$' => result.push_str("\\$"),
            '@' => result.push_str("\\@"),
            _ => result.push(chars[i]),
        }
        i += 1;
    }

    result
}

/// Find closing double-char marker (e.g., **).
fn find_closing(chars: &[char], start: usize, marker: &[char; 2]) -> Option<usize> {
    let len = chars.len();
    let mut i = start;
    while i + 1 < len {
        if chars[i] == marker[0] && chars[i + 1] == marker[1] {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Find closing single-char marker.
fn find_closing_single(chars: &[char], start: usize, marker: char) -> Option<usize> {
    for i in start..chars.len() {
        if chars[i] == marker {
            return Some(i);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heading_conversion() {
        assert_eq!(convert_line("# Title"), "= Title");
        assert_eq!(convert_line("## Section"), "== Section");
        assert_eq!(convert_line("### Sub"), "=== Sub");
    }

    #[test]
    fn test_bold_conversion() {
        assert_eq!(convert_inline("This is **bold** text"), "This is *bold* text");
    }

    #[test]
    fn test_italic_conversion() {
        assert_eq!(convert_inline("This is *italic* text"), "This is _italic_ text");
    }

    #[test]
    fn test_special_char_escaping() {
        assert_eq!(convert_inline("Cost is $100"), "Cost is \\$100");
        assert_eq!(convert_inline("#tag"), "\\#tag");
    }

    #[test]
    fn test_bullet_list() {
        assert_eq!(convert_line("- item one"), "- item one");
    }

    #[test]
    fn test_template_with_branding() {
        let preamble = template_preamble(true);
        assert!(preamble.contains("Created by Bundlebase"));
        assert!(preamble.contains("cetz-plot"));
    }

    #[test]
    fn test_template_without_branding() {
        let preamble = template_preamble(false);
        assert!(!preamble.contains("Created by Bundlebase"));
    }

    #[test]
    fn test_markdown_to_typst_multiline() {
        let md = "# Report\n\nSome **bold** and *italic* text.\n\n- bullet\n";
        let result = markdown_to_typst(md);
        assert!(result.contains("= Report"));
        assert!(result.contains("*bold*"));
        assert!(result.contains("_italic_"));
        assert!(result.contains("- bullet"));
    }
}
