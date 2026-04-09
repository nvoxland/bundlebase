# PDF Reports

Bundlebase can generate PDF reports from a markdown template. You write a normal markdown document, and wherever you want live data from a bundle — a chart, a table — you drop in a `bundlebase` fenced code block with a SQL query and chart type. Bundlebase runs the queries and compiles everything to PDF via [Typst](https://typst.app/).

## Generating a Report

Generate a PDF from a markdown file:

```bash
bundlebase generate-report --bundle ./my-bundle --input report.md --output report.pdf
```

Or from a stored report (see [Stored Reports](#stored-reports) below):

```bash
bundlebase generate-report --bundle ./my-bundle --id monthly-sales --output report.pdf
```

| Flag | Description |
|------|-------------|
| `--bundle` | Path to the bundle (required) |
| `--input` | Path to the markdown report file (use this or `--id`) |
| `--id` | ID of a stored report in the bundle (use this or `--input`) |
| `--output` / `-o` | Output PDF path (must end in `.pdf`) |
| `--no-branding` | Omit the "Created by Bundlebase" footer |

## Report Structure

A report is a regular markdown file. Text, headings, bullet lists, and bold/italic all work as normal. To embed a chart or table, use a `bundlebase` fenced block:

````markdown
```bundlebase
bundle: path/to/my/bundle
query: SELECT region, SUM(revenue) as total FROM bundle GROUP BY region
type: bar
title: Revenue by Region
```
````

Every block requires three fields:

| Field | Description |
|-------|-------------|
| `bundle` | Path to the bundle to query |
| `query` | SQL query — `bundle` is the table name |
| `type` | `table`, `pie`, `bar`, `line`, `horizontal_bar`, `box_whisker`, `pyramid`, `error_bar`, or `violin` |

`title` is optional and adds a caption below the figure.

Tables are capped at 20 rows. Write your SQL accordingly (`ORDER BY ... LIMIT 20`).

## Tables

````markdown
```bundlebase
bundle: sales/q4
query: |
  SELECT product, units_sold, revenue
  FROM bundle
  ORDER BY revenue DESC
  LIMIT 20
type: table
title: Top Products by Revenue
options:
  zebra: true
  header_fill: "#4e79a7"
```
````

Table options:

| Option | Description | Default |
|--------|-------------|---------|
| `zebra` | Alternate row shading | `true` |
| `header_fill` | Header background color (hex) | `#e8edf2` |
| `zebra_color` | Alternating row color (hex) | `#f5f7fa` |
| `border` | Cell border style (Typst value) | `0.5pt + rgb("#cccccc")` |

## Charts

All charts need at least two columns from the query result. The first column is treated as the category/label and the second as the value, except where noted.

### Bar Chart

````markdown
```bundlebase
bundle: sales/q4
query: SELECT region, SUM(revenue) as total FROM bundle GROUP BY region ORDER BY total DESC
type: bar
title: Revenue by Region
options:
  x_label: "Region"
  y_label: "Revenue"
  size: [10, 6]
```
````

Use `type: horizontal_bar` to flip the orientation — useful for long category names.

Bar/horizontal bar options:

| Option | Description |
|--------|-------------|
| `size` | `[width, height]` in inches |
| `x_label` / `y_label` | Axis labels |
| `bar_width` | Width of each bar |
| `bar_style` | Array of hex colors |
| `mode` | `"stacked"` for stacked bars |
| `legend` | Legend configuration |

### Pie Chart

````markdown
```bundlebase
bundle: sales/q4
query: SELECT region, SUM(revenue) as total FROM bundle GROUP BY region
type: pie
title: Revenue Share by Region
options:
  radius: 5
  inner_radius: 2
  slice_style: ["#4e79a7", "#f28e2b", "#e15759", "#76b7b2", "#59a14f"]
```
````

Setting `inner_radius` makes it a donut chart. Omit it for a solid pie.

Pie options:

| Option | Description |
|--------|-------------|
| `radius` | Outer radius |
| `inner_radius` | Inner radius (donut) |
| `slice_style` | Array of hex colors |
| `outset` | Slice separation |
| `legend` | Legend configuration |

### Line Chart

````markdown
```bundlebase
bundle: sensor_data
query: SELECT month, avg_temp FROM bundle ORDER BY month
type: line
title: Average Temperature by Month
options:
  size: [12, 6]
  x_label: "Month"
  y_label: "Temperature (°C)"
  x_grid: "major"
  y_grid: "major"
  fill: true
```
````

Line chart options:

| Option | Description |
|--------|-------------|
| `size` | `[width, height]` |
| `x_label` / `y_label` | Axis labels |
| `x_min` / `x_max` / `y_min` / `y_max` | Axis range |
| `x_tick_step` / `y_tick_step` | Tick interval |
| `x_grid` / `y_grid` | `"major"`, `"minor"`, or `"both"` |
| `fill` | Fill area under line (`true`/`false`) |
| `mark` | Point marker style |
| `x_format` / `y_format` | Number format string |
| `x_decimals` / `y_decimals` | Decimal places on axis |

### Other Chart Types

**`box_whisker`** — requires six columns: `label, min, q1, median, q3, max`.

```sql
SELECT station, min_depth, q1_depth, median_depth, q3_depth, max_depth
FROM bundle
```

**`violin`** — requires two columns: `category, value`. Each unique category gets its own violin.

**`pyramid`** — two columns: `label, value`. Useful for funnel or hierarchical data. Options include `mode` (`"REGULAR"`, `"AREA-HEIGHT"`, `"HEIGHT"`, `"WIDTH"`) and `gap`.

**`error_bar`** — three or four columns: `x, y, y_error[, x_error]`.

## Default Color Palette

When `slice_style` / `bar_style` isn't set, charts use this palette in order:

`#4e79a7` · `#f28e2b` · `#e15759` · `#76b7b2` · `#59a14f` · `#edc949` · `#af7aa1` · `#ff9da7`

## Full Example

````markdown
# Q4 2024 Sales Report

Regional performance summary for the quarter.

## Revenue by Region

```bundlebase
bundle: sales/q4
query: SELECT region, SUM(revenue) as total FROM bundle GROUP BY region ORDER BY total DESC
type: bar
title: Total Revenue by Region
options:
  x_label: "Region"
  y_label: "Revenue ($)"
  size: [10, 5]
```

Revenue was up across all regions year over year, with the North leading at $4.2M.

## Market Share

```bundlebase
bundle: sales/q4
query: SELECT region, SUM(revenue) as total FROM bundle GROUP BY region
type: pie
title: Revenue Distribution
options:
  radius: 5
  inner_radius: 2
```

## Top Products

```bundlebase
bundle: sales/q4
query: SELECT product, units_sold, revenue FROM bundle ORDER BY revenue DESC LIMIT 20
type: table
title: Top 20 Products
```
````

## Stored Reports

You can save report templates inside a bundle so they travel with the data. Stored reports are identified by an **id** (alphanumeric characters, underscores, and hyphens).

### Creating a Report

````sql
CREATE REPORT monthly-sales
  NAME 'Monthly Sales Report'
  DESCRIPTION 'Regional revenue breakdown'
  CONTENT $$
# Monthly Sales

```bundlebase
bundle: .
query: SELECT region, SUM(revenue) as total FROM bundle GROUP BY region
type: bar
title: Revenue by Region
```
$$
````

The `CONTENT` field uses `$$` dollar-quoting so the markdown can contain single quotes, newlines, and any other characters without escaping.

If a report with the same id already exists, `CREATE REPORT` replaces it.

### Listing Reports

```sql
SHOW REPORTS
```

Returns `id`, `name`, and `description` for each stored report (excludes the markdown content).

To see full report data including content, query the info schema table directly:

```sql
SELECT * FROM bundle_info.reports
```

### Generating a Stored Report

From SQL (returns the PDF as a binary column):

```sql
GENERATE REPORT monthly-sales
```

From the CLI:

```bash
bundlebase generate-report --bundle ./my-bundle --id monthly-sales --output report.pdf
```

From Python:

```python
c = await bundlebase.create("/path/to/bundle")
await c.create_report(
    "monthly-sales",
    "Monthly Sales Report",
    "Regional revenue breakdown",
    markdown_content,
)
await c.commit("Add monthly sales report")

# Generate PDF
msg = await c.generate_report("monthly-sales", "report.pdf")
```

### Removing a Report

```sql
DROP REPORT monthly-sales
```
