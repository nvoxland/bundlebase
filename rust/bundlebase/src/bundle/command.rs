use crate::bundle::operation::AnyOperation;
use crate::bundle::Bundle;
use crate::{BundleBuilder, BundlebaseError};
use async_trait::async_trait;
use datafusion::common::ScalarValue;
use std::collections::HashMap;
use crate::bundle::pack::JoinTypeOption;

pub mod parser;
pub mod parser_pest;

// Command struct modules
mod attach;
mod commit;
mod create_index;
mod create_source;
mod create_view;
mod drop_column;
mod drop_index;
mod drop_join;
mod drop_view;
mod fetch;
mod filter;
mod join;
mod reindex;
mod rename_column;
mod rename_join;
mod rename_view;
mod reset;
mod select;
mod set_description;
mod set_name;
mod undo;

// Re-export command structs
pub use attach::AttachCommand;
pub use commit::CommitCommand;
pub use create_index::CreateIndexCommand;
pub use create_source::CreateSourceCommand;
pub use create_view::CreateViewCommand;
pub use drop_column::DropColumnCommand;
pub use drop_index::DropIndexCommand;
pub use drop_join::DropJoinCommand;
pub use drop_view::DropViewCommand;
pub use fetch::{FetchAllCommand, FetchCommand};
pub use filter::FilterCommand;
pub use join::JoinCommand;
pub use reindex::ReindexCommand;
pub use rename_column::RenameColumnCommand;
pub use rename_join::RenameJoinCommand;
pub use rename_view::RenameViewCommand;
pub use reset::ResetCommand;
pub use select::SelectCommand;
pub use set_description::SetDescriptionCommand;
pub use set_name::SetNameCommand;
pub use undo::UndoCommand;

/// Context provided to commands during execution.
///
/// This provides a controlled interface for commands to interact with the
/// BundleBuilder without exposing its internals directly.
pub struct CommandContext<'a> {
    pub(crate) builder: &'a mut BundleBuilder,
}

impl<'a> CommandContext<'a> {
    /// Create a new CommandContext wrapping a BundleBuilder
    pub fn new(builder: &'a mut BundleBuilder) -> Self {
        Self { builder }
    }

    /// Apply an operation to the bundle
    pub async fn apply_operation(&mut self, op: AnyOperation) -> Result<(), BundlebaseError> {
        self.builder.apply_operation(op).await
    }

    /// Get a reference to the bundle
    pub fn bundle(&self) -> &Bundle {
        &self.builder.bundle
    }

    /// Get a mutable reference to the bundle
    pub fn bundle_mut(&mut self) -> &mut Bundle {
        &mut self.builder.bundle
    }

    /// Get a reference to the builder (for methods that need full builder access)
    pub fn builder(&self) -> &BundleBuilder {
        self.builder
    }

    /// Get a mutable reference to the builder (for methods that need full builder access)
    pub fn builder_mut(&mut self) -> &mut BundleBuilder {
        self.builder
    }

    /// Rebuild indexes after changes
    pub async fn reindex_internal(&mut self) -> Result<(), BundlebaseError> {
        self.builder.reindex_internal().await
    }
}

/// Trait for self-contained commands that can be executed on a BundleBuilder.
///
/// Commands encapsulate all the logic needed to perform a specific operation
/// on a bundle. They are constructed with all necessary parameters and then
/// executed via the `execute` method.
#[async_trait]
pub trait Command: Send + Sync {
    /// A human-readable description of what this command does
    fn description(&self) -> String;

    /// Execute the command using the provided context
    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError>;
}

/// Command that can be executed on a BundleBuilder.
///
/// This enum represents statements as user-facing Bundle/BundleBuilder method calls.
///
/// # Examples
///
/// ```ignore
/// use bundlebase::bundle::BundleCommand;
///
/// let cmd = BundleCommand::Attach { path: "data.parquet".to_string() };
/// cmd.execute(&mut bundle).await?;
/// ```
#[derive(Debug, Clone)]
pub enum BundleCommand {
    /// Attach a data source
    /// Maps to: `bundle.attach(&path, pack.as_deref())`
    /// If pack is None or "base", attaches to the base pack. Otherwise, attaches to the join pack.
    Attach {
        path: String,
        pack: Option<String>,
    },

    /// Filter rows by a WHERE condition
    /// Maps to: `bundle.filter(&where_clause, params)`
    Filter {
        where_clause: String,
        params: Vec<ScalarValue>,
    },

    /// Remove a column
    /// Maps to: `bundle.remove_column(&name)`
    DropColumn { name: String },

    /// Rename a column
    /// Maps to: `bundle.rename_column(&old_name, &new_name)`
    RenameColumn { old_name: String, new_name: String },

    /// Rename a view
    /// Maps to: `bundle.rename_view(&old_name, &new_name)`
    RenameView { old_name: String, new_name: String },

    /// Execute a full SQL query
    /// Maps to: `bundle.select(&sql, params)`
    Select {
        sql: String,
        params: Vec<ScalarValue>,
    },

    /// Join with another data source
    /// Maps to: `bundle.join(&name, location.as_deref(), &expression, join_type)`
    /// If location is None, creates a join point without initial data.
    Join {
        name: String,
        location: Option<String>,
        expression: String,
        join_type: JoinTypeOption,
    },

    /// Create an index on a column
    /// Maps to: `bundle.create_index(&column, index_type)`
    CreateIndex {
        column: String,
        index_type: crate::index::IndexType,
    },

    /// Drop an index on a column
    /// Maps to: `bundle.drop_index(&column)`
    DropIndex { column: String },

    /// Drop a view
    /// Maps to: `bundle.drop_view(&name)`
    DropView { name: String },

    /// Drop a join
    /// Maps to: `bundle.drop_join(&name)`
    DropJoin { name: String },

    /// Rename a join
    /// Maps to: `bundle.rename_join(&old_name, &new_name)`
    RenameJoin { old_name: String, new_name: String },

    /// Rebuild all indexes
    /// Maps to: `bundle.reindex()`
    Reindex,

    /// Set bundle name
    /// Maps to: `bundle.set_name(&name)`
    SetName { name: String },

    /// Set bundle description
    /// Maps to: `bundle.set_description(&description)`
    SetDescription { description: String },

    /// Commit changes
    /// Maps to: `bundle.commit(&message)`
    Commit { message: String },

    /// Reset uncommitted changes
    /// Maps to: `bundle.reset()`
    Reset,

    /// Undo last change
    /// Maps to: `bundle.undo()`
    Undo,

    /// Create a data source for fetching files
    /// Maps to: `bundle.create_source(&function, args, pack.as_deref())`
    CreateSource {
        function: String,
        args: HashMap<String, String>,
        pack: Option<String>,
    },

    /// Fetch new files from sources for a pack
    /// Maps to: `bundle.fetch(pack.as_deref())`
    Fetch { pack: Option<String> },

    /// Fetch new files from all defined sources
    /// Maps to: `bundle.fetch_all()`
    FetchAll,
}

impl BundleCommand {
    /// Execute this SQL command on a BundleBuilder.
    ///
    /// This method constructs the appropriate command struct and executes it via
    /// `execute_command`, which provides a self-contained command pattern.
    ///
    /// # Arguments
    ///
    /// * `builder` - Mutable reference to the BundleBuilder to execute the command on
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Command executed successfully
    /// * `Err(BundlebaseError)` - Execution failed
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let cmd = BundleCommand::Attach { path: "data.parquet".to_string(), pack: None };
    /// cmd.execute(&mut builder).await?;
    /// ```
    pub async fn execute(self, builder: &mut BundleBuilder) -> Result<(), BundlebaseError> {
        match self {
            BundleCommand::Attach { path, pack } => {
                builder.execute_command(AttachCommand::new(path, pack)).await?;
                Ok(())
            }
            BundleCommand::Filter {
                where_clause,
                params,
            } => {
                builder.execute_command(FilterCommand::new(where_clause, params)).await?;
                Ok(())
            }
            BundleCommand::DropColumn { name } => {
                builder.execute_command(DropColumnCommand::new(name)).await?;
                Ok(())
            }
            BundleCommand::RenameColumn { old_name, new_name } => {
                builder.execute_command(RenameColumnCommand::new(old_name, new_name)).await?;
                Ok(())
            }
            BundleCommand::RenameView { old_name, new_name } => {
                builder.execute_command(RenameViewCommand::new(old_name, new_name)).await?;
                Ok(())
            }
            BundleCommand::Select { sql, params } => {
                // Select is special - it returns a new BundleBuilder, not modifying in place
                // For the command pattern, we apply the SelectOp to modify the current builder
                builder.execute_command(SelectCommand::new(sql, params)).await?;
                Ok(())
            }
            BundleCommand::Join {
                name,
                location,
                expression,
                join_type,
            } => {
                builder.execute_command(JoinCommand::new(name, expression, location, join_type)).await?;
                Ok(())
            }
            BundleCommand::CreateIndex { column, index_type } => {
                builder.execute_command(CreateIndexCommand::new(column, index_type)).await?;
                Ok(())
            }
            BundleCommand::DropIndex { column } => {
                builder.execute_command(DropIndexCommand::new(column)).await?;
                Ok(())
            }
            BundleCommand::DropView { name } => {
                builder.execute_command(DropViewCommand::new(name)).await?;
                Ok(())
            }
            BundleCommand::DropJoin { name } => {
                builder.execute_command(DropJoinCommand::new(name)).await?;
                Ok(())
            }
            BundleCommand::RenameJoin { old_name, new_name } => {
                builder.execute_command(RenameJoinCommand::new(old_name, new_name)).await?;
                Ok(())
            }
            BundleCommand::Reindex => {
                builder.execute_command(ReindexCommand::new()).await?;
                Ok(())
            }
            BundleCommand::SetName { name } => {
                builder.execute_command(SetNameCommand::new(name)).await?;
                Ok(())
            }
            BundleCommand::SetDescription { description } => {
                builder.execute_command(SetDescriptionCommand::new(description)).await?;
                Ok(())
            }
            BundleCommand::Commit { message } => {
                // Commit is special - it doesn't go through execute_command
                // because it needs to finalize all pending changes
                builder.commit(&message).await?;
                Ok(())
            }
            BundleCommand::Reset => {
                // Reset is special - it doesn't go through execute_command
                builder.reset().await?;
                Ok(())
            }
            BundleCommand::Undo => {
                // Undo is special - it doesn't go through execute_command
                builder.undo().await?;
                Ok(())
            }
            BundleCommand::CreateSource {
                function,
                args,
                pack,
            } => {
                builder.execute_command(CreateSourceCommand::new(function, args, pack)).await?;
                Ok(())
            }
            BundleCommand::Fetch { pack } => {
                builder.execute_command(FetchCommand::new(pack)).await?;
                Ok(())
            }
            BundleCommand::FetchAll => {
                builder.execute_command(FetchAllCommand::new()).await?;
                Ok(())
            }
        }
    }

    /// Add parameters to this command for parameterized queries.
    ///
    /// This method is used to bind parameters ($1, $2, etc.) in SQL statements if applicable.
    ///
    /// # Arguments
    ///
    /// * `params` - Vector of ScalarValue parameters
    ///
    /// # Returns
    ///
    /// * `Self` - The command with parameters added
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let cmd = BundleCommand::Filter {
    ///     where_clause: "salary > $1".to_string(),
    ///     params: vec![],
    /// };
    /// let cmd_with_params = cmd.with_params(vec![
    ///     ScalarValue::Float64(Some(50000.0))
    /// ]);
    /// ```
    pub fn with_params(mut self, params: Vec<ScalarValue>) -> Self {
        match &mut self {
            BundleCommand::Filter {
                params: ref mut p, ..
            } => *p = params,
            BundleCommand::Select {
                params: ref mut p, ..
            } => *p = params,
            _ => {} // Other commands don't support parameters
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::command::BundleCommand;
    use crate::bundle::ScalarValue;
    #[test]
    fn test_with_params_filter() {
        let cmd = BundleCommand::Filter {
            where_clause: "salary > $1".to_string(),
            params: vec![],
        };

        let params = vec![ScalarValue::Float64(Some(50000.0))];
        let cmd_with_params = cmd.with_params(params.clone());

        match cmd_with_params {
            BundleCommand::Filter {
                where_clause,
                params: p,
            } => {
                assert_eq!(where_clause, "salary > $1");
                assert_eq!(p.len(), 1);
            }
            _ => panic!("Expected Filter variant"),
        }
    }

    #[test]
    fn test_with_params_select() {
        let cmd = BundleCommand::Select {
            sql: "SELECT * FROM bundle WHERE id = $1".to_string(),
            params: vec![],
        };

        let params = vec![ScalarValue::Int64(Some(42))];
        let cmd_with_params = cmd.with_params(params.clone());

        match cmd_with_params {
            BundleCommand::Select { sql, params: p } => {
                assert_eq!(sql, "SELECT * FROM bundle WHERE id = $1");
                assert_eq!(p.len(), 1);
            }
            _ => panic!("Expected Query variant"),
        }
    }

    #[test]
    fn test_with_params_other_command() {
        // with_params should have no effect on commands that don't support parameters
        let cmd = BundleCommand::Attach {
            path: "data.parquet".to_string(),
            pack: None,
        };

        let params = vec![ScalarValue::Int64(Some(42))];
        let cmd_with_params = cmd.with_params(params);

        match cmd_with_params {
            BundleCommand::Attach { path, pack } => {
                assert_eq!(path, "data.parquet");
                assert_eq!(pack, None);
            }
            _ => panic!("Expected Attach variant"),
        }
    }

    #[test]
    fn test_attach_to_pack_command() {
        let cmd = BundleCommand::Attach {
            path: "more_users.parquet".to_string(),
            pack: Some("users".to_string()),
        };

        match cmd {
            BundleCommand::Attach { path, pack } => {
                assert_eq!(path, "more_users.parquet");
                assert_eq!(pack, Some("users".to_string()));
            }
            _ => panic!("Expected Attach variant"),
        }
    }

    #[test]
    fn test_create_source_command() {
        let mut args = HashMap::new();
        args.insert("url".to_string(), "s3://bucket/data/".to_string());
        args.insert("patterns".to_string(), "**/*.parquet".to_string());

        let cmd = BundleCommand::CreateSource {
            function: "remote_dir".to_string(),
            args: args.clone(),
            pack: None,
        };

        match cmd {
            BundleCommand::CreateSource {
                function,
                args: a,
                pack,
            } => {
                assert_eq!(function, "remote_dir");
                assert_eq!(a.get("url"), Some(&"s3://bucket/data/".to_string()));
                assert_eq!(a.get("patterns"), Some(&"**/*.parquet".to_string()));
                assert_eq!(pack, None);
            }
            _ => panic!("Expected CreateSource variant"),
        }
    }

    #[test]
    fn test_create_source_with_pack_command() {
        let mut args = HashMap::new();
        args.insert("url".to_string(), "s3://bucket/users/".to_string());

        let cmd = BundleCommand::CreateSource {
            function: "remote_dir".to_string(),
            args,
            pack: Some("users".to_string()),
        };

        match cmd {
            BundleCommand::CreateSource {
                function,
                args: _,
                pack,
            } => {
                assert_eq!(function, "remote_dir");
                assert_eq!(pack, Some("users".to_string()));
            }
            _ => panic!("Expected CreateSource variant"),
        }
    }

    #[test]
    fn test_fetch_command() {
        let cmd = BundleCommand::Fetch {
            pack: Some("users".to_string()),
        };

        match cmd {
            BundleCommand::Fetch { pack } => {
                assert_eq!(pack, Some("users".to_string()));
            }
            _ => panic!("Expected Fetch variant"),
        }
    }

    #[test]
    fn test_fetch_all_command() {
        let cmd = BundleCommand::FetchAll;

        match cmd {
            BundleCommand::FetchAll => {}
            _ => panic!("Expected FetchAll variant"),
        }
    }
}
