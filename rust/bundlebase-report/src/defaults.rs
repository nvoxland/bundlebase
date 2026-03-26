//! Default styling constants for report generation.
//!
//! Provides a pleasant color palette and default styling values
//! used when options are not specified in chart/table blocks.

/// Default color palette for charts (8 pleasant, distinct colors).
pub const CHART_COLORS: &[&str] = &[
    "#4e79a7", // steel blue
    "#f28e2b", // orange
    "#e15759", // red
    "#76b7b2", // teal
    "#59a14f", // green
    "#edc949", // yellow
    "#af7aa1", // purple
    "#ff9da7", // pink
];

/// Default table header fill color.
pub const TABLE_HEADER_FILL: &str = "#e8edf2";

/// Default zebra stripe color for alternating table rows.
pub const TABLE_ZEBRA_COLOR: &str = "#f5f7fa";

/// Default table border stroke.
pub const TABLE_BORDER: &str = "0.5pt + rgb(\"#cccccc\")";
