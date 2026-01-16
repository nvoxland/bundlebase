use crate::index::IndexedValue;
use crate::BundlebaseError;
use datafusion::logical_expr::{expr, BinaryExpr, Expr, Operator};

/// Represents the type of index predicate extracted from a filter expression
#[derive(Debug, Clone, PartialEq)]
pub enum IndexPredicate {
    /// Equality predicate: column = value
    Exact(IndexedValue),

    /// IN predicate: column IN (value1, value2, ...)
    In(Vec<IndexedValue>),

    /// Range predicate: column >= min AND column <= max
    Range {
        min: IndexedValue,
        max: IndexedValue,
    },
}

/// Represents a filter expression that can be optimized with an index
#[derive(Debug, Clone)]
pub struct IndexableFilter {
    /// The column name being filtered
    pub column: String,

    /// The predicate type
    pub predicate: IndexPredicate,
}

/// Analyzes DataFusion filter expressions to extract index-optimizable predicates
pub struct FilterAnalyzer;

impl FilterAnalyzer {
    /// Extract all indexable filter predicates from a list of filter expressions
    ///
    /// Supports:
    /// - Equality: `column = literal` → IndexPredicate::Exact
    /// - IN lists: `column IN (lit1, lit2, ...)` → IndexPredicate::In
    /// - Ranges: `column >= min AND column <= max` → IndexPredicate::Range (future)
    ///
    /// Returns a vector of IndexableFilter for each supported predicate found.
    /// Complex expressions, OR conditions, and function calls are ignored.
    pub fn extract_indexable(filters: &[Expr]) -> Vec<IndexableFilter> {
        filters
            .iter()
            .filter_map(|expr| Self::analyze_expr(expr).ok())
            .collect()
    }

    /// Analyze a single expression to determine if it's indexable
    fn analyze_expr(expr: &Expr) -> Result<IndexableFilter, BundlebaseError> {
        match expr {
            // Handle equality: column = literal
            Expr::BinaryExpr(BinaryExpr {
                left,
                op: Operator::Eq,
                right,
            }) => Self::extract_equality(left, right),

            // Handle IN list: column IN (literal1, literal2, ...)
            Expr::InList(expr::InList {
                expr,
                list,
                negated,
            }) if !negated => Self::extract_in_list(expr, list),

            // Handle range predicates: column >= min, column > min, column <= max, column < max
            Expr::BinaryExpr(BinaryExpr {
                left,
                op: op @ (Operator::Gt | Operator::GtEq | Operator::Lt | Operator::LtEq),
                right,
            }) => Self::extract_range_single(left, *op, right),

            // Handle AND expressions that might combine range predicates
            Expr::BinaryExpr(BinaryExpr {
                left,
                op: Operator::And,
                right,
            }) => Self::extract_range_and(left, right),

            _ => Err("Expression is not indexable".into()),
        }
    }

    /// Extract equality predicate: column = literal
    fn extract_equality(
        left: &Expr,
        right: &Expr,
    ) -> Result<IndexableFilter, BundlebaseError> {
        // Try left = literal
        if let (Expr::Column(col), Expr::Literal(scalar, _)) = (left, right) {
            let indexed_value = IndexedValue::from_scalar(scalar)?;
            return Ok(IndexableFilter {
                column: col.name.clone(),
                predicate: IndexPredicate::Exact(indexed_value),
            });
        }

        // Try literal = right (reversed)
        if let (Expr::Literal(scalar, _), Expr::Column(col)) = (left, right) {
            let indexed_value = IndexedValue::from_scalar(scalar)?;
            return Ok(IndexableFilter {
                column: col.name.clone(),
                predicate: IndexPredicate::Exact(indexed_value),
            });
        }

        Err("Equality expression does not match column = literal pattern".into())
    }

    /// Extract IN list predicate: column IN (literal1, literal2, ...)
    fn extract_in_list(
        expr: &Expr,
        list: &[Expr],
    ) -> Result<IndexableFilter, BundlebaseError> {
        // Check if expr is a column reference
        if let Expr::Column(col) = expr {
            // Extract all literals from the list
            let mut values = Vec::new();
            for item in list {
                if let Expr::Literal(scalar, _) = item {
                    let indexed_value = IndexedValue::from_scalar(scalar)?;
                    values.push(indexed_value);
                } else {
                    return Err("IN list contains non-literal expression".into());
                }
            }

            if values.is_empty() {
                return Err("IN list is empty".into());
            }

            return Ok(IndexableFilter {
                column: col.name.clone(),
                predicate: IndexPredicate::In(values),
            });
        }

        Err("IN list expression does not have column reference".into())
    }

    /// Extract single range predicate: column > min, column >= min, column < max, column <= max
    /// Returns a Range with appropriate bounds
    fn extract_range_single(
        left: &Expr,
        op: Operator,
        right: &Expr,
    ) -> Result<IndexableFilter, BundlebaseError> {
        // Try column OP literal
        if let (Expr::Column(col), Expr::Literal(scalar, _)) = (left, right) {
            let indexed_value = IndexedValue::from_scalar(scalar)?;
            let predicate = match op {
                Operator::Gt | Operator::GtEq => {
                    // column >= min means range from min to max possible value
                    let max = indexed_value.max_for_type();
                    IndexPredicate::Range {
                        min: indexed_value,
                        max,
                    }
                }
                Operator::Lt | Operator::LtEq => {
                    // column <= max means range from min possible value to max
                    let min = indexed_value.min_for_type();
                    IndexPredicate::Range {
                        min,
                        max: indexed_value,
                    }
                }
                _ => return Err("Unsupported operator for range".into()),
            };

            return Ok(IndexableFilter {
                column: col.name.clone(),
                predicate,
            });
        }

        // Try literal OP column (reversed)
        if let (Expr::Literal(scalar, _), Expr::Column(col)) = (left.as_ref(), right.as_ref()) {
            let indexed_value = IndexedValue::from_scalar(scalar)?;
            let predicate = match op {
                Operator::Lt | Operator::LtEq => {
                    // min < column means range from min to max possible value
                    let max = indexed_value.max_for_type();
                    IndexPredicate::Range {
                        min: indexed_value,
                        max,
                    }
                }
                Operator::Gt | Operator::GtEq => {
                    // max > column means range from min possible value to max
                    let min = indexed_value.min_for_type();
                    IndexPredicate::Range {
                        min,
                        max: indexed_value,
                    }
                }
                _ => return Err("Unsupported operator for range".into()),
            };

            return Ok(IndexableFilter {
                column: col.name.clone(),
                predicate,
            });
        }

        Err("Range expression does not match column OP literal pattern".into())
    }

    /// Extract range predicate from AND expression: column >= min AND column <= max
    fn extract_range_and(
        left: &Expr,
        right: &Expr,
    ) -> Result<IndexableFilter, BundlebaseError> {
        // Try to extract range predicates from both sides
        let left_filter = Self::analyze_expr(left);
        let right_filter = Self::analyze_expr(right);

        match (left_filter, right_filter) {
            (Ok(left_f), Ok(right_f)) => {
                // Both sides are indexable - check if they can be combined into a range
                if left_f.column == right_f.column {
                    match (&left_f.predicate, &right_f.predicate) {
                        (
                            IndexPredicate::Range {
                                min: min1,
                                max: max1,
                            },
                            IndexPredicate::Range {
                                min: min2,
                                max: max2,
                            },
                        ) => {
                            // Combine two range predicates
                            // Use the more restrictive min and max
                            let min = if min1 > min2 {
                                min1.clone()
                            } else {
                                min2.clone()
                            };
                            let max = if max1 < max2 {
                                max1.clone()
                            } else {
                                max2.clone()
                            };

                            return Ok(IndexableFilter {
                                column: left_f.column,
                                predicate: IndexPredicate::Range { min, max },
                            });
                        }
                        _ => {
                            // Not both ranges - return the first indexable filter
                            return Ok(left_f);
                        }
                    }
                }
                // Different columns - return the first one
                Ok(left_f)
            }
            (Ok(filter), Err(_)) => Ok(filter),
            (Err(_), Ok(filter)) => Ok(filter),
            (Err(_), Err(_)) => Err("AND expression has no indexable predicates".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::column_index::OrderedFloat;
    use datafusion::common::Column;
    use datafusion::common::ScalarValue;
    use datafusion::logical_expr::col;

    #[test]
    fn test_extract_equality_column_first() {
        // Test: email = 'test@example.com'
        let expr = col("email").eq(Expr::Literal(
            ScalarValue::Utf8(Some("test@example.com".to_string())),
            None,
        ));

        let filters = vec![expr];
        let indexable = FilterAnalyzer::extract_indexable(&filters);

        assert_eq!(indexable.len(), 1);
        assert_eq!(indexable[0].column, "email");
        assert!(matches!(indexable[0].predicate, IndexPredicate::Exact(_)));
    }

    #[test]
    fn test_extract_equality_literal_first() {
        // Test: 'value' = column (reversed)
        let expr = Expr::BinaryExpr(BinaryExpr {
            left: Box::new(Expr::Literal(
                ScalarValue::Utf8(Some("value".to_string())),
                None,
            )),
            op: Operator::Eq,
            right: Box::new(Expr::Column(Column::from_name("name"))),
        });

        let filters = vec![expr];
        let indexable = FilterAnalyzer::extract_indexable(&filters);

        assert_eq!(indexable.len(), 1);
        assert_eq!(indexable[0].column, "name");
    }

    #[test]
    fn test_extract_in_list() {
        // Test: status IN ('active', 'pending', 'approved')
        let expr = Expr::InList(expr::InList {
            expr: Box::new(Expr::Column(Column::from_name("status"))),
            list: vec![
                Expr::Literal(ScalarValue::Utf8(Some("active".to_string())), None),
                Expr::Literal(ScalarValue::Utf8(Some("pending".to_string())), None),
                Expr::Literal(ScalarValue::Utf8(Some("approved".to_string())), None),
            ],
            negated: false,
        });

        let filters = vec![expr];
        let indexable = FilterAnalyzer::extract_indexable(&filters);

        assert_eq!(indexable.len(), 1);
        assert_eq!(indexable[0].column, "status");

        if let IndexPredicate::In(values) = &indexable[0].predicate {
            assert_eq!(values.len(), 3);
        } else {
            panic!("Expected IndexPredicate::In");
        }
    }

    #[test]
    fn test_extract_integer_equality() {
        // Test: age = 25
        let expr = col("age").eq(Expr::Literal(ScalarValue::Int64(Some(25)), None));

        let filters = vec![expr];
        let indexable = FilterAnalyzer::extract_indexable(&filters);

        assert_eq!(indexable.len(), 1);
        assert_eq!(indexable[0].column, "age");

        if let IndexPredicate::Exact(IndexedValue::Int64(val)) = &indexable[0].predicate {
            assert_eq!(*val, 25);
        } else {
            panic!("Expected IndexPredicate::Exact with Int64");
        }
    }

    #[test]
    fn test_ignore_complex_expression() {
        // Test: column + 1 = 5 (should be ignored - binary expr on left)
        let expr = Expr::BinaryExpr(BinaryExpr {
            left: Box::new(Expr::BinaryExpr(BinaryExpr {
                left: Box::new(Expr::Column(Column::from_name("value"))),
                op: Operator::Plus,
                right: Box::new(Expr::Literal(ScalarValue::Int64(Some(1)), None)),
            })),
            op: Operator::Eq,
            right: Box::new(Expr::Literal(ScalarValue::Int64(Some(5)), None)),
        });

        let filters = vec![expr];
        let indexable = FilterAnalyzer::extract_indexable(&filters);

        // Should be empty - complex expressions are not indexable
        assert_eq!(indexable.len(), 0);
    }

    #[test]
    fn test_multiple_filters() {
        // Test: Multiple filters, some indexable, some not
        let filters = vec![
            col("email").eq(Expr::Literal(
                ScalarValue::Utf8(Some("test@example.com".to_string())),
                None,
            )),
            col("age").gt(Expr::Literal(ScalarValue::Int64(Some(18)), None)), // Now supported as range
            col("status").eq(Expr::Literal(
                ScalarValue::Utf8(Some("active".to_string())),
                None,
            )),
        ];

        let indexable = FilterAnalyzer::extract_indexable(&filters);

        // Should extract all three predicates (equality + range + equality)
        assert_eq!(indexable.len(), 3);
        assert!(indexable.iter().any(|f| f.column == "email"));
        assert!(indexable.iter().any(|f| f.column == "age"));
        assert!(indexable.iter().any(|f| f.column == "status"));
    }

    #[test]
    fn test_extract_range_greater_than() {
        // Test: age > 18
        let expr = col("age").gt(Expr::Literal(ScalarValue::Int64(Some(18)), None));

        let filters = vec![expr];
        let indexable = FilterAnalyzer::extract_indexable(&filters);

        assert_eq!(indexable.len(), 1);
        assert_eq!(indexable[0].column, "age");

        if let IndexPredicate::Range { min, max } = &indexable[0].predicate {
            assert_eq!(*min, IndexedValue::Int64(18));
            assert_eq!(*max, IndexedValue::Int64(i64::MAX));
        } else {
            panic!("Expected IndexPredicate::Range");
        }
    }

    #[test]
    fn test_extract_range_less_than() {
        // Test: price <= 100.0
        let expr = col("price").lt_eq(Expr::Literal(ScalarValue::Float64(Some(100.0)), None));

        let filters = vec![expr];
        let indexable = FilterAnalyzer::extract_indexable(&filters);

        assert_eq!(indexable.len(), 1);
        assert_eq!(indexable[0].column, "price");

        if let IndexPredicate::Range { min: _, max } = &indexable[0].predicate {
            assert_eq!(*max, IndexedValue::Float64(OrderedFloat(100.0)));
        } else {
            panic!("Expected IndexPredicate::Range");
        }
    }

    #[test]
    fn test_extract_range_and_combination() {
        // Test: age >= 18 AND age <= 65
        let left = col("age").gt_eq(Expr::Literal(ScalarValue::Int64(Some(18)), None));
        let right = col("age").lt_eq(Expr::Literal(ScalarValue::Int64(Some(65)), None));
        let expr = Expr::BinaryExpr(BinaryExpr {
            left: Box::new(left),
            op: Operator::And,
            right: Box::new(right),
        });

        let filters = vec![expr];
        let indexable = FilterAnalyzer::extract_indexable(&filters);

        assert_eq!(indexable.len(), 1);
        assert_eq!(indexable[0].column, "age");

        if let IndexPredicate::Range { min, max } = &indexable[0].predicate {
            assert_eq!(*min, IndexedValue::Int64(18));
            assert_eq!(*max, IndexedValue::Int64(65));
        } else {
            panic!(
                "Expected IndexPredicate::Range, got {:?}",
                indexable[0].predicate
            );
        }
    }

    #[test]
    fn test_extract_range_reversed_literal() {
        // Test: 18 < age (reversed)
        let expr = Expr::BinaryExpr(BinaryExpr {
            left: Box::new(Expr::Literal(ScalarValue::Int64(Some(18)), None)),
            op: Operator::Lt,
            right: Box::new(Expr::Column(Column::from_name("age"))),
        });

        let filters = vec![expr];
        let indexable = FilterAnalyzer::extract_indexable(&filters);

        assert_eq!(indexable.len(), 1);
        assert_eq!(indexable[0].column, "age");

        if let IndexPredicate::Range { min, max } = &indexable[0].predicate {
            assert_eq!(*min, IndexedValue::Int64(18));
            assert_eq!(*max, IndexedValue::Int64(i64::MAX));
        } else {
            panic!("Expected IndexPredicate::Range");
        }
    }
}
