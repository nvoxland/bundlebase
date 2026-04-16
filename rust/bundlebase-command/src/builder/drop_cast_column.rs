//! DropCastColumn command implementation.
//!
//! `DROP CAST COLUMN <name>` cancels the most recent active cast on a column.
//! The command validates that an active cast exists (using cast-stack logic),
//! then records a `DropCastColumnOp`. At pipeline time, `resolve_cast_ops`
//! pairs each `DropCastColumnOp` with the `CastColumnOp` it cancels — both
//! are skipped, so the column reverts to the type it had before that cast.

use crate::parser::extract_identifier;
use crate::BundleBuilderCommand;
use crate::{CommandParsing, Rule};
use bundlebase::bundle::operation::{AnyOperation, DropCastColumnOp};
use bundlebase::bundle::BundleFacade;
use bundlebase::BundleBuilder;
use bundlebase_common::BundlebaseError;

/// Command to drop (cancel) the most recent active cast on a column.
#[derive(Debug, Clone)]
pub struct DropCastColumnCommand {
    /// The column name whose most recent cast should be dropped
    pub name: String,
}

impl CommandParsing for DropCastColumnCommand {
    fn rule() -> Rule {
        Rule::drop_cast_column_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;
        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::identifier {
                name = Some(extract_identifier(&inner));
                break;
            }
        }
        let name = name.ok_or_else(|| -> BundlebaseError {
            "DROP CAST COLUMN statement missing column name".into()
        })?;
        Ok(DropCastColumnCommand { name })
    }

    fn to_statement(&self) -> String {
        format!("DROP CAST COLUMN \"{}\"", self.name)
    }
}

impl BundleBuilderCommand for DropCastColumnCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let id = builder
            .column_id(&self.name)
            .ok_or_else(|| BundlebaseError::from(format!("Column '{}' not found", self.name)))?;

        // Validate that there is at least one active cast to drop by replaying the cast stack.
        let ops = builder.operations();
        let active_cast_count = active_cast_depth(&ops, id);
        if active_cast_count == 0 {
            return Err(format!("Column '{}' has no active cast to drop", self.name).into());
        }

        builder
            .apply_operation(DropCastColumnOp::setup(id).into())
            .await?;

        Ok(format!("Dropped cast on column '{}'", self.name))
    }
}

/// Count the number of active (un-dropped) casts for a column by replaying the cast stack.
fn active_cast_depth(ops: &[AnyOperation], id: bundlebase_common::object_id::ColumnId) -> usize {
    let mut depth: usize = 0;
    for op in ops {
        match op {
            AnyOperation::CastColumn(c) if c.id == id => depth += 1,
            AnyOperation::DropCastColumn(d) if d.id == id => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    depth
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;
    use arrow_schema::DataType;
    use bundlebase::bundle::operation::{resolve_cast_ops, CastColumnOp};
    use bundlebase_common::object_id::ColumnId;

    #[test]
    fn test_parse_drop_cast_column() {
        let cmd = parse_command("DROP CAST COLUMN value").unwrap();
        match cmd {
            BundleCommand::DropCastColumn(c) => assert_eq!(c.name, "value"),
            other => panic!("Expected DropCastColumn, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_quoted() {
        let cmd = parse_command(r#"DROP CAST COLUMN "My Column""#).unwrap();
        match cmd {
            BundleCommand::DropCastColumn(c) => assert_eq!(c.name, "My Column"),
            other => panic!("Expected DropCastColumn, got {:?}", other),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = DropCastColumnCommand {
            name: "value".to_string(),
        };
        let stmt = cmd.to_statement();
        assert_eq!(stmt, r#"DROP CAST COLUMN "value""#);
        let parsed = parse_command(&stmt).unwrap();
        match parsed {
            BundleCommand::DropCastColumn(c) => assert_eq!(c.name, "value"),
            other => panic!("Expected DropCastColumn, got {:?}", other),
        }
    }

    // ---------- resolve_cast_ops stack tests ----------

    /// Single cast, no drop: the cast is active.
    #[test]
    fn test_resolve_single_cast_active() {
        let id = ColumnId::generate();
        let ops = vec![AnyOperation::CastColumn(CastColumnOp::setup(
            id,
            DataType::Int64,
        ))];
        let active = resolve_cast_ops(&ops);
        assert_eq!(active, vec![true]);
    }

    /// Cast then drop: both are cancelled (neither applied).
    #[test]
    fn test_resolve_cast_then_drop_both_cancelled() {
        let id = ColumnId::generate();
        let ops = vec![
            AnyOperation::CastColumn(CastColumnOp::setup(id, DataType::Int64)),
            AnyOperation::DropCastColumn(DropCastColumnOp::setup(id)),
        ];
        let active = resolve_cast_ops(&ops);
        assert_eq!(
            active,
            vec![false, false],
            "cast and its drop should both be skipped"
        );
    }

    /// Two casts then one drop: second cast is cancelled, first remains.
    #[test]
    fn test_resolve_two_casts_one_drop() {
        let id = ColumnId::generate();
        let ops = vec![
            AnyOperation::CastColumn(CastColumnOp::setup(id, DataType::Int64)), // idx 0
            AnyOperation::CastColumn(CastColumnOp::setup(id, DataType::Float64)), // idx 1
            AnyOperation::DropCastColumn(DropCastColumnOp::setup(id)),          // idx 2
        ];
        let active = resolve_cast_ops(&ops);
        assert_eq!(
            active,
            vec![true, false, false],
            "only first cast survives; second cast and drop are cancelled"
        );
    }

    /// Two casts then two drops: both casts cancelled, column reverts to original.
    #[test]
    fn test_resolve_two_casts_two_drops() {
        let id = ColumnId::generate();
        let ops = vec![
            AnyOperation::CastColumn(CastColumnOp::setup(id, DataType::Int64)), // idx 0
            AnyOperation::CastColumn(CastColumnOp::setup(id, DataType::Float64)), // idx 1
            AnyOperation::DropCastColumn(DropCastColumnOp::setup(id)),          // idx 2
            AnyOperation::DropCastColumn(DropCastColumnOp::setup(id)),          // idx 3
        ];
        let active = resolve_cast_ops(&ops);
        assert_eq!(
            active,
            vec![false, false, false, false],
            "all casts and drops cancelled; column has no active cast"
        );
    }

    /// Cast, drop, cast: first pair cancels, second cast remains.
    #[test]
    fn test_resolve_cast_drop_cast() {
        let id = ColumnId::generate();
        let ops = vec![
            AnyOperation::CastColumn(CastColumnOp::setup(id, DataType::Int64)), // idx 0
            AnyOperation::DropCastColumn(DropCastColumnOp::setup(id)),          // idx 1
            AnyOperation::CastColumn(CastColumnOp::setup(id, DataType::Float64)), // idx 2
        ];
        let active = resolve_cast_ops(&ops);
        assert_eq!(
            active,
            vec![false, false, true],
            "first cast and its drop cancel; second cast is the active one"
        );
    }

    /// Active cast depth validation: no casts → 0, one cast → 1, cast+drop → 0.
    #[test]
    fn test_active_cast_depth() {
        let id = ColumnId::generate();

        let no_casts: Vec<AnyOperation> = vec![];
        assert_eq!(active_cast_depth(&no_casts, id), 0);

        let one_cast = vec![AnyOperation::CastColumn(CastColumnOp::setup(
            id,
            DataType::Int64,
        ))];
        assert_eq!(active_cast_depth(&one_cast, id), 1);

        let cast_and_drop = vec![
            AnyOperation::CastColumn(CastColumnOp::setup(id, DataType::Int64)),
            AnyOperation::DropCastColumn(DropCastColumnOp::setup(id)),
        ];
        assert_eq!(active_cast_depth(&cast_and_drop, id), 0);

        let two_casts_one_drop = vec![
            AnyOperation::CastColumn(CastColumnOp::setup(id, DataType::Int64)),
            AnyOperation::CastColumn(CastColumnOp::setup(id, DataType::Float64)),
            AnyOperation::DropCastColumn(DropCastColumnOp::setup(id)),
        ];
        assert_eq!(active_cast_depth(&two_casts_one_drop, id), 1);
    }

    /// Operations on a different column are not affected.
    #[test]
    fn test_resolve_other_column_unaffected() {
        let id_a = ColumnId::generate();
        let id_b = ColumnId::generate();
        let ops = vec![
            AnyOperation::CastColumn(CastColumnOp::setup(id_a, DataType::Int64)),
            AnyOperation::DropCastColumn(DropCastColumnOp::setup(id_b)), // different column
        ];
        let active = resolve_cast_ops(&ops);
        // id_a cast is active (no drop for it); id_b drop has nothing to cancel
        assert_eq!(active, vec![true, false]);
    }
}
