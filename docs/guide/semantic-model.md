---
seo_title: "Semantic Model: Descriptions, Metrics, and AI Context - Bundlebase"
---

# Semantic Model

A bundle carries column meanings, named metric definitions, AI context, and declared keys with its data. These edits use the same change history as transformations: commit them, share the bundle, and readers see the same definitions.

## Read and edit directly

Read the current model, including pending edits:

```sql
SHOW SEMANTIC MODEL;
```

The result uses [Apache Ossie](https://github.com/apache/ossie)'s document format. You can edit individual properties without handling that document:

```sql
ALTER COLUMN amount SET DESCRIPTION 'Order total in USD, net of refunds';
ALTER COLUMN amount SET LABEL 'Net order total';
ALTER DATASET bundle SET DESCRIPTION 'One row per order';

CREATE METRIC total_revenue AS 'SUM(amount)' DESCRIPTION 'Net revenue in USD';
ALTER METRIC total_revenue SET EXPRESSION 'SUM(amount - discount)';
ALTER METRIC total_revenue SET DESCRIPTION 'Net revenue after discounts';
```

Changing an expression preserves its description and AI context. Column descriptions, labels, and metric references follow column renames. A dropped column's metadata disappears from the current model; a metric referring to that column remains listed but fails when its expression is planned for use.

Use explicit `DROP` clauses to clear properties:

```sql
ALTER COLUMN amount DROP LABEL;
ALTER COLUMN amount DROP DESCRIPTION;
ALTER METRIC total_revenue DROP DESCRIPTION;
DROP METRIC total_revenue;
```

The final statement removes the metric and its metadata. Creating the name again starts fresh. Model description (`SET DESCRIPTION`) and dataset description (`ALTER DATASET bundle SET DESCRIPTION`) are separate properties.

## Add AI context

AI context accepts a string or an object. The standard object members are `instructions` (a string), `synonyms` (an array of strings), and `examples` (an array of strings). You can add custom members too.

```sql
ALTER SEMANTIC MODEL SET AI CONTEXT '{"instructions":"Use committed metric definitions for reporting"}';
ALTER COLUMN amount SET AI CONTEXT '{"synonyms":["net order total"],"currency":"USD"}';
ALTER METRIC total_revenue SET AI CONTEXT MEMBER examples '["Revenue by region"]';
ALTER DATASET bundle SET AI CONTEXT MEMBER freshness 'Updated daily';

ALTER COLUMN amount DROP AI CONTEXT MEMBER currency;
ALTER DATASET bundle DROP AI CONTEXT;
```

`SET AI CONTEXT` replaces the whole value. `SET AI CONTEXT MEMBER` writes one top-level member; an array is replaced as a whole. Setting a member after a string or cleared context creates an object containing that member. Dropping a member from a string or cleared context does nothing.

An omitted property means unchanged. Clearing uses `DROP`; JSON `null` is accepted only as a value where that member's schema permits it.

## Declare keys

```sql
DECLARE PRIMARY KEY (order_id);
DECLARE UNIQUE KEY (customer_id, order_date);
DROP UNIQUE KEY (order_date, customer_id);
DROP PRIMARY KEY;
```

Keys describe the intended grain. They do not scan data or enforce uniqueness. There is one primary key, whose column order is preserved. Unique keys match by their member columns, so the reversed order above removes the same declaration. A key disappears from the current model if any member column is dropped.

Explicit native commands record writes even when the local value already matches. They participate in normal commit-order replay with other writers. Document application instead compares against current state and records only the differences.

## Exchange JSON or YAML

Documents are an optional way to read or batch-edit the same metadata:

```sql
EXPORT SEMANTIC MODEL TO 'sales.yaml';
IMPORT SEMANTIC MODEL FROM 'sales.yaml';
IMPORT SEMANTIC MODEL FROM 'sales.yaml' MERGE;
IMPORT SEMANTIC MODEL FROM 'sales.yaml' REPLACE;
```

The `.json`, `.yaml`, or `.yml` extension selects the format. Schema validation uses a bundled Ossie revision and requires no schema downloads. Reads and exports work on a read-only bundle and record no changes.

`MERGE` is the default. It updates supplied properties and leaves omitted properties, columns' metadata, metrics, and key declarations unchanged. For an existing AI context object, supplied members are updated independently and omitted members remain.

For example, with the bundle named `sales`, importing this document changes only the expression of an existing `total_revenue` metric, preserving its description and AI context:

```yaml
version: 0.2.0.dev0
name: sales
datasets:
  - name: bundle
    source: bundle
metrics:
  - name: total_revenue
    expression:
      dialects:
        - dialect: ANSI_SQL
          expression: SUM(amount - discount)
```

`REPLACE` explicitly selects all supported metadata in the opened model. Omitted descriptions, labels, AI context, and keys are cleared; omitted metrics are removed. Physical columns remain. To replace safely, start from `SHOW` or an export and review the complete document before applying it. The adapter lowers the difference into individual property operations inside one change. Invalid input preserves pending work, an identical document records no change, and undo removes the whole import batch before commit.

The Ossie view and export require supported ANSI SQL syntax and built-in functions. Imported functions and expressions that cannot be represented in that subset produce an actionable export error. The native metric remains readable and computable when valid in Bundlebase. The adapter does not translate SQL dialects.

The application subset covers the opened bundle's single dataset. It rejects unknown columns, computed field expressions, datatype changes, dimension metadata, and relationships. Declaring relationships does not create joins. Generated Bundlebase provenance is informational and does not change bundle history on import.

## Python updates

The async API returns a snapshot as a dictionary and accepts a dictionary for a batch update:

```python
model = await bundle.semantic_model()
model["datasets"][0]["description"] = "One row per order"

changes = await builder.preview_semantic_model(model)
for operation in changes:
    print(operation)

await builder.apply_semantic_model(model)  # merge supplied properties
await builder.apply_semantic_model(model, replace=True)  # explicit full scope
await builder.export_semantic_model("sales.yaml")
await builder.import_semantic_model("sales.yaml")
```

Snapshots are independent values. Editing a dictionary changes the bundle only when you apply it. Preview returns the ordinary operations that the document would record and leaves pending work unchanged. Applying the batch does not commit it.

Common property edits also have direct methods:

```python
await builder.set_column_label("amount", "Net order total")
await builder.set_metric_expression("total_revenue", "SUM(amount - discount)")
await builder.set_metric_description("total_revenue", "Revenue after discounts")
await builder.set_ai_context("column", {"synonyms": ["net total"]}, name="amount")
await builder.set_ai_context("metric", ["Revenue by region"],
                             name="total_revenue", member="examples")
await builder.set_ai_context("dataset", "Updated daily", member="freshness")
await builder.set_column_label("amount", None)  # explicit clear
```

`set_ai_context` accepts the targets `model`, `dataset`, `column`, and `metric`. Column and metric targets require `name`. With `member`, it sets or removes that object member; without it, it replaces or clears the whole context. `None` clears the selected property.

Run native commands from Python with `execute`:

```python
await builder.execute("DECLARE PRIMARY KEY (order_id)")
await builder.execute("ALTER DATASET bundle SET DESCRIPTION 'One row per order'")
```

The sync API exposes the same calls without `await`.

## Evaluate a metric

Metric expressions are checked against the current schema and available functions when authored. Computation uses the saved definition:

=== "Async API"

    ```python
    result = await bundle.compute_metric("total_revenue", ["region"])
    df = await result.to_pandas()
    ```

=== "Sync API"

    ```python
    df = bundle.compute_metric("total_revenue", ["region"]).to_pandas()
    ```

## History and other readers

Semantic edits replay in normal manifest order. Later writes to the same property win. Edits to independent properties survive together, such as one writer changing a metric's expression and another changing its description. Whole AI context replacement overlaps its member edits. This uses replay order, not client timestamps.

Python readers and MCP use the same resolved state. Arrow field metadata exposes column descriptions under `description` for BI clients. See the [MCP guide](cli-mcp.md) and [Arrow Flight SQL guide](cli-serve.md#column-descriptions-in-bi-tools).
