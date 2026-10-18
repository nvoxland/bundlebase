//! Replay checkpoint maintenance commands. These do not record operations.

use crate::{BundleBuilderCommand, CommandParsing, Rule};
use bundlebase::BundleBuilder;
use bundlebase_common::BundlebaseError;

#[derive(Debug, Clone, Default)]
pub struct CheckpointCommand;

impl CommandParsing for CheckpointCommand {
    fn rule() -> Rule {
        Rule::checkpoint_stmt
    }
    fn from_statement(_pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        Ok(Self)
    }
    fn to_statement(&self) -> String {
        "CHECKPOINT".to_string()
    }
}

impl BundleBuilderCommand for CheckpointCommand {
    type Output = String;
    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        builder.checkpoint().await?;
        Ok("Created checkpoint".to_string())
    }
}

#[derive(Debug, Clone, Default)]
pub struct CheckpointPruneCommand;

impl CommandParsing for CheckpointPruneCommand {
    fn rule() -> Rule {
        Rule::checkpoint_prune_stmt
    }
    fn from_statement(_pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        Ok(Self)
    }
    fn to_statement(&self) -> String {
        "CHECKPOINT PRUNE".to_string()
    }
}

impl BundleBuilderCommand for CheckpointPruneCommand {
    type Output = String;
    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        builder.checkpoint_prune().await?;
        Ok("Pruned checkpoints".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parser::parse_command, BundleCommand};

    #[test]
    fn parse_checkpoint_and_prune_round_trip() -> Result<(), BundlebaseError> {
        for input in ["CHECKPOINT", "checkpoint"] {
            let command = parse_command(input)?;
            let BundleCommand::Checkpoint(command) = command else {
                panic!("expected checkpoint")
            };
            assert_eq!(command.to_statement(), "CHECKPOINT");
            assert!(matches!(
                parse_command(&command.to_statement())?,
                BundleCommand::Checkpoint(_)
            ));
        }
        for input in ["CHECKPOINT PRUNE", "checkpoint prune"] {
            let command = parse_command(input)?;
            let BundleCommand::CheckpointPrune(command) = command else {
                panic!("expected checkpoint prune")
            };
            assert_eq!(command.to_statement(), "CHECKPOINT PRUNE");
            assert!(matches!(
                parse_command(&command.to_statement())?,
                BundleCommand::CheckpointPrune(_)
            ));
        }
        Ok(())
    }
}
