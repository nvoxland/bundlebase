//! Escaping for text interpolated into generated Typst source.
//!
//! Report definitions and query results are bundle data, so any text placed
//! into Typst markup must be escaped for the context it lands in.

/// Escape text for use inside a Typst markup content block (`[...]`):
/// titles, captions, axis labels, table cells. Brackets are escaped so a `]`
/// can't close the block early and let the rest be parsed as Typst code.
pub(crate) fn escape_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '[' | ']' | '#' | '$' | '@' | '<' | '>') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Escape text for use inside a Typst double-quoted string literal. Only
/// backslash and `"` are special there; markup escapes like `\#` are invalid
/// inside a string and an unescaped `"` would break out of it.
pub(crate) fn escape_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_markup() {
        assert_eq!(escape_markup("a]b"), "a\\]b");
        assert_eq!(escape_markup("[x]"), "\\[x\\]");
        assert_eq!(escape_markup("hello #world $100"), "hello \\#world \\$100");
        assert_eq!(escape_markup("x < 5 @y > 1"), "x \\< 5 \\@y \\> 1");
        assert_eq!(escape_markup("a\\b"), "a\\\\b");
    }

    #[test]
    fn test_escape_string() {
        assert_eq!(escape_string("he\"llo"), "he\\\"llo");
        assert_eq!(escape_string("a\\b"), "a\\\\b");
        assert_eq!(escape_string("#[x]"), "#[x]");
    }
}
