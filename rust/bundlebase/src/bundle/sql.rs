use super::Pack;
use datafusion::common::DataFusionError;
use datafusion::logical_expr::{Expr, LogicalPlan, Operator};
use datafusion::prelude::Expr::BinaryExpr;
use datafusion::prelude::SessionContext;
use crate::bundle::pack::JoinTypeOption;

/// The name used to reference the base pack in join expressions
pub const BASE_PACK_NAME: &str = "base";

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
    use crate::io::ObjectId;
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
