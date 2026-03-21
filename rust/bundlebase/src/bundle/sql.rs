use super::{DataBlock, Pack};
use crate::{catalog, BundlebaseError};
use datafusion::common::DataFusionError;
use datafusion::dataframe::DataFrame;
use datafusion::logical_expr::{Expr, LogicalPlan, Operator};
use datafusion::prelude::Expr::BinaryExpr;
use datafusion::prelude::SessionContext;
use datafusion::sql::TableReference;
use std::sync::Arc;
use crate::bundle::pack::JoinTypeOption;

/// The name used to reference the base pack in join expressions
pub const BASE_PACK_NAME: &str = "base";

/// Finds the original source (table and column name) for a logical column.
///
/// Analyzes the logical execution plan to trace a column back to its physical source,
/// accounting for renames and joins. Returns all source pairs if a column comes from
/// multiple sources (e.g., via UNION). If data_packs is provided, expands pack tables
/// into their constituent block tables.
///
/// # Arguments
/// * `column_name` - The logical column name to trace
/// * `df` - The DataFrame to analyze
/// * `packs` - Optional map of packs for expanding pack tables to blocks
///
/// # Returns
/// * `Ok(Some(sources))` - List of (table_name, physical_column_name) pairs
/// * `Ok(None)` - Column not found in the schema
/// * `Err(e)` - Analysis failed
pub(crate) async fn column_sources_from_df(
    column_name: &str,
    df: &DataFrame,
    packs: Option<
        &Arc<
            parking_lot::RwLock<
                std::collections::HashMap<crate::io::ObjectId, Arc<Pack>>,
            >,
        >,
    >,
) -> Result<Option<Vec<(String, String)>>, BundlebaseError> {
    let plan = df.logical_plan();

    let mut sources = Vec::new();
    find_orig(plan, column_name, &mut sources);

    // Expand pack references to their constituent blocks if packs is provided
    let expanded_sources = if let Some(packs_map) = packs {
        let mut result = Vec::new();
        for (table_name, col_name) in sources {
            if let Some(pack_id) = Pack::parse_id(&table_name) {
                // This is a pack table - expand it to the individual blocks that have this column
                let packs = packs_map.read();
                if let Some(pack) = packs.get(&pack_id) {
                    for block in pack.blocks() {
                        // Only include this block if it actually has the column
                        let block_schema = block.schema();
                        if block_schema.column_with_name(&col_name).is_some() {
                            result.push((
                                format!("blocks.{}", DataBlock::table_name(block.id())),
                                col_name.clone(),
                            ));
                        }
                    }
                } else {
                    // Pack not found - just return the pack table name
                    result.push((table_name, col_name));
                }
            } else {
                result.push((table_name, col_name));
            }
        }
        result
    } else {
        sources
    };

    if expanded_sources.is_empty() {
        Ok(None)
    } else {
        // Preserve order of appearance in logical plan instead of sorting
        // Use HashSet-based dedup to avoid reordering
        let mut seen = std::collections::HashSet::new();
        let mut deduped = expanded_sources;
        deduped.retain(|item| seen.insert(item.clone()));
        Ok(Some(deduped))
    }
}

/// Recursively extract the underlying column name from an expression,
/// peeling through Cast, TryCast, Alias, and single-column ScalarFunction wrappers.
fn extract_source_column(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Column(col) => Some(&col.name),
        Expr::Alias(alias) => extract_source_column(&alias.expr),
        Expr::Cast(cast) => extract_source_column(&cast.expr),
        Expr::TryCast(try_cast) => extract_source_column(&try_cast.expr),
        Expr::ScalarFunction(func) => {
            // For single-column functions like regexp_replace, the first arg is the input column
            func.args.first().and_then(extract_source_column)
        }
        _ => None,
    }
}

/// Recursively traces a column back to its physical source(s) in the execution plan.
///
/// This function walks the logical execution plan to find where a column originally comes from.
/// It handles:
/// - **Projections/Aliases:** Tracks renamed columns back to their original names
/// - **Unions:** Returns all sources if a column comes from multiple tables (UNION operation)
/// - **Table Scans:** Identifies the physical table and column name
/// - **Complex Expressions:** Traces through nested references
///
/// # Algorithm
/// 1. For Projection nodes: Extract the underlying column name from the expression
/// 2. For Union nodes: Recursively search all union inputs
/// 3. For TableScan nodes: Return the physical table name and column
/// 4. For other nodes: Look for column references in expressions
/// 5. Recurse into plan inputs until reaching a table scan
///
/// # Edge Cases
/// - **Computed Columns:** Functions/expressions that don't reference a single column
///   are skipped (e.g., `SELECT col1 + col2 AS total`)
/// - **Unions with Different Column Orders:** Each source returns independently,
///   duplicates are removed by caller
/// - **Complex Table References:** Attempts to extract table name, falls back to debug format
///
/// # Arguments
/// * `plan` - The logical execution plan node to analyze
/// * `target` - The column name to find (may be an alias)
/// * `sources` - Accumulator vec of (table_name, physical_column_name) pairs
fn find_orig(plan: &LogicalPlan, target: &str, sources: &mut Vec<(String, String)>) {
    // Handle Projection nodes specially to track through aliases/renames
    if let LogicalPlan::Projection(proj) = plan {
        // Look for the target in the projection expressions
        for (i, expr) in proj.expr.iter().enumerate() {
            let output_col_name = proj.schema.field(i).name();

            if output_col_name == target {
                // Found the target, now find what it comes from
                // Handle aliases: Alias(expr, name)
                let source_expr = if let Expr::Alias(alias) = expr {
                    &alias.expr
                } else {
                    expr
                };

                // Now recursively find the source for the underlying expression,
                // tracing through Cast, TryCast, ScalarFunction wrappers
                if let Some(col_name) = extract_source_column(source_expr) {
                    find_orig(&proj.input, col_name, sources);
                }
                // Note: We ignore computed columns that don't trace to a single source column
                // e.g., "SELECT col1 + col2 AS total"
                return;
            }
        }
        // If target not found in projection outputs, recurse into input
        find_orig(&proj.input, target, sources);
        return;
    }

    // Handle Union nodes specially - search all inputs
    // A column from a UNION comes from multiple sources
    if let LogicalPlan::Union(_union) = plan {
        for input in plan.inputs() {
            find_orig(input, target, sources);
        }
        return;
    }

    // For TableScan nodes, add the table and column directly
    if let LogicalPlan::TableScan(scan) = plan {
        let table_name = scan.table_name.to_string();
        // Check if the column is in the projected schema
        for field in scan.projected_schema.fields() {
            if field.name() == target {
                sources.push((table_name, target.to_string()));
                return;
            }
        }
        return;
    }

    // For other node types, inspect expressions to find column references
    for expr in plan.expressions() {
        if let Expr::Column(col) = expr {
            if col.name == target {
                match &col.relation {
                    Some(TableReference::Bare { table }) => {
                        // Simple table reference - use directly
                        sources.push((table.to_string(), col.name.clone()));
                        return;
                    }
                    Some(other) => {
                        // Complex table reference (qualified, subquery, etc.)
                        // Try to extract a readable name
                        let table_name = match other {
                            TableReference::Bare { table } => table.to_string(),
                            TableReference::Partial { schema, table } => {
                                format!("{}.{}", schema, table)
                            }
                            TableReference::Full {
                                catalog,
                                schema,
                                table,
                            } => {
                                format!("{}.{}.{}", catalog, schema, table)
                            }
                        };
                        sources.push((table_name, col.name.clone()));
                        return;
                    }
                    None => {
                        // No relation specified - use the dataframe alias as source
                        sources.push((catalog::BUNDLE_TABLE.to_string(), col.name.clone()));
                        return;
                    }
                }
            }
        }
    }
    // Recurse into inputs to continue searching
    for input in plan.inputs() {
        find_orig(input, target, sources);
    }
}

/// Check which of the given function names appear in a SQL query.
///
/// Uses text matching to find dotted function names in the SQL string,
/// checking for both quoted (`"ns.func"(...)`) and unquoted (`ns.func(...)`) forms.
/// This is more reliable than plan-based extraction since DataFusion may interpret
/// dotted names as schema-qualified references depending on context.
pub(crate) fn find_temp_functions_in_sql(
    sql: &str,
    temp_names: &[String],
) -> Vec<String> {
    let sql_lower = sql.to_lowercase();
    temp_names
        .iter()
        .filter(|name| {
            let name_lower = name.to_lowercase();
            // Check unquoted: ns.func(
            sql_lower.contains(&name_lower)
                // Check quoted: "ns.func"(
                || sql_lower.contains(&format!("\"{}\"", name_lower))
        })
        .cloned()
        .collect()
}

/// Parse a WHERE-clause fragment (no leading `WHERE`) into one or more DataFusion `Expr` nodes.
///
/// - `ctx` is the SessionContext used to parse SQL and resolve names.
/// - `where_sql` is the condition text (e.g. `a > 10 AND b = 'x'`).
/// - `table` is the table name to use as the FROM target when the condition references columns.
///    If `None` a dummy name `t` is used (you can register a temp table with that name beforehand).
pub(crate) async fn parse_join_expr(
    ctx: &SessionContext,
    table: &str,
    pack: &Pack,
) -> Result<Vec<Expr>, DataFusionError> {
    // Pack must have join metadata
    let pack_join_type = pack.join_type().expect("Pack must have join_type for join");
    let pack_name = pack.name();
    let pack_expression = pack.expression().expect("Pack must have expression for join");

    let join_type = match pack_join_type {
        JoinTypeOption::Inner => "INNER JOIN",
        JoinTypeOption::Left => "LEFT JOIN",
        JoinTypeOption::Right => "RIGHT JOIN",
        JoinTypeOption::Full => "FULL OUTER JOIN",
    };

    let sql = format!(
        "SELECT * FROM {} AS {} {} packs.{} AS {} ON {}",
        table,
        BASE_PACK_NAME,
        join_type,
        Pack::table_name(pack.id()),
        pack_name,
        pack_expression
    );

    let df = ctx.sql(&sql).await?;
    let plan = df.logical_plan();

    let mut preds = Vec::new();
    collect_join_exprs(plan, &mut preds);
    Ok(preds)
}

fn collect_join_exprs(plan: &LogicalPlan, out: &mut Vec<Expr>) {
    match plan {
        LogicalPlan::Join(filter) => {
            match &filter.filter {
                Some(filter) => out.push(filter.clone()),
                None => {}
            }
            for (x1, x2) in &filter.on {
                out.push(BinaryExpr(datafusion::logical_expr::BinaryExpr::new(
                    Box::new(x1.clone()),
                    Operator::Eq,
                    Box::new(x2.clone()),
                )));
            }
        }
        other => {
            // recurse into any inputs (covers Projection, Join, etc.)
            for input in other.inputs() {
                collect_join_exprs(input, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::facade::BundleFacade;
    use crate::io::ObjectId;
    use crate::test_utils::test_datafile;
    use crate::BundleBuilder;
    use arrow_schema::{DataType, Field, Schema, SchemaRef};
    use datafusion::catalog::SchemaProvider;
    use datafusion::datasource::empty::EmptyTable;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_parse_join() {
        let ctx = SessionContext::new();
        ctx.register_table(
            "t",
            Arc::new(EmptyTable::new(SchemaRef::new(Schema::new(vec![
                Field::new("a", DataType::Int32, false),
                Field::new("b", DataType::Utf8, false),
            ])))),
        )
        .unwrap();

        let join_id = ObjectId::generate();

        // Create and register a packs schema for testing
        use datafusion::catalog::MemorySchemaProvider;
        let packs_schema = Arc::new(MemorySchemaProvider::new());

        ctx.catalog("datafusion")
            .unwrap()
            .register_schema("packs", packs_schema.clone())
            .unwrap();

        packs_schema
            .register_table(
                Pack::table_name(&join_id).to_string(),
                Arc::new(EmptyTable::new(SchemaRef::new(Schema::new(vec![
                    Field::new("x", DataType::Int32, false),
                    Field::new("y", DataType::Utf8, false),
                ])))),
            )
            .unwrap();

        let pack = Pack::new(join_id, "test_join", "a=x", JoinTypeOption::Inner);
        let preds = parse_join_expr(
            &ctx,
            "t",
            &pack,
        )
        .await
        .unwrap()
        .iter()
        .map(|pred| format!("{:?}", pred))
        .collect::<Vec<_>>()
        .join("\n");
        assert_eq!("BinaryExpr(BinaryExpr { left: Column(Column { relation: Some(Bare { table: \"base\" }), name: \"a\" }), op: Eq, right: Column(Column { relation: Some(Bare { table: \"test_join\" }), name: \"x\" }) })",
                   preds.as_str());

        let pack2 = Pack::new(join_id, "test_join", "a=x and x > 3", JoinTypeOption::Inner);
        let preds = parse_join_expr(
            &ctx,
            "t",
            &pack2,
        )
        .await
        .unwrap()
        .iter()
        .map(|pred| format!("{:?}", pred))
        .collect::<Vec<_>>()
        .join("\n");
        assert_eq!("BinaryExpr(BinaryExpr { left: BinaryExpr(BinaryExpr { left: Column(Column { relation: Some(Bare { table: \"base\" }), name: \"a\" }), op: Eq, right: Column(Column { relation: Some(Bare { table: \"test_join\" }), name: \"x\" }) }), op: And, right: BinaryExpr(BinaryExpr { left: Column(Column { relation: Some(Bare { table: \"test_join\" }), name: \"x\" }), op: Gt, right: Literal(Int64(3), None) }) })",
                   preds.as_str());
    }

    #[test]
    fn test_extract_source_column_from_plain_column() {
        let expr = Expr::Column(datafusion::common::Column::new_unqualified("my_col"));
        assert_eq!(extract_source_column(&expr), Some("my_col"));
    }

    #[test]
    fn test_extract_source_column_from_alias() {
        use datafusion::logical_expr::expr::Alias;
        let inner = Expr::Column(datafusion::common::Column::new_unqualified("orig"));
        let expr = Expr::Alias(Alias {
            expr: Box::new(inner),
            relation: None,
            name: "renamed".to_string(),
            metadata: None,
        });
        assert_eq!(extract_source_column(&expr), Some("orig"));
    }

    #[test]
    fn test_extract_source_column_from_cast() {
        use datafusion::logical_expr::expr::Cast;
        let inner = Expr::Column(datafusion::common::Column::new_unqualified("id_col"));
        let expr = Expr::Cast(Cast {
            expr: Box::new(inner),
            data_type: DataType::Int64,
        });
        assert_eq!(extract_source_column(&expr), Some("id_col"));
    }

    #[test]
    fn test_extract_source_column_from_alias_cast() {
        use datafusion::logical_expr::expr::{Alias, Cast};
        let col = Expr::Column(datafusion::common::Column::new_unqualified("id_col"));
        let cast = Expr::Cast(Cast {
            expr: Box::new(col),
            data_type: DataType::Int64,
        });
        let expr = Expr::Alias(Alias {
            expr: Box::new(cast),
            relation: None,
            name: "id_col".to_string(),
            metadata: None,
        });
        assert_eq!(extract_source_column(&expr), Some("id_col"));
    }

    #[test]
    fn test_extract_source_column_from_try_cast() {
        use datafusion::logical_expr::expr::TryCast;
        let inner = Expr::Column(datafusion::common::Column::new_unqualified("val"));
        let expr = Expr::TryCast(TryCast {
            expr: Box::new(inner),
            data_type: DataType::Float64,
        });
        assert_eq!(extract_source_column(&expr), Some("val"));
    }

    #[test]
    fn test_extract_source_column_from_scalar_function() {
        use datafusion::logical_expr::expr::ScalarFunction;
        use datafusion::functions::regex::regexp_replace;
        let col = Expr::Column(datafusion::common::Column::new_unqualified("dirty_col"));
        let func = regexp_replace();
        let expr = Expr::ScalarFunction(ScalarFunction {
            func: func,
            args: vec![
                col,
                Expr::Literal(datafusion::common::ScalarValue::Utf8(Some("[^0-9]".to_string())), None),
                Expr::Literal(datafusion::common::ScalarValue::Utf8(Some("".to_string())), None),
            ],
        });
        assert_eq!(extract_source_column(&expr), Some("dirty_col"));
    }

    #[test]
    fn test_extract_source_column_returns_none_for_literal() {
        let expr = Expr::Literal(datafusion::common::ScalarValue::Int32(Some(42)), None);
        assert_eq!(extract_source_column(&expr), None);
    }

    #[tokio::test]
    async fn test_column_source_from_dataframe() -> Result<(), BundlebaseError> {
        let bundle = BundleBuilder::create("memory:///test_bundle", None).await?;
        bundle.attach(test_datafile("userdata.parquet"), None).await?;

        let df = bundle.dataframe().await?;

        // Test with pack expansion - should return only blocks that have the column
        let binding = bundle.bundle();
        let sources = column_sources_from_df("first_name", &df, Some(&binding.packs))
            .await?
            .ok_or("Could not find columns")?;

        assert!(!sources.is_empty(), "Expected at least one source");
        let (table, col) = sources.get(0).unwrap();
        // The table should be in the blocks schema
        assert!(
            table.starts_with("blocks.__block_"),
            "Expected table to start with 'blocks.__block_' but got: '{}'",
            table
        );
        assert_eq!("first_name", col);

        Ok(())
    }

    #[test]
    fn test_find_temp_functions_in_sql_unquoted() {
        let temp_names = vec!["test.double_val".to_string()];
        let matches = find_temp_functions_in_sql(
            "SELECT * FROM bundle WHERE test.double_val(id) > 10",
            &temp_names,
        );
        assert_eq!(matches, vec!["test.double_val"]);
    }

    #[test]
    fn test_find_temp_functions_in_sql_quoted() {
        let temp_names = vec!["test.double_val".to_string()];
        let matches = find_temp_functions_in_sql(
            "SELECT \"test.double_val\"(id) FROM bundle",
            &temp_names,
        );
        assert_eq!(matches, vec!["test.double_val"]);
    }

    #[test]
    fn test_find_temp_functions_in_sql_no_match() {
        let temp_names = vec!["test.double_val".to_string()];
        let matches = find_temp_functions_in_sql(
            "SELECT * FROM bundle WHERE id > 10",
            &temp_names,
        );
        assert!(matches.is_empty());
    }

    #[test]
    fn test_find_temp_functions_in_sql_case_insensitive() {
        let temp_names = vec!["Test.Double_Val".to_string()];
        let matches = find_temp_functions_in_sql(
            "SELECT * FROM bundle WHERE test.double_val(id) > 10",
            &temp_names,
        );
        assert_eq!(matches, vec!["Test.Double_Val"]);
    }

    #[test]
    fn test_find_temp_functions_in_sql_multiple() {
        let temp_names = vec![
            "test.func_a".to_string(),
            "test.func_b".to_string(),
            "test.func_c".to_string(),
        ];
        let matches = find_temp_functions_in_sql(
            "SELECT test.func_a(id), test.func_c(name) FROM bundle",
            &temp_names,
        );
        assert_eq!(matches.len(), 2);
        assert!(matches.contains(&"test.func_a".to_string()));
        assert!(matches.contains(&"test.func_c".to_string()));
    }
}
