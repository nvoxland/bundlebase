use datafusion::logical_expr::{Expr, LogicalPlan};
use std::collections::HashMap;

/// Maps a logical column name to its physical source
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ColumnSource {
    /// Pack name ("base" for base pack, or join name for joined packs)
    pub pack_name: String,
    /// Physical column name in the source file
    pub physical_name: String,
}

/// Analyzes a DataFusion LogicalPlan to extract column lineage
#[derive(Default)]
pub struct ColumnLineageAnalyzer {
    /// Maps logical column names to their sources
    lineage: HashMap<String, ColumnSource>,
    /// Maps table names to pack names (from our registration)
    table_to_pack: HashMap<String, String>,
}

impl ColumnLineageAnalyzer {
    pub fn new() -> Self {
        Self {
            lineage: HashMap::new(),
            table_to_pack: HashMap::new(),
        }
    }

    /// Register a table name to pack name mapping
    /// Used for base tables (__base_N) and joined tables (join names)
    pub fn register_table(&mut self, table_name: String, pack_name: String) {
        self.table_to_pack.insert(table_name, pack_name);
    }

    /// Analyze a LogicalPlan to extract column lineage
    pub fn analyze(&mut self, plan: &LogicalPlan) -> Result<(), String> {
        // Walk the plan tree and extract column mappings
        self.visit_plan(plan)?;
        Ok(())
    }

    /// Get the source for a logical column name
    pub fn get_source(&self, logical_name: &str) -> Option<ColumnSource> {
        self.lineage.get(logical_name).cloned()
    }

    /// Get all column sources
    pub fn get_all_sources(&self) -> HashMap<String, ColumnSource> {
        self.lineage.clone()
    }

    /// Visit a LogicalPlan node recursively
    fn visit_plan(&mut self, plan: &LogicalPlan) -> Result<(), String> {
        match plan {
            LogicalPlan::TableScan(scan) => {
                let table_name = scan.table_name.to_string();
                let pack_name = self
                    .table_to_pack
                    .get(&table_name)
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string());

                // All columns from this table scan map to the pack
                for field in scan.projected_schema.fields() {
                    let col_name = field.name();
                    self.lineage.insert(
                        col_name.to_string(),
                        ColumnSource {
                            pack_name: pack_name.clone(),
                            physical_name: col_name.to_string(),
                        },
                    );
                }
            }
            LogicalPlan::Projection(projection) => {
                // Visit input first (bottom-up)
                self.visit_plan(&projection.input)?;

                let mut new_lineage = HashMap::new();

                for (i, expr) in projection.expr.iter().enumerate() {
                    let output_name = projection.schema.field(i).name().to_string();

                    // Track the source of this output column
                    if let Some(source) = self.extract_column_source(expr) {
                        new_lineage.insert(output_name, source);
                    }
                }

                // Update lineage with projection results (keep previous lineage for untracked columns)
                for (name, source) in new_lineage {
                    self.lineage.insert(name, source);
                }
            }
            LogicalPlan::Join(join) => {
                // Visit inputs first (bottom-up)
                self.visit_plan(&join.left)?;
                self.visit_plan(&join.right)?;

                // Join merges columns from both sides - they're already in lineage
            }
            LogicalPlan::Filter(filter) => {
                // Filters don't change column lineage, just propagate
                self.visit_plan(&filter.input)?;
            }
            LogicalPlan::Union(_union) => {
                // Union merges columns from multiple inputs
                // Just visit inputs - columns should be available from one of them
                for input in plan.inputs() {
                    self.visit_plan(input)?;
                }
            }
            _ => {
                // For other node types, just visit inputs
                for input in plan.inputs() {
                    self.visit_plan(input)?;
                }
            }
        }
        Ok(())
    }

    /// Extract the column source from an expression
    fn extract_column_source(&self, expr: &Expr) -> Option<ColumnSource> {
        match expr {
            Expr::Column(col) => {
                // Direct column reference - preserve lineage
                self.lineage.get(col.name.as_str()).cloned()
            }
            Expr::Alias(alias) => {
                // Column alias (rename) - extract underlying column
                self.extract_column_source(&alias.expr)
            }
            Expr::Cast(cast) => {
                // Cast doesn't change source
                self.extract_column_source(&cast.expr)
            }
            // For other expressions (computed, functions, etc.), we can't track lineage
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analyzer_creation() {
        let analyzer = ColumnLineageAnalyzer::new();
        assert_eq!(analyzer.get_all_sources().len(), 0);
    }

    #[test]
    fn test_register_table() {
        let mut analyzer = ColumnLineageAnalyzer::new();
        analyzer.register_table("users".to_string(), "base".to_string());
        analyzer.register_table("orders".to_string(), "orders_pack".to_string());

        // Just verify tables are registered (we can't inspect them directly)
        assert_eq!(analyzer.get_all_sources().len(), 0); // No columns added yet
    }

    #[test]
    fn test_column_source_equality() {
        let source1 = ColumnSource {
            pack_name: "base".to_string(),
            physical_name: "id".to_string(),
        };
        let source2 = ColumnSource {
            pack_name: "base".to_string(),
            physical_name: "id".to_string(),
        };
        let source3 = ColumnSource {
            pack_name: "base".to_string(),
            physical_name: "name".to_string(),
        };

        assert_eq!(source1, source2);
        assert_ne!(source1, source3);
    }
}
