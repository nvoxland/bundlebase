use crate::{BundleBuilder, BundleFacade, BundlebaseError};
use datafusion::common::ScalarValue;
use std::collections::HashMap;
use crate::bundle::pack::JoinTypeOption;

pub mod parser;
pub mod parser_pest;

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
    /// Maps to: `bundle.index(&column)`
    Index { column: String },

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
    /// This method delegates to the appropriate BundleBuilder method based on the command variant.
    ///
    /// # Arguments
    ///
    /// * `bundle` - Mutable reference to the BundleBuilder to execute the command on
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Command executed successfully
    /// * `Err(BundlebaseError)` - Execution failed
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let cmd = BundleCommand::Attach { path: "data.parquet".to_string() };
    /// cmd.execute(&mut bundle).await?;
    /// ```
    pub async fn execute(self, bundle: &mut BundleBuilder) -> Result<(), BundlebaseError> {
        match self {
            BundleCommand::Attach { path, pack } => {
                bundle.attach(&path, pack.as_deref()).await?;
                Ok(())
            }
            BundleCommand::Filter {
                where_clause,
                params,
            } => {
                bundle.filter(&where_clause, params).await?;
                Ok(())
            }
            BundleCommand::DropColumn { name } => {
                bundle.drop_column(&name).await?;
                Ok(())
            }
            BundleCommand::RenameColumn { old_name, new_name } => {
                bundle.rename_column(&old_name, &new_name).await?;
                Ok(())
            }
            BundleCommand::RenameView { old_name, new_name } => {
                bundle.rename_view(&old_name, &new_name).await?;
                Ok(())
            }
            BundleCommand::Select { sql, params } => {
                bundle.select(&sql, params).await?;
                Ok(())
            }
            BundleCommand::Join {
                name,
                location,
                expression,
                join_type,
            } => {
                bundle
                    .join(&name, &expression, location.as_deref(), join_type)
                    .await?;
                Ok(())
            }
            BundleCommand::Index { column } => {
                bundle.index(&column).await?;
                Ok(())
            }
            BundleCommand::DropIndex { column } => {
                bundle.drop_index(&column).await?;
                Ok(())
            }
            BundleCommand::DropView { name } => {
                bundle.drop_view(&name).await?;
                Ok(())
            }
            BundleCommand::DropJoin { name } => {
                bundle.drop_join(&name).await?;
                Ok(())
            }
            BundleCommand::RenameJoin { old_name, new_name } => {
                bundle.rename_join(&old_name, &new_name).await?;
                Ok(())
            }
            BundleCommand::Reindex => {
                bundle.reindex().await?;
                Ok(())
            }
            BundleCommand::SetName { name } => {
                bundle.set_name(&name).await?;
                Ok(())
            }
            BundleCommand::SetDescription { description } => {
                bundle.set_description(&description).await?;
                Ok(())
            }
            BundleCommand::Commit { message } => {
                bundle.commit(&message).await?;
                Ok(())
            }
            BundleCommand::Reset => {
                bundle.reset().await?;
                Ok(())
            }
            BundleCommand::Undo => {
                bundle.undo().await?;
                Ok(())
            }
            BundleCommand::CreateSource {
                function,
                args,
                pack,
            } => {
                bundle
                    .create_source(&function, args, pack.as_deref())
                    .await?;
                Ok(())
            }
            BundleCommand::Fetch { pack } => {
                bundle.fetch(pack.as_deref()).await?;
                Ok(())
            }
            BundleCommand::FetchAll => {
                bundle.fetch_all().await?;
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
