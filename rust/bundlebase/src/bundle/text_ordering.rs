//! Guard against comparing a text column to a number with an ordering
//! operator — DataFusion silently compares such pairs lexicographically.

use crate::BundlebaseError;
use datafusion::logical_expr::{Expr, LogicalPlan};
use datafusion::scalar::ScalarValue;
use std::collections::HashMap;

/// Reject an ordering comparison (`>`, `>=`, `<`, `<=`, `BETWEEN`) between a
/// text (Utf8) column and a numeric literal — e.g. `WHERE amount > 100` where
/// `amount` came from a CSV and is therefore text.
///
/// This is the *silent* half of the all-Utf8 footgun. DataFusion does not fail
/// here: it coerces the number to a string and compares lexicographically, so
/// `"9" > "50"` is true and the query quietly returns the wrong rows. Since
/// CSV/TSV/JSONL columns are always imported as text, writing a bare numeric
/// bound against one is a mistake essentially every time, so we fail loudly and
/// say how to fix it.
///
/// Deliberately limited to *ordering* comparisons. Equality is left alone:
/// `"amount" = 100` compares against the literal's text form, which behaves
/// sensibly, and failing `WHERE id = 5` over a text id would break working
/// queries for no correctness gain.
///
/// Column types are resolved qualifier-aware against each node's *input*
/// schema, so an explicit cast (`FROM (SELECT CAST(x AS DOUBLE) AS x FROM
/// bundle) WHERE x > 100`) is left alone, and in a join `b.amount > 5` is
/// judged by `b.amount`'s type even when `a.amount` is text. Subquery plans
/// (`IN (SELECT ...)`, `EXISTS`, scalar subqueries) are checked too.
///
/// Must run before DataFusion's type-coercion analyzer, which rewrites the
/// numeric literal to a string and hides the mistake.
pub(crate) fn check_text_ordering_comparison(plan: &LogicalPlan) -> Result<(), BundlebaseError> {
    check_text_ordering_comparison_named(plan, &HashMap::new())
}

/// [`check_text_ordering_comparison`] for plans written against internal
/// column names (operation plans): `display_names` maps an internal name to
/// the user-visible one, so the error names the column the user wrote.
pub(crate) fn check_text_ordering_comparison_named(
    plan: &LogicalPlan,
    display_names: &HashMap<String, String>,
) -> Result<(), BundlebaseError> {
    match find_text_ordering_comparison(plan)? {
        None => Ok(()),
        Some((col, lit)) => {
            let col = display_names.get(&col).cloned().unwrap_or(col);
            Err(text_ordering_error(&col, &lit))
        }
    }
}

/// Find the first `text_col <op> number` ordering comparison in `plan` or any
/// of its subqueries; returns the column's name and the literal's text.
fn find_text_ordering_comparison(
    plan: &LogicalPlan,
) -> Result<Option<(String, String)>, BundlebaseError> {
    use arrow_schema::DataType;
    use datafusion::common::tree_node::{TreeNode, TreeNodeRecursion};
    use datafusion::common::{Column, DFSchema};
    use datafusion::logical_expr::{Between, BinaryExpr, Operator};

    fn is_numeric_literal(sv: &ScalarValue) -> bool {
        matches!(
            sv,
            ScalarValue::Int8(_)
                | ScalarValue::Int16(_)
                | ScalarValue::Int32(_)
                | ScalarValue::Int64(_)
                | ScalarValue::UInt8(_)
                | ScalarValue::UInt16(_)
                | ScalarValue::UInt32(_)
                | ScalarValue::UInt64(_)
                | ScalarValue::Float32(_)
                | ScalarValue::Float64(_)
                | ScalarValue::Decimal128(..)
                | ScalarValue::Decimal256(..)
        )
    }

    fn is_text(dt: &DataType) -> bool {
        matches!(
            dt,
            DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View
        )
    }

    let mut offender: Option<(String, String)> = None;

    plan.apply_with_subqueries(|node| {
        // A node's expressions are written against its inputs' schemas
        // (against its own schema only when it has no inputs).
        let inputs = node.inputs();
        let schemas: Vec<&DFSchema> = if inputs.is_empty() {
            vec![node.schema()]
        } else {
            inputs.iter().map(|i| i.schema().as_ref()).collect()
        };
        // Qualifier-aware: `b.amount` resolves to b's field, never a's. An
        // unresolvable (e.g. ambiguous) reference is not judged.
        let col_is_text = |c: &Column| {
            schemas.iter().any(|s| {
                s.qualified_field_from_column(c)
                    .is_ok_and(|(_, f)| is_text(f.data_type()))
            })
        };
        let offending = |col: &Expr, bound: &Expr| -> Option<(String, String)> {
            match (col, bound) {
                (Expr::Column(c), Expr::Literal(sv, _))
                    if is_numeric_literal(sv) && col_is_text(c) =>
                {
                    Some((c.name.clone(), sv.to_string()))
                }
                _ => None,
            }
        };

        for expr in node.expressions() {
            expr.apply(|e| {
                let found = match e {
                    Expr::BinaryExpr(BinaryExpr {
                        left,
                        op: Operator::Gt | Operator::GtEq | Operator::Lt | Operator::LtEq,
                        right,
                    }) => {
                        // Either orientation: `col > 100` or `100 < col`.
                        offending(left, right).or_else(|| offending(right, left))
                    }
                    Expr::Between(Between {
                        expr, low, high, ..
                    }) => offending(expr, low).or_else(|| offending(expr, high)),
                    _ => None,
                };
                Ok(match found {
                    Some(f) => {
                        offender = Some(f);
                        TreeNodeRecursion::Stop
                    }
                    None => TreeNodeRecursion::Continue,
                })
            })?;
            if offender.is_some() {
                break;
            }
        }
        Ok(if offender.is_some() {
            TreeNodeRecursion::Stop
        } else {
            TreeNodeRecursion::Continue
        })
    })?;

    Ok(offender)
}

fn text_ordering_error(col: &str, lit: &str) -> BundlebaseError {
    BundlebaseError::from(format!(
        "Column \"{col}\" is text (Utf8) but is being compared to the number {lit}. Text \
             compares lexicographically — \"9\" > \"50\" — so this would silently return the \
             wrong rows. Bundlebase imports CSV, TSV, and JSONL columns as text.\n\n  \
             • To compare numerically, cast the column:   CAST COLUMN \"{col}\" TO DOUBLE\n  \
             • To compare as text on purpose, quote it:   \"{col}\" > '{lit}'"
    ))
}

#[cfg(test)]
mod text_ordering_tests {
    use super::*;
    use arrow::array::{Int64Array, RecordBatch, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};
    use datafusion::prelude::SessionContext;
    use std::sync::Arc;

    /// `a(id, amount: Utf8)` and `b(id, amount: Int64)`.
    async fn plan(sql: &str) -> LogicalPlan {
        let ctx = SessionContext::new();
        let a = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("amount", DataType::Utf8, false),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![1])),
                Arc::new(StringArray::from(vec!["9"])),
            ],
        )
        .expect("batch a");
        let b = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("amount", DataType::Int64, false),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![1])),
                Arc::new(Int64Array::from(vec![9])),
            ],
        )
        .expect("batch b");
        ctx.register_batch("a", a).expect("register a");
        ctx.register_batch("b", b).expect("register b");
        ctx.state().create_logical_plan(sql).await.expect("plan")
    }

    fn rejected(plan: &LogicalPlan) -> bool {
        check_text_ordering_comparison(plan).is_err()
    }

    #[tokio::test]
    async fn join_resolves_column_by_qualifier() {
        let numeric_side = plan("SELECT * FROM a JOIN b ON a.id = b.id WHERE b.amount > 5").await;
        assert!(
            !rejected(&numeric_side),
            "b.amount is Int64 — must not be judged by a.amount's Utf8 type"
        );
        let text_side = plan("SELECT * FROM a JOIN b ON a.id = b.id WHERE a.amount > 5").await;
        assert!(rejected(&text_side), "a.amount is text");
    }

    #[tokio::test]
    async fn between_numeric_bounds_on_text_is_rejected() {
        assert!(rejected(
            &plan("SELECT * FROM a WHERE amount BETWEEN 1 AND 100").await
        ));
        assert!(!rejected(
            &plan("SELECT * FROM b WHERE amount BETWEEN 1 AND 100").await
        ));
        assert!(!rejected(
            &plan("SELECT * FROM a WHERE amount BETWEEN '1' AND '100'").await
        ));
    }

    #[tokio::test]
    async fn subqueries_are_checked() {
        assert!(rejected(
            &plan("SELECT * FROM b WHERE id IN (SELECT id FROM a WHERE amount > 5)").await
        ));
        assert!(rejected(
            &plan("SELECT * FROM b WHERE EXISTS (SELECT 1 FROM a WHERE a.amount > 5)").await
        ));
        assert!(rejected(
            &plan("SELECT (SELECT COUNT(*) FROM a WHERE amount < 3) AS n FROM b").await
        ));
        assert!(!rejected(
            &plan("SELECT * FROM a WHERE id IN (SELECT id FROM b WHERE amount > 5)").await
        ));
    }

    #[tokio::test]
    async fn named_variant_reports_display_name() {
        let p = plan("SELECT * FROM a WHERE amount > 5").await;
        let names = HashMap::from([("amount".to_string(), "Total Amount".to_string())]);
        let err = check_text_ordering_comparison_named(&p, &names)
            .expect_err("text ordering must be rejected")
            .to_string();
        assert!(err.contains("\"Total Amount\""), "got: {err}");
    }
}
