//! Generate cetz-plot Typst markup from chart blocks and query results.

use crate::defaults::CHART_COLORS;
use crate::parse::{ChartBlock, ChartType};
use crate::query::BoundedQueryResult;
use bundlebase_common::BundlebaseError;

/// Generate Typst markup for a chart block with its query results.
pub fn render_chart(chart: &ChartBlock, data: &BoundedQueryResult) -> Result<String, BundlebaseError> {
    if data.columns.len() < 2 {
        return Err(BundlebaseError::from(format!(
            "Chart query must return at least 2 columns, got {}",
            data.columns.len()
        )));
    }

    let markup = match chart.chart_type {
        ChartType::Pie => render_pie(chart, data)?,
        ChartType::Bar => render_bar(chart, data)?,
        ChartType::Line => render_line(chart, data)?,
        ChartType::HorizontalBar => render_horizontal_bar(chart, data)?,
        ChartType::BoxWhisker => render_box_whisker(chart, data)?,
        ChartType::Pyramid => render_pyramid(chart, data)?,
        ChartType::ErrorBar => render_error_bar(chart, data)?,
        ChartType::Violin => render_violin(chart, data)?,
    };

    // Wrap in figure with optional title
    let mut output = String::new();
    if chart.title.is_some() {
        output.push_str("#figure(\n");
    }
    output.push_str(&markup);
    if let Some(title) = &chart.title {
        output.push_str(&format!(",\ncaption: [{}]\n)\n", escape_typst(title)));
    }
    output.push('\n');

    Ok(output)
}

/// Render a pie chart using cetz-plot chart.piechart.
fn render_pie(chart: &ChartBlock, data: &BoundedQueryResult) -> Result<String, BundlebaseError> {
    let opts = &chart.options;
    let mut lines = Vec::new();

    lines.push("cetz.canvas({".to_string());
    lines.push("  import cetz-plot.chart: piechart".to_string());

    // Build data array: ((label, value), ...)
    let data_str = build_tuple_data(data)?;

    // Build options
    let mut params = Vec::new();
    params.push(format!("  {}", data_str));
    params.push("  label-key: 0".to_string());
    params.push("  value-key: 1".to_string());

    // Slice styling (colors)
    let colors = get_color_list(opts, "slice_style");
    params.push(format!("  slice-style: {}", colors));

    // Optional parameters from options
    if let Some(v) = get_yaml_number(opts, "radius") {
        params.push(format!("  radius: {}", v));
    }
    if let Some(v) = get_yaml_number(opts, "inner_radius") {
        params.push(format!("  inner-radius: {}", v));
    }
    if let Some(v) = get_yaml_number(opts, "outset") {
        params.push(format!("  outset: {}", v));
    }
    if let Some(v) = get_yaml_str(opts, "stroke") {
        params.push(format!("  stroke: {}", v));
    }

    // Label configuration
    if let Some(label_opts) = opts.get("outer_label") {
        params.push(format!("  outer-label: {}", yaml_to_typst_dict(label_opts)));
    }
    if let Some(label_opts) = opts.get("inner_label") {
        params.push(format!("  inner-label: {}", yaml_to_typst_dict(label_opts)));
    }

    // Legend
    if let Some(v) = get_yaml_str(opts, "legend") {
        params.push(format!("  legend: \"{}\"", v));
    }

    lines.push(format!("  piechart(\n{}\n  )", params.join(",\n")));
    lines.push("})".to_string());

    Ok(lines.join("\n"))
}

/// Render a bar chart using cetz-plot chart.columnchart.
fn render_bar(chart: &ChartBlock, data: &BoundedQueryResult) -> Result<String, BundlebaseError> {
    let opts = &chart.options;
    let mut lines = Vec::new();

    lines.push("cetz.canvas({".to_string());
    lines.push("  import cetz-plot.chart: columnchart".to_string());

    let data_str = build_tuple_data(data)?;

    let mut params = Vec::new();
    params.push(format!("  {}", data_str));
    params.push("  label-key: 0".to_string());
    params.push("  value-key: 1".to_string());

    // Size
    if let Some(size) = get_yaml_array_pair(opts, "size") {
        params.push(format!("  size: ({})", size));
    } else {
        params.push("  size: (10, 6)".to_string());
    }

    // Bar styling
    let colors = get_color_list(opts, "bar_style");
    params.push(format!("  bar-style: {}", colors));

    if let Some(v) = get_yaml_number(opts, "bar_width") {
        params.push(format!("  bar-width: {}", v));
    }
    if let Some(v) = get_yaml_str(opts, "mode") {
        params.push(format!("  mode: \"{}\"", v));
    }

    // Axis labels
    if let Some(v) = get_yaml_str(opts, "x_label") {
        params.push(format!("  x-label: [{}]", escape_typst(&v)));
    }
    if let Some(v) = get_yaml_str(opts, "y_label") {
        params.push(format!("  y-label: [{}]", escape_typst(&v)));
    }

    // Legend
    if let Some(v) = get_yaml_str(opts, "legend") {
        params.push(format!("  legend: \"{}\"", v));
    }
    if let Some(labels) = get_yaml_str_array(opts, "labels") {
        params.push(format!("  labels: ({})", labels.iter().map(|l| format!("\"{}\"", l)).collect::<Vec<_>>().join(", ")));
    }

    lines.push(format!("  columnchart(\n{}\n  )", params.join(",\n")));
    lines.push("})".to_string());

    Ok(lines.join("\n"))
}

/// Render a line chart using cetz-plot plot.plot + plot.add.
fn render_line(chart: &ChartBlock, data: &BoundedQueryResult) -> Result<String, BundlebaseError> {
    let opts = &chart.options;
    let mut lines = Vec::new();

    lines.push("cetz.canvas({".to_string());
    lines.push("  import cetz-plot.plot".to_string());

    // Build plot.plot() params
    let mut plot_params = Vec::new();

    if let Some(size) = get_yaml_array_pair(opts, "size") {
        plot_params.push(format!("    size: ({})", size));
    } else {
        plot_params.push("    size: (10, 6)".to_string());
    }

    // Axis ranges
    if let Some(v) = get_yaml_number(opts, "x_min") { plot_params.push(format!("    x-min: {}", v)); }
    if let Some(v) = get_yaml_number(opts, "x_max") { plot_params.push(format!("    x-max: {}", v)); }
    if let Some(v) = get_yaml_number(opts, "y_min") { plot_params.push(format!("    y-min: {}", v)); }
    if let Some(v) = get_yaml_number(opts, "y_max") { plot_params.push(format!("    y-max: {}", v)); }

    // Tick steps
    if let Some(v) = get_yaml_number(opts, "x_tick_step") { plot_params.push(format!("    x-tick-step: {}", v)); }
    if let Some(v) = get_yaml_number(opts, "y_tick_step") { plot_params.push(format!("    y-tick-step: {}", v)); }
    if let Some(v) = get_yaml_number(opts, "x_minor_tick_step") { plot_params.push(format!("    x-minor-tick-step: {}", v)); }
    if let Some(v) = get_yaml_number(opts, "y_minor_tick_step") { plot_params.push(format!("    y-minor-tick-step: {}", v)); }

    // Grid
    if let Some(v) = get_yaml_str(opts, "x_grid") { plot_params.push(format!("    x-grid: \"{}\"", v)); }
    if let Some(v) = get_yaml_str(opts, "y_grid") { plot_params.push(format!("    y-grid: \"{}\"", v)); }

    // Axis labels
    if let Some(v) = get_yaml_str(opts, "x_label") { plot_params.push(format!("    x-label: [{}]", escape_typst(&v))); }
    if let Some(v) = get_yaml_str(opts, "y_label") { plot_params.push(format!("    y-label: [{}]", escape_typst(&v))); }

    // Axis format
    if let Some(v) = get_yaml_str(opts, "x_format") { plot_params.push(format!("    x-format: \"{}\"", v)); }
    if let Some(v) = get_yaml_str(opts, "y_format") { plot_params.push(format!("    y-format: \"{}\"", v)); }

    // Units and decimals
    if let Some(v) = get_yaml_str(opts, "x_unit") { plot_params.push(format!("    x-unit: [{}]", v)); }
    if let Some(v) = get_yaml_str(opts, "y_unit") { plot_params.push(format!("    y-unit: [{}]", v)); }
    if let Some(v) = get_yaml_number(opts, "x_decimals") { plot_params.push(format!("    x-decimals: {}", v)); }
    if let Some(v) = get_yaml_number(opts, "y_decimals") { plot_params.push(format!("    y-decimals: {}", v)); }

    // Legend
    if let Some(v) = get_yaml_str(opts, "legend") { plot_params.push(format!("    legend: \"{}\"", v)); }

    // Build data points for plot.add
    let points = build_point_data(data)?;

    // plot.add options
    let mut add_params = Vec::new();
    add_params.push(format!("      {}", points));

    // Line styling
    if let Some(stroke_opts) = opts.get("stroke") {
        add_params.push(format!("      style: (stroke: {})", yaml_to_typst_value(stroke_opts)));
    } else {
        let color = CHART_COLORS.first().copied().unwrap_or("#4e79a7");
        add_params.push(format!("      style: (stroke: rgb(\"{}\") + 1.5pt)", color));
    }

    // Fill
    if let Some(v) = get_yaml_bool(opts, "fill") {
        if v {
            add_params.push("      fill: true".to_string());
        }
    }

    // Markers
    if let Some(v) = get_yaml_str(opts, "mark") { add_params.push(format!("      mark: \"{}\"", v)); }
    if let Some(v) = get_yaml_number(opts, "mark_size") { add_params.push(format!("      mark-size: {}", v)); }
    if let Some(mark_style) = opts.get("mark_style") {
        add_params.push(format!("      mark-style: {}", yaml_to_typst_dict(mark_style)));
    }

    lines.push(format!("  plot.plot(\n{},\n    {{\n    plot.add(\n{}\n    )\n  }})", plot_params.join(",\n"), add_params.join(",\n")));
    lines.push("})".to_string());

    Ok(lines.join("\n"))
}

/// Render a horizontal bar chart using cetz-plot chart.barchart.
fn render_horizontal_bar(chart: &ChartBlock, data: &BoundedQueryResult) -> Result<String, BundlebaseError> {
    let opts = &chart.options;
    let mut lines = Vec::new();

    lines.push("cetz.canvas({".to_string());
    lines.push("  import cetz-plot.chart: barchart".to_string());

    let data_str = build_tuple_data(data)?;

    let mut params = Vec::new();
    params.push(format!("  {}", data_str));
    params.push("  label-key: 0".to_string());
    params.push("  value-key: 1".to_string());

    // Size
    if let Some(size) = get_yaml_array_pair(opts, "size") {
        params.push(format!("  size: ({})", size));
    } else {
        params.push("  size: (10, 6)".to_string());
    }

    // Bar styling
    let colors = get_color_list(opts, "bar_style");
    params.push(format!("  bar-style: {}", colors));

    if let Some(v) = get_yaml_number(opts, "bar_width") {
        params.push(format!("  bar-width: {}", v));
    }
    if let Some(v) = get_yaml_str(opts, "mode") {
        params.push(format!("  mode: \"{}\"", v));
    }

    // Axis labels
    if let Some(v) = get_yaml_str(opts, "x_label") {
        params.push(format!("  x-label: [{}]", escape_typst(&v)));
    }
    if let Some(v) = get_yaml_str(opts, "y_label") {
        params.push(format!("  y-label: [{}]", escape_typst(&v)));
    }

    // Legend
    if let Some(v) = get_yaml_str(opts, "legend") {
        params.push(format!("  legend: \"{}\"", v));
    }
    if let Some(labels) = get_yaml_str_array(opts, "labels") {
        params.push(format!("  labels: ({})", labels.iter().map(|l| format!("\"{}\"", l)).collect::<Vec<_>>().join(", ")));
    }

    lines.push(format!("  barchart(\n{}\n  )", params.join(",\n")));
    lines.push("})".to_string());

    Ok(lines.join("\n"))
}

/// Render a box-and-whisker chart using cetz-plot chart.boxwhisker.
///
/// Expects query columns: label, min, q1, q2 (median), q3, max.
fn render_box_whisker(chart: &ChartBlock, data: &BoundedQueryResult) -> Result<String, BundlebaseError> {
    if data.columns.len() < 6 {
        return Err(BundlebaseError::from(format!(
            "Box whisker chart requires at least 6 columns (label, min, q1, q2, q3, max), got {}",
            data.columns.len()
        )));
    }

    let opts = &chart.options;
    let mut lines = Vec::new();

    lines.push("cetz.canvas({".to_string());
    lines.push("  import cetz-plot.chart: boxwhisker".to_string());

    // Build data as array of dicts
    let mut dicts = Vec::new();
    for (idx, row) in data.rows.iter().enumerate() {
        if row.len() < 6 {
            return Err(BundlebaseError::from("Each row must have at least 6 columns for box whisker"));
        }
        let mut entries = Vec::new();
        entries.push(format!("label: {}", json_to_typst_value(&row[0])));
        entries.push(format!("x: {}", idx + 1));
        entries.push(format!("min: {}", json_number_or_zero(&row[1])));
        entries.push(format!("q1: {}", json_number_or_zero(&row[2])));
        entries.push(format!("q2: {}", json_number_or_zero(&row[3])));
        entries.push(format!("q3: {}", json_number_or_zero(&row[4])));
        entries.push(format!("max: {}", json_number_or_zero(&row[5])));
        dicts.push(format!("({})", entries.join(", ")));
    }

    let mut params = Vec::new();
    params.push(format!("  ({})", dicts.join(", ")));
    params.push("  label-key: \"label\"".to_string());

    if let Some(size) = get_yaml_array_pair(opts, "size") {
        params.push(format!("  size: ({})", size));
    } else {
        params.push("  size: (10, 6)".to_string());
    }

    if let Some(v) = get_yaml_str(opts, "mark") {
        params.push(format!("  mark: \"{}\"", v));
    }

    lines.push(format!("  boxwhisker(\n{}\n  )", params.join(",\n")));
    lines.push("})".to_string());

    Ok(lines.join("\n"))
}

/// Render a pyramid/funnel chart using cetz-plot chart.pyramid.
///
/// Expects query columns: label, value.
fn render_pyramid(chart: &ChartBlock, data: &BoundedQueryResult) -> Result<String, BundlebaseError> {
    let opts = &chart.options;
    let mut lines = Vec::new();

    lines.push("cetz.canvas({".to_string());
    lines.push("  import cetz-plot.chart: pyramid".to_string());

    let data_str = build_tuple_data(data)?;

    let mut params = Vec::new();
    params.push(format!("  {}", data_str));
    params.push("  value-key: 1".to_string());
    params.push("  label-key: 0".to_string());

    // Level styling (colors)
    let colors = get_color_list(opts, "level_style");
    params.push(format!("  level-style: {}", colors));

    // Mode: REGULAR, AREA-HEIGHT, HEIGHT, WIDTH
    if let Some(v) = get_yaml_str(opts, "mode") {
        params.push(format!("  mode: \"{}\"", v));
    }
    if let Some(v) = get_yaml_number(opts, "gap") {
        params.push(format!("  gap: {}", v));
    }
    if let Some(v) = get_yaml_number(opts, "level_height") {
        params.push(format!("  level-height: {}", v));
    }

    lines.push(format!("  pyramid(\n{}\n  )", params.join(",\n")));
    lines.push("})".to_string());

    Ok(lines.join("\n"))
}

/// Render an error bar chart using cetz-plot plot.add-errorbar.
///
/// Expects query columns: x, y, y_error (and optionally x_error as 4th column).
/// Also renders the data points as a line plot for context.
fn render_error_bar(chart: &ChartBlock, data: &BoundedQueryResult) -> Result<String, BundlebaseError> {
    if data.columns.len() < 3 {
        return Err(BundlebaseError::from(format!(
            "Error bar chart requires at least 3 columns (x, y, y_error), got {}",
            data.columns.len()
        )));
    }

    let opts = &chart.options;
    let has_x_error = data.columns.len() >= 4;
    let mut lines = Vec::new();

    lines.push("cetz.canvas({".to_string());
    lines.push("  import cetz-plot.plot".to_string());

    // Plot params
    let mut plot_params = Vec::new();
    if let Some(size) = get_yaml_array_pair(opts, "size") {
        plot_params.push(format!("    size: ({})", size));
    } else {
        plot_params.push("    size: (10, 6)".to_string());
    }

    if let Some(v) = get_yaml_str(opts, "x_label") { plot_params.push(format!("    x-label: [{}]", escape_typst(&v))); }
    if let Some(v) = get_yaml_str(opts, "y_label") { plot_params.push(format!("    y-label: [{}]", escape_typst(&v))); }
    if let Some(v) = get_yaml_str(opts, "legend") { plot_params.push(format!("    legend: \"{}\"", v)); }

    // Build data points for the line
    let points = build_point_data(data)?;
    let color = CHART_COLORS.first().copied().unwrap_or("#4e79a7");

    // Build individual add-errorbar calls
    let mut add_calls = Vec::new();
    for row in &data.rows {
        let x = json_number_or_zero(&row[0]);
        let y = json_number_or_zero(&row[1]);
        let y_err = json_number_or_zero(&row[2]);

        let mut err_params = Vec::new();
        err_params.push(format!("({}, {})", x, y));
        err_params.push(format!("y-error: {}", y_err));

        if has_x_error {
            if let Some(val) = row.get(3) {
                let x_err = json_number_or_zero(val);
                err_params.push(format!("x-error: {}", x_err));
            }
        }

        add_calls.push(format!("    plot.add-errorbar({})", err_params.join(", ")));
    }

    let body = format!(
        "    plot.add(\n      {},\n      style: (stroke: rgb(\"{}\") + 1.5pt),\n      mark: \"o\",\n      mark-size: 0.15\n    )\n{}",
        points, color, add_calls.join("\n")
    );

    lines.push(format!("  plot.plot(\n{},\n    {{\n{}\n  }})", plot_params.join(",\n"), body));
    lines.push("})".to_string());

    Ok(lines.join("\n"))
}

/// Render a violin plot using cetz-plot plot.add-violin.
///
/// Expects query columns: category, value. Rows are grouped by category,
/// and values within each category form the distribution.
fn render_violin(chart: &ChartBlock, data: &BoundedQueryResult) -> Result<String, BundlebaseError> {
    let opts = &chart.options;
    let mut lines = Vec::new();

    lines.push("cetz.canvas({".to_string());
    lines.push("  import cetz-plot.plot".to_string());

    // Group rows by first column (category), preserving insertion order
    let mut categories: Vec<String> = Vec::new();
    let mut category_values: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();

    for row in &data.rows {
        if row.len() < 2 {
            continue;
        }
        let category = match &row[0] {
            serde_json::Value::String(s) => s.clone(),
            v => v.to_string(),
        };
        if !categories.contains(&category) {
            categories.push(category.clone());
        }
        category_values
            .entry(category)
            .or_default()
            .push(json_number_or_zero(&row[1]));
    }

    // Build violin data: ((x_pos, (values...)), ...)
    let mut tuples = Vec::new();
    for (idx, category) in categories.iter().enumerate() {
        if let Some(values) = category_values.get(category) {
            tuples.push(format!("({}, ({}))", idx + 1, values.join(", ")));
        }
    }
    let data_str = format!("({})", tuples.join(", "));

    // Plot params
    let mut plot_params = Vec::new();
    if let Some(size) = get_yaml_array_pair(opts, "size") {
        plot_params.push(format!("    size: ({})", size));
    } else {
        plot_params.push("    size: (10, 6)".to_string());
    }

    if let Some(v) = get_yaml_str(opts, "x_label") { plot_params.push(format!("    x-label: [{}]", escape_typst(&v))); }
    if let Some(v) = get_yaml_str(opts, "y_label") { plot_params.push(format!("    y-label: [{}]", escape_typst(&v))); }
    if let Some(v) = get_yaml_str(opts, "legend") { plot_params.push(format!("    legend: \"{}\"", v)); }

    // add-violin params
    let mut add_params = Vec::new();
    add_params.push(format!("      {}", data_str));

    if let Some(v) = get_yaml_str(opts, "side") {
        add_params.push(format!("      side: \"{}\"", v));
    } else {
        add_params.push("      side: \"both\"".to_string());
    }
    if let Some(v) = get_yaml_number(opts, "bandwidth") {
        add_params.push(format!("      bandwidth: {}", v));
    }
    if let Some(v) = get_yaml_number(opts, "samples") {
        add_params.push(format!("      samples: {}", v));
    }
    if let Some(v) = get_yaml_number(opts, "extents") {
        add_params.push(format!("      extents: {}", v));
    }

    lines.push(format!(
        "  plot.plot(\n{},\n    {{\n    plot.add-violin(\n{}\n    )\n  }})",
        plot_params.join(",\n"),
        add_params.join(",\n")
    ));
    lines.push("})".to_string());

    Ok(lines.join("\n"))
}

// --- Helper functions ---

/// Build a Typst data array from query results: ((label, value), ...)
fn build_tuple_data(data: &BoundedQueryResult) -> Result<String, BundlebaseError> {
    let mut tuples = Vec::new();
    for row in &data.rows {
        if row.len() < 2 {
            return Err(BundlebaseError::from("Each row must have at least 2 columns"));
        }
        let label = json_to_typst_value(&row[0]);
        let value = json_to_typst_value(&row[1]);
        tuples.push(format!("({}, {})", label, value));
    }
    Ok(format!("({})", tuples.join(", ")))
}

/// Build a Typst data array of (x, y) points for line plots.
fn build_point_data(data: &BoundedQueryResult) -> Result<String, BundlebaseError> {
    let mut points = Vec::new();
    for (idx, row) in data.rows.iter().enumerate() {
        if row.len() < 2 {
            return Err(BundlebaseError::from("Each row must have at least 2 columns"));
        }
        // For line charts, if x is a string, use the row index as x
        let x = match &row[0] {
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::String(s) => {
                // Try parsing as number, fall back to index
                if s.parse::<f64>().is_ok() {
                    s.clone()
                } else {
                    idx.to_string()
                }
            }
            _ => idx.to_string(),
        };
        let y = match &row[1] {
            serde_json::Value::Number(n) => n.to_string(),
            _ => "0".to_string(),
        };
        points.push(format!("({}, {})", x, y));
    }
    Ok(format!("({})", points.join(", ")))
}

/// Extract a numeric value from JSON, falling back to "0".
fn json_number_or_zero(val: &serde_json::Value) -> String {
    match val {
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) if s.parse::<f64>().is_ok() => s.clone(),
        _ => "0".to_string(),
    }
}

/// Convert a JSON value to a Typst value representation.
fn json_to_typst_value(val: &serde_json::Value) -> String {
    match val {
        serde_json::Value::Null => "none".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => format!("\"{}\"", escape_typst(s)),
        serde_json::Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(json_to_typst_value).collect();
            format!("({})", items.join(", "))
        }
        serde_json::Value::Object(_) => "none".to_string(),
    }
}

/// Get a color list from options, falling back to default palette.
fn get_color_list(opts: &serde_yaml_ng::Value, key: &str) -> String {
    if let Some(serde_yaml_ng::Value::Sequence(colors)) = opts.get(key) {
        let items: Vec<String> = colors
            .iter()
            .map(|c| match c {
                serde_yaml_ng::Value::String(s) => format!("rgb(\"{}\")", s),
                _ => format!("rgb(\"{}\")", CHART_COLORS[0]),
            })
            .collect();
        format!("({})", items.join(", "))
    } else {
        let items: Vec<String> = CHART_COLORS.iter().map(|c| format!("rgb(\"{}\")", c)).collect();
        format!("({})", items.join(", "))
    }
}

/// Extract a string value from YAML options.
fn get_yaml_str(opts: &serde_yaml_ng::Value, key: &str) -> Option<String> {
    opts.get(key).and_then(|v| match v {
        serde_yaml_ng::Value::String(s) => Some(s.clone()),
        serde_yaml_ng::Value::Bool(b) => Some(b.to_string()),
        serde_yaml_ng::Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

/// Extract a numeric value from YAML options.
fn get_yaml_number(opts: &serde_yaml_ng::Value, key: &str) -> Option<String> {
    opts.get(key).and_then(|v| match v {
        serde_yaml_ng::Value::Number(n) => Some(n.to_string()),
        serde_yaml_ng::Value::String(s) => Some(s.clone()),
        _ => None,
    })
}

/// Extract a boolean value from YAML options.
fn get_yaml_bool(opts: &serde_yaml_ng::Value, key: &str) -> Option<bool> {
    opts.get(key).and_then(|v| match v {
        serde_yaml_ng::Value::Bool(b) => Some(*b),
        _ => None,
    })
}

/// Extract a string array from YAML options.
fn get_yaml_str_array(opts: &serde_yaml_ng::Value, key: &str) -> Option<Vec<String>> {
    opts.get(key).and_then(|v| match v {
        serde_yaml_ng::Value::Sequence(seq) => {
            let items: Vec<String> = seq
                .iter()
                .filter_map(|item| match item {
                    serde_yaml_ng::Value::String(s) => Some(s.clone()),
                    _ => None,
                })
                .collect();
            if items.is_empty() { None } else { Some(items) }
        }
        _ => None,
    })
}

/// Extract a two-element array as "a, b" string for Typst size tuples.
fn get_yaml_array_pair(opts: &serde_yaml_ng::Value, key: &str) -> Option<String> {
    opts.get(key).and_then(|v| match v {
        serde_yaml_ng::Value::Sequence(seq) if seq.len() == 2 => {
            let a = yaml_to_typst_value(&seq[0]);
            let b = yaml_to_typst_value(&seq[1]);
            Some(format!("{}, {}", a, b))
        }
        _ => None,
    })
}

/// Convert a YAML value to a Typst value representation.
fn yaml_to_typst_value(val: &serde_yaml_ng::Value) -> String {
    match val {
        serde_yaml_ng::Value::Null => "none".to_string(),
        serde_yaml_ng::Value::Bool(b) => b.to_string(),
        serde_yaml_ng::Value::Number(n) => n.to_string(),
        serde_yaml_ng::Value::String(s) => {
            // Check if it looks like a Typst expression (e.g., "1pt", "rgb(...)")
            if s.ends_with("pt") || s.ends_with("em") || s.ends_with('%') || s.starts_with("rgb") {
                s.clone()
            } else {
                format!("\"{}\"", escape_typst(s))
            }
        }
        serde_yaml_ng::Value::Sequence(seq) => {
            let items: Vec<String> = seq.iter().map(yaml_to_typst_value).collect();
            format!("({})", items.join(", "))
        }
        serde_yaml_ng::Value::Mapping(map) => yaml_mapping_to_typst_dict(map),
        serde_yaml_ng::Value::Tagged(tagged) => yaml_to_typst_value(&tagged.value),
    }
}

/// Convert a YAML value to a Typst dict.
fn yaml_to_typst_dict(val: &serde_yaml_ng::Value) -> String {
    match val {
        serde_yaml_ng::Value::Mapping(map) => yaml_mapping_to_typst_dict(map),
        _ => yaml_to_typst_value(val),
    }
}

/// Convert a YAML mapping to a Typst dict: (key: value, ...)
fn yaml_mapping_to_typst_dict(map: &serde_yaml_ng::Mapping) -> String {
    let entries: Vec<String> = map
        .iter()
        .map(|(k, v)| {
            let key = match k {
                serde_yaml_ng::Value::String(s) => s.replace('_', "-"),
                _ => k.as_str().unwrap_or("unknown").replace('_', "-"),
            };
            format!("{}: {}", key, yaml_to_typst_value(v))
        })
        .collect();
    format!("({})", entries.join(", "))
}

/// Escape special Typst characters in text content.
fn escape_typst(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('#', "\\#")
        .replace('$', "\\$")
        .replace('@', "\\@")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_data() -> BoundedQueryResult {
        BoundedQueryResult {
            columns: vec!["region".to_string(), "revenue".to_string()],
            rows: vec![
                vec![serde_json::json!("North"), serde_json::json!(1500)],
                vec![serde_json::json!("South"), serde_json::json!(1200)],
                vec![serde_json::json!("East"), serde_json::json!(900)],
            ],
        }
    }

    #[test]
    fn test_render_pie_basic() {
        let chart = ChartBlock {
            bundle: "test".to_string(),
            query: "SELECT region, revenue FROM bundle".to_string(),
            chart_type: ChartType::Pie,
            title: Some("Revenue by Region".to_string()),
            options: serde_yaml_ng::Value::Null,
        };
        let result = render_chart(&chart, &sample_data()).expect("should render");
        assert!(result.contains("piechart"));
        assert!(result.contains("Revenue by Region"));
        assert!(result.contains("\"North\""));
    }

    #[test]
    fn test_render_bar_basic() {
        let chart = ChartBlock {
            bundle: "test".to_string(),
            query: "SELECT region, revenue FROM bundle".to_string(),
            chart_type: ChartType::Bar,
            title: None,
            options: serde_yaml_ng::Value::Null,
        };
        let result = render_chart(&chart, &sample_data()).expect("should render");
        assert!(result.contains("columnchart"));
        assert!(!result.contains("caption")); // no title
    }

    #[test]
    fn test_render_line_basic() {
        let data = BoundedQueryResult {
            columns: vec!["x".to_string(), "y".to_string()],
            rows: vec![
                vec![serde_json::json!(1), serde_json::json!(10)],
                vec![serde_json::json!(2), serde_json::json!(20)],
                vec![serde_json::json!(3), serde_json::json!(15)],
            ],
        };
        let chart = ChartBlock {
            bundle: "test".to_string(),
            query: "SELECT x, y FROM bundle".to_string(),
            chart_type: ChartType::Line,
            title: Some("Trend".to_string()),
            options: serde_yaml_ng::Value::Null,
        };
        let result = render_chart(&chart, &data).expect("should render");
        assert!(result.contains("plot.plot"));
        assert!(result.contains("plot.add"));
        assert!(result.contains("Trend"));
    }

    #[test]
    fn test_render_chart_insufficient_columns() {
        let data = BoundedQueryResult {
            columns: vec!["x".to_string()],
            rows: vec![vec![serde_json::json!(1)]],
        };
        let chart = ChartBlock {
            bundle: "test".to_string(),
            query: "SELECT x FROM bundle".to_string(),
            chart_type: ChartType::Pie,
            title: None,
            options: serde_yaml_ng::Value::Null,
        };
        let result = render_chart(&chart, &data);
        assert!(result.is_err());
    }

    #[test]
    fn test_escape_typst() {
        assert_eq!(escape_typst("hello #world $100"), "hello \\#world \\$100");
    }

    #[test]
    fn test_render_horizontal_bar_basic() {
        let chart = ChartBlock {
            bundle: "test".to_string(),
            query: "SELECT region, revenue FROM bundle".to_string(),
            chart_type: ChartType::HorizontalBar,
            title: None,
            options: serde_yaml_ng::Value::Null,
        };
        let result = render_chart(&chart, &sample_data()).expect("should render");
        assert!(result.contains("barchart"));
        assert!(result.contains("\"North\""));
    }

    #[test]
    fn test_render_box_whisker_basic() {
        let data = BoundedQueryResult {
            columns: vec![
                "group".to_string(), "min".to_string(), "q1".to_string(),
                "q2".to_string(), "q3".to_string(), "max".to_string(),
            ],
            rows: vec![
                vec![
                    serde_json::json!("A"), serde_json::json!(10),
                    serde_json::json!(25), serde_json::json!(35),
                    serde_json::json!(50), serde_json::json!(60),
                ],
                vec![
                    serde_json::json!("B"), serde_json::json!(5),
                    serde_json::json!(20), serde_json::json!(30),
                    serde_json::json!(45), serde_json::json!(55),
                ],
            ],
        };
        let chart = ChartBlock {
            bundle: "test".to_string(),
            query: "SELECT * FROM bundle".to_string(),
            chart_type: ChartType::BoxWhisker,
            title: Some("Distribution".to_string()),
            options: serde_yaml_ng::Value::Null,
        };
        let result = render_chart(&chart, &data).expect("should render");
        assert!(result.contains("boxwhisker"));
        assert!(result.contains("q1: 25"));
        assert!(result.contains("q2: 35"));
        assert!(result.contains("Distribution"));
    }

    #[test]
    fn test_render_box_whisker_insufficient_columns() {
        let data = BoundedQueryResult {
            columns: vec!["a".to_string(), "b".to_string()],
            rows: vec![vec![serde_json::json!("A"), serde_json::json!(10)]],
        };
        let chart = ChartBlock {
            bundle: "test".to_string(),
            query: "SELECT * FROM bundle".to_string(),
            chart_type: ChartType::BoxWhisker,
            title: None,
            options: serde_yaml_ng::Value::Null,
        };
        let result = render_chart(&chart, &data);
        assert!(result.is_err());
    }

    #[test]
    fn test_render_pyramid_basic() {
        let data = BoundedQueryResult {
            columns: vec!["stage".to_string(), "count".to_string()],
            rows: vec![
                vec![serde_json::json!("Awareness"), serde_json::json!(1000)],
                vec![serde_json::json!("Interest"), serde_json::json!(600)],
                vec![serde_json::json!("Purchase"), serde_json::json!(200)],
            ],
        };
        let chart = ChartBlock {
            bundle: "test".to_string(),
            query: "SELECT stage, count FROM bundle".to_string(),
            chart_type: ChartType::Pyramid,
            title: Some("Funnel".to_string()),
            options: serde_yaml_ng::Value::Null,
        };
        let result = render_chart(&chart, &data).expect("should render");
        assert!(result.contains("pyramid"));
        assert!(result.contains("\"Awareness\""));
        assert!(result.contains("Funnel"));
    }

    #[test]
    fn test_render_error_bar_basic() {
        let data = BoundedQueryResult {
            columns: vec!["x".to_string(), "y".to_string(), "y_error".to_string()],
            rows: vec![
                vec![serde_json::json!(1), serde_json::json!(10), serde_json::json!(2)],
                vec![serde_json::json!(2), serde_json::json!(20), serde_json::json!(3)],
                vec![serde_json::json!(3), serde_json::json!(15), serde_json::json!(1)],
            ],
        };
        let chart = ChartBlock {
            bundle: "test".to_string(),
            query: "SELECT x, y, y_error FROM bundle".to_string(),
            chart_type: ChartType::ErrorBar,
            title: None,
            options: serde_yaml_ng::Value::Null,
        };
        let result = render_chart(&chart, &data).expect("should render");
        assert!(result.contains("plot.add-errorbar"));
        assert!(result.contains("y-error: 2"));
        assert!(result.contains("plot.add("));
    }

    #[test]
    fn test_render_error_bar_with_x_error() {
        let data = BoundedQueryResult {
            columns: vec![
                "x".to_string(), "y".to_string(),
                "y_error".to_string(), "x_error".to_string(),
            ],
            rows: vec![
                vec![
                    serde_json::json!(1), serde_json::json!(10),
                    serde_json::json!(2), serde_json::json!(0.5),
                ],
            ],
        };
        let chart = ChartBlock {
            bundle: "test".to_string(),
            query: "SELECT * FROM bundle".to_string(),
            chart_type: ChartType::ErrorBar,
            title: None,
            options: serde_yaml_ng::Value::Null,
        };
        let result = render_chart(&chart, &data).expect("should render");
        assert!(result.contains("x-error: 0.5"));
        assert!(result.contains("y-error: 2"));
    }

    #[test]
    fn test_render_error_bar_insufficient_columns() {
        let data = BoundedQueryResult {
            columns: vec!["x".to_string(), "y".to_string()],
            rows: vec![vec![serde_json::json!(1), serde_json::json!(10)]],
        };
        let chart = ChartBlock {
            bundle: "test".to_string(),
            query: "SELECT * FROM bundle".to_string(),
            chart_type: ChartType::ErrorBar,
            title: None,
            options: serde_yaml_ng::Value::Null,
        };
        let result = render_chart(&chart, &data);
        assert!(result.is_err());
    }

    #[test]
    fn test_render_violin_basic() {
        let data = BoundedQueryResult {
            columns: vec!["group".to_string(), "value".to_string()],
            rows: vec![
                vec![serde_json::json!("A"), serde_json::json!(10)],
                vec![serde_json::json!("A"), serde_json::json!(15)],
                vec![serde_json::json!("A"), serde_json::json!(12)],
                vec![serde_json::json!("B"), serde_json::json!(20)],
                vec![serde_json::json!("B"), serde_json::json!(25)],
                vec![serde_json::json!("B"), serde_json::json!(22)],
            ],
        };
        let chart = ChartBlock {
            bundle: "test".to_string(),
            query: "SELECT group, value FROM bundle".to_string(),
            chart_type: ChartType::Violin,
            title: Some("Distribution".to_string()),
            options: serde_yaml_ng::Value::Null,
        };
        let result = render_chart(&chart, &data).expect("should render");
        assert!(result.contains("plot.add-violin"));
        assert!(result.contains("side: \"both\""));
        // Group A values should be collected together
        assert!(result.contains("(1, (10, 15, 12))"));
        // Group B values should be collected together
        assert!(result.contains("(2, (20, 25, 22))"));
    }

    #[test]
    fn test_render_pie_with_options() {
        let opts: serde_yaml_ng::Value = serde_yaml_ng::from_str(
            "radius: 5\ninner_radius: 2\nslice_style: [\"#ff0000\", \"#00ff00\"]",
        )
        .expect("valid yaml");
        let chart = ChartBlock {
            bundle: "test".to_string(),
            query: "SELECT a, b FROM bundle".to_string(),
            chart_type: ChartType::Pie,
            title: None,
            options: opts,
        };
        let result = render_chart(&chart, &sample_data()).expect("should render");
        assert!(result.contains("radius: 5"));
        assert!(result.contains("inner-radius: 2"));
        assert!(result.contains("rgb(\"#ff0000\")"));
    }
}
