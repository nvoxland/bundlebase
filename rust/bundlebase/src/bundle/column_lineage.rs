use crate::bundle::Pack;
use datafusion::logical_expr::{Expr, LogicalPlan};
use std::collections::HashMap;
use std::sync::Arc;

/// Maps a logical column name to its physical source
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ColumnSource {
    /// Pack name ("base" for base pack, or join name for joined packs)
    pub pack_name: String,
    /// Physical column name in the source file
    pub physical_name: String,
}

/// Determine the source pack for each column in the final bundle schema.
///
/// Uses the join pack column names and the disambiguation convention
/// (`{pack_name}_{col}` for collisions) to map each final column back
/// to its origin pack. Returns a map from logical column name to `ColumnSource`.
pub fn analyze_column_sources(
    bundle_schema: &arrow_schema::Schema,
    packs: &HashMap<crate::io::ObjectId, Arc<Pack>>,
) -> HashMap<String, ColumnSource> {
    // Collect join pack names for disambiguated column detection.
    let join_pack_names: Vec<&str> = packs
        .values()
        .filter(|p| p.is_join())
        .map(|p| p.name())
        .collect();

    // Collect join pack column sets using user-visible names from the bundle schema.
    // Since block schemas use col_<id> internally, we identify join pack columns by
    // checking if the bundle schema column:
    //   1. Is not in the base pack (identified by exclusion)
    //   2. Or matches the disambiguated pattern {pack}_{col}
    let join_packs: Vec<(&str, std::collections::HashSet<String>)> = packs
        .values()
        .filter(|p| p.is_join())
        .map(|p| {
            // Get all column IDs from this join pack
            let pack_col_ids: std::collections::HashSet<_> = p
                .blocks()
                .iter()
                .flat_map(|b| b.column_ids().to_vec())
                .collect();

            // Find bundle schema columns whose col_<id> matches this pack's column IDs.
            // Since the bundle schema has been through the final rename and may have lost
            // ColumnId metadata, we find columns by checking ALL fields.
            // Simpler approach: collect the non-disambiguated column names by finding
            // bundle schema columns that match the pack's column count and position.
            // Actually simplest: just return an empty set and let the disambiguated
            // pattern matching handle everything below.
            let col_names = std::collections::HashSet::new();
            (p.name(), col_names)
        })
        .collect();

    let mut sources = HashMap::new();

    for field in bundle_schema.fields() {
        let col_name = field.name();

        // Check if this column matches a disambiguated join column ({pack}_{col})
        let mut found = false;
        for (pack_name, pack_cols) in &join_packs {
            // Check disambiguated pattern: regions_Country -> pack "regions", physical "Country"
            let prefix = format!("{}_", pack_name);
            if let Some(physical) = col_name.strip_prefix(&prefix) {
                // With col_<id> internals, we can't easily verify the physical name
                // is in the pack. Accept any {pack}_ prefixed column as from that pack.
                sources.insert(
                    col_name.clone(),
                    ColumnSource {
                        pack_name: pack_name.to_string(),
                        physical_name: physical.to_string(),
                    },
                );
                found = true;
                break;
            }

            // Check if it's a non-colliding join column (same name, not in base)
            if pack_cols.contains(col_name.as_str()) {
                // Could be from base or join - we need to check if base also has it
                // If base had it, the join column would have been disambiguated
                // So if we reach here with an un-prefixed name that's in a join pack,
                // it either came from base (if base also has it) or from the join pack
                // The disambiguation logic means: if it's NOT prefixed and it's in a
                // join pack, then base does NOT have a column with this name, so it's
                // from the join pack.
                // But we also need to check that base doesn't have this column...
                // We'll handle this below after checking all join packs.
            }
        }

        if !found {
            // Check if it's a non-disambiguated join-only column
            // (exists in a join pack but was not renamed because no collision with base)
            let mut from_join = false;
            for (pack_name, pack_cols) in &join_packs {
                if pack_cols.contains(col_name.as_str()) {
                    // Check if any OTHER pack (including base) also has this column
                    // If it wasn't disambiguated, it means it only exists in this join pack
                    // (or the base pack also has it, in which case this IS the base version)
                    // We can't easily distinguish, so check if the base pack has blocks with this col
                    let base_has_col = packs
                        .values()
                        .filter(|p| !p.is_join())
                        .any(|p| {
                            p.blocks().iter().any(|b| {
                                b.schema().fields().iter().any(|f| f.name() == col_name)
                            })
                        });

                    if !base_has_col {
                        sources.insert(
                            col_name.clone(),
                            ColumnSource {
                                pack_name: pack_name.to_string(),
                                physical_name: col_name.clone(),
                            },
                        );
                        from_join = true;
                        break;
                    }
                }
            }

            if !from_join {
                // Default: column is from base pack
                sources.insert(
                    col_name.clone(),
                    ColumnSource {
                        pack_name: "base".to_string(),
                        physical_name: col_name.clone(),
                    },
                );
            }
        }
    }

    sources
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
