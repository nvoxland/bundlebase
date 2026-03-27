//! Extension trait for command-based convenience methods on BundleBuilder.
//!
//! This module provides the `BundleBuilderExt` trait that adds command-based
//! convenience methods to `BundleBuilder`. These methods were previously defined
//! directly on `BundleBuilder` but are separated to keep the command system
//! in its own crate.

use bundlebase::bundle::BundleBuilder;
use bundlebase::bundle::JoinTypeOption;
use bundlebase::source::{FetchResults, SyncMode};
use bundlebase::bundle_config::Scope;
use bundlebase_common::BundlebaseError;
use bundlebase_common::Platform;
use bundlebase_index::IndexType;
use datafusion::scalar::ScalarValue;
use std::collections::HashMap;
use std::sync::Arc;
use async_trait::async_trait;

use crate::builder::{
    AddColumnCommand, AttachCommand, CastColumnCommand, CreateIndexCommand, CreateSourceCommand,
    DeleteCommand, DetachBlockCommand, DropColumnCommand, DropConnectorCommand, DropFunctionCommand,
    DropIndexCommand, DropJoinCommand, DropViewCommand, FetchAllCommand, FetchCommand,
    FilterCommand, ImportConnectorCommand, ImportFunctionCommand, JoinCommand,
    RebuildIndexCommand, ReindexCommand, RenameColumnCommand, RenameConnectorCommand,
    RenameFunctionCommand, RenameJoinCommand, RenameViewCommand, ReplaceBlockCommand,
    SaveConfigCommand, SetDescriptionCommand, SetNameCommand, StandardizeColumnNamesCommand,
    VerifyDataCommand,
};
use crate::BundleBuilderCommand;

/// Helper function to execute a builder command on a BundleBuilder.
///
/// This is a standalone function (not a trait method) because generic async methods
/// with `async_trait` have Send bound issues. Each trait method calls this internally.
async fn exec_cmd<C: BundleBuilderCommand + 'static>(
    builder: &BundleBuilder,
    cmd: C,
) -> Result<C::Output, BundlebaseError> {
    let description = cmd.to_statement();
    builder.run_command(description, Box::new(cmd).execute(builder)).await
}

/// Extension trait that adds command-based convenience methods to `BundleBuilder`.
///
/// Import this trait to call methods like `attach()`, `filter()`, `commit()`, etc.
/// on a `BundleBuilder`.
///
/// # Example
/// ```ignore
/// use bundlebase_command::BundleBuilderExt;
///
/// let builder = BundleBuilder::create("memory://work", None).await?;
/// builder.attach("data.parquet", None).await?;
/// builder.filter("amount > 100", vec![]).await?;
/// builder.commit("Filter high-value transactions").await?;
/// ```
#[async_trait]
pub trait BundleBuilderExt {
    /// Attach a data block to the bundle.
    async fn attach(&self, path: &str, pack: Option<&str>) -> Result<&Self, BundlebaseError>;

    /// Detach a data block from the bundle by its location.
    async fn detach_block(&self, location: &str) -> Result<&Self, BundlebaseError>;

    /// Replace a block's location in the bundle.
    async fn replace_block(
        &self,
        old_location: &str,
        new_location: &str,
    ) -> Result<&Self, BundlebaseError>;

    /// Create a data source for a pack.
    async fn create_source(
        &self,
        connector: &str,
        args: HashMap<String, String>,
        pack: Option<&str>,
    ) -> Result<&Self, BundlebaseError>;

    /// Load a named connector (persisted).
    async fn import_connector(
        &self,
        name: &str,
        from: &str,
        platform: &str,
    ) -> Result<&Self, BundlebaseError>;

    /// Rename a connector to a new dotted name.
    async fn rename_connector(
        &self,
        old_name: &str,
        new_name: &str,
    ) -> Result<&Self, BundlebaseError>;

    /// Drop a connector. Without a platform, removes the entire connector definition.
    /// With a platform, removes only that platform.
    async fn drop_connector(
        &self,
        name: &str,
        platform: Option<&str>,
    ) -> Result<&Self, BundlebaseError>;

    /// Fetch from sources for a pack - discover and attach new files.
    async fn fetch(
        &self,
        pack: &str,
        mode: SyncMode,
    ) -> Result<Vec<FetchResults>, BundlebaseError>;

    /// Fetch from all defined sources - discover and attach new files.
    async fn fetch_all(&self, mode: SyncMode) -> Result<Vec<FetchResults>, BundlebaseError>;

    /// Create a view from a SQL statement.
    async fn create_view(
        &self,
        name: &str,
        sql: &str,
    ) -> Result<Arc<BundleBuilder>, BundlebaseError>;

    /// Rename an existing view.
    async fn rename_view(
        &self,
        old_name: &str,
        new_name: &str,
    ) -> Result<&Self, BundlebaseError>;

    /// Drop an existing view.
    async fn drop_view(&self, view_name: &str) -> Result<&Self, BundlebaseError>;

    /// Drop an existing join.
    async fn drop_join(&self, join_name: &str) -> Result<&Self, BundlebaseError>;

    /// Rename an existing join.
    async fn rename_join(
        &self,
        old_name: &str,
        new_name: &str,
    ) -> Result<&Self, BundlebaseError>;

    /// Drop a column.
    async fn drop_column(&self, name: &str) -> Result<&Self, BundlebaseError>;

    /// Rename a column.
    async fn rename_column(
        &self,
        old_name: &str,
        new_name: &str,
    ) -> Result<&Self, BundlebaseError>;

    /// Add a computed column to the bundle.
    async fn add_column(
        &self,
        name: &str,
        expression: &str,
    ) -> Result<&Self, BundlebaseError>;

    /// Cast a column to a different data type, optionally cleaning values first.
    async fn cast_column(
        &self,
        name: &str,
        new_type: &str,
        clean: Option<String>,
    ) -> Result<&Self, BundlebaseError>;

    /// Standardize all column names to lowercase+underscore identifiers.
    async fn standardize_column_names(&self) -> Result<&Self, BundlebaseError>;

    /// Filter rows with a SELECT query.
    async fn filter(
        &self,
        query: &str,
        params: Vec<ScalarValue>,
    ) -> Result<&Self, BundlebaseError>;

    /// Delete rows matching a WHERE clause.
    /// Returns the number of deleted rows.
    async fn delete(&self, where_clause: &str) -> Result<usize, BundlebaseError>;

    /// Join with another data source.
    async fn join(
        &self,
        name: &str,
        expression: &str,
        location: Option<&str>,
        join_type: JoinTypeOption,
    ) -> Result<&Self, BundlebaseError>;

    /// Load a persistent function (bundled, not session-only).
    async fn import_function(
        &self,
        name: &str,
        from: &str,
        platform: &str,
    ) -> Result<&Self, BundlebaseError>;

    /// Rename a function to a new dotted name.
    async fn rename_function(
        &self,
        old_name: &str,
        new_name: &str,
    ) -> Result<&Self, BundlebaseError>;

    /// Drop a persistent function.
    async fn drop_function(
        &self,
        name: &str,
        platform: Option<&str>,
    ) -> Result<&Self, BundlebaseError>;

    /// Set the bundle's name.
    async fn set_name(&self, name: &str) -> Result<&Self, BundlebaseError>;

    /// Set the bundle's description.
    async fn set_description(&self, description: &str) -> Result<&Self, BundlebaseError>;

    /// Save a configuration value to the bundle manifest.
    async fn save_config(
        &self,
        scope: &Scope,
        key: &str,
        value: &str,
    ) -> Result<&Self, BundlebaseError>;

    /// Create an index on one or more columns.
    async fn create_index(
        &self,
        columns: &[&str],
        index_type: IndexType,
        name: Option<&str>,
    ) -> Result<&Self, BundlebaseError>;

    /// Drop an index on a column.
    async fn drop_index(&self, column: &str) -> Result<&Self, BundlebaseError>;

    /// Rebuild an index on a column.
    async fn rebuild_index(&self, column: &str) -> Result<&Self, BundlebaseError>;

    /// Creates index files for anything missing based on the defined indexes.
    async fn reindex(&self) -> Result<&Self, BundlebaseError>;

    /// Verify the integrity of all files in the bundle by checking SHA256 hashes.
    async fn verify_data(
        &self,
        update_versions: bool,
    ) -> Result<bundlebase::bundle::verification::VerificationResults, BundlebaseError>;
}

#[async_trait]
impl BundleBuilderExt for BundleBuilder {
    async fn attach(&self, path: &str, pack: Option<&str>) -> Result<&Self, BundlebaseError> {
        exec_cmd(self, AttachCommand::new(path, pack.map(|s| s.to_string())))
            .await?;
        Ok(self)
    }

    async fn detach_block(&self, location: &str) -> Result<&Self, BundlebaseError> {
        exec_cmd(self,DetachBlockCommand::new(location))
            .await?;
        Ok(self)
    }

    async fn replace_block(
        &self,
        old_location: &str,
        new_location: &str,
    ) -> Result<&Self, BundlebaseError> {
        exec_cmd(self,ReplaceBlockCommand::new(old_location, new_location))
            .await?;
        Ok(self)
    }

    async fn create_source(
        &self,
        connector: &str,
        args: HashMap<String, String>,
        pack: Option<&str>,
    ) -> Result<&Self, BundlebaseError> {
        exec_cmd(self,CreateSourceCommand::new(
            connector,
            args,
            pack.map(|s| s.to_string()),
        ))
        .await?;
        Ok(self)
    }

    async fn import_connector(
        &self,
        name: &str,
        from: &str,
        platform: &str,
    ) -> Result<&Self, BundlebaseError> {
        let platform: Platform = platform.parse()?;
        exec_cmd(self,ImportConnectorCommand::new(name, from, platform))
            .await?;
        Ok(self)
    }

    async fn rename_connector(
        &self,
        old_name: &str,
        new_name: &str,
    ) -> Result<&Self, BundlebaseError> {
        exec_cmd(self,RenameConnectorCommand::new(old_name, new_name))
            .await?;
        Ok(self)
    }

    async fn drop_connector(
        &self,
        name: &str,
        platform: Option<&str>,
    ) -> Result<&Self, BundlebaseError> {
        let platform: Option<Platform> = platform.map(|s| s.parse()).transpose()?;
        exec_cmd(self,DropConnectorCommand::new(name, platform))
            .await?;
        Ok(self)
    }

    async fn fetch(
        &self,
        pack: &str,
        mode: SyncMode,
    ) -> Result<Vec<FetchResults>, BundlebaseError> {
        exec_cmd(self,FetchCommand::new(pack.to_string(), mode))
            .await
    }

    async fn fetch_all(&self, mode: SyncMode) -> Result<Vec<FetchResults>, BundlebaseError> {
        exec_cmd(self,FetchAllCommand::new(mode))
            .await
    }

    async fn create_view(
        &self,
        name: &str,
        sql: &str,
    ) -> Result<Arc<BundleBuilder>, BundlebaseError> {
        use bundlebase::bundle::operation::CreateViewOp;
        use tracing::info;

        self.check_no_temp_functions_in_sql(sql, "view")?;

        let name_clone = name.to_string();
        let sql_clone = sql.to_string();

        // Use a cell to capture the view_builder from inside the closure.
        let view_builder_cell: Arc<parking_lot::RwLock<Option<Arc<BundleBuilder>>>> =
            Arc::new(parking_lot::RwLock::new(None));
        let view_builder_cell_clone = view_builder_cell.clone();

        self.do_change(&format!("Create view '{}'", name), |builder| {
            let name = name_clone.clone();
            let sql = sql_clone.clone();
            let cell = view_builder_cell_clone.clone();
            Box::pin(async move {
                let (op, view_builder) = CreateViewOp::setup(&name, &sql, builder).await?;
                *cell.write() = Some(view_builder);
                builder.apply_operation(op.into()).await?;
                info!("Created view '{}'", name);
                Ok(())
            })
        })
        .await?;

        // Extract the view builder from the cell
        let view_builder = view_builder_cell
            .read()
            .clone()
            .ok_or_else(|| BundlebaseError::from("View builder not created"))?;

        Ok(view_builder)
    }

    async fn rename_view(
        &self,
        old_name: &str,
        new_name: &str,
    ) -> Result<&Self, BundlebaseError> {
        exec_cmd(self,RenameViewCommand::new(old_name, new_name))
            .await?;
        Ok(self)
    }

    async fn drop_view(&self, view_name: &str) -> Result<&Self, BundlebaseError> {
        exec_cmd(self,DropViewCommand::new(view_name))
            .await?;
        Ok(self)
    }

    async fn drop_join(&self, join_name: &str) -> Result<&Self, BundlebaseError> {
        exec_cmd(self,DropJoinCommand::new(join_name))
            .await?;
        Ok(self)
    }

    async fn rename_join(
        &self,
        old_name: &str,
        new_name: &str,
    ) -> Result<&Self, BundlebaseError> {
        exec_cmd(self,RenameJoinCommand::new(old_name, new_name))
            .await?;
        Ok(self)
    }

    async fn drop_column(&self, name: &str) -> Result<&Self, BundlebaseError> {
        exec_cmd(self,DropColumnCommand::new(name))
            .await?;
        Ok(self)
    }

    async fn rename_column(
        &self,
        old_name: &str,
        new_name: &str,
    ) -> Result<&Self, BundlebaseError> {
        exec_cmd(self,RenameColumnCommand::new(old_name, new_name))
            .await?;
        Ok(self)
    }

    async fn add_column(
        &self,
        name: &str,
        expression: &str,
    ) -> Result<&Self, BundlebaseError> {
        exec_cmd(self,AddColumnCommand::new(name, expression))
            .await?;
        Ok(self)
    }

    async fn cast_column(
        &self,
        name: &str,
        new_type: &str,
        clean: Option<String>,
    ) -> Result<&Self, BundlebaseError> {
        exec_cmd(self,CastColumnCommand::new(name, new_type, clean))
            .await?;
        Ok(self)
    }

    async fn standardize_column_names(&self) -> Result<&Self, BundlebaseError> {
        exec_cmd(self,StandardizeColumnNamesCommand)
            .await?;
        Ok(self)
    }

    async fn filter(
        &self,
        query: &str,
        params: Vec<ScalarValue>,
    ) -> Result<&Self, BundlebaseError> {
        exec_cmd(self,FilterCommand::new(query, params))
            .await?;
        Ok(self)
    }

    async fn delete(&self, where_clause: &str) -> Result<usize, BundlebaseError> {
        let result = exec_cmd(self, DeleteCommand::new(where_clause)).await?;
        // Parse count from "Deleted N rows"
        let count = result.split_whitespace().nth(1)
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0);
        Ok(count)
    }

    async fn join(
        &self,
        name: &str,
        expression: &str,
        location: Option<&str>,
        join_type: JoinTypeOption,
    ) -> Result<&Self, BundlebaseError> {
        exec_cmd(self,JoinCommand::new(
            name,
            expression,
            location.map(|s| s.to_string()),
            join_type,
        ))
        .await?;
        Ok(self)
    }

    async fn import_function(
        &self,
        name: &str,
        from: &str,
        platform: &str,
    ) -> Result<&Self, BundlebaseError> {
        let platform: Platform = platform.parse()?;
        exec_cmd(self,ImportFunctionCommand::new(name, from, platform))
            .await?;
        Ok(self)
    }

    async fn rename_function(
        &self,
        old_name: &str,
        new_name: &str,
    ) -> Result<&Self, BundlebaseError> {
        exec_cmd(self,RenameFunctionCommand::new(old_name, new_name))
            .await?;
        Ok(self)
    }

    async fn drop_function(
        &self,
        name: &str,
        platform: Option<&str>,
    ) -> Result<&Self, BundlebaseError> {
        let platform: Option<Platform> = platform.map(|s| s.parse()).transpose()?;
        exec_cmd(self,DropFunctionCommand::new(name, platform))
            .await?;
        Ok(self)
    }

    async fn set_name(&self, name: &str) -> Result<&Self, BundlebaseError> {
        exec_cmd(self,SetNameCommand::new(name))
            .await?;
        Ok(self)
    }

    async fn set_description(&self, description: &str) -> Result<&Self, BundlebaseError> {
        exec_cmd(self,SetDescriptionCommand::new(description))
            .await?;
        Ok(self)
    }

    async fn save_config(
        &self,
        scope: &Scope,
        key: &str,
        value: &str,
    ) -> Result<&Self, BundlebaseError> {
        exec_cmd(self,SaveConfigCommand::new(scope.clone(), key, value))
            .await?;
        Ok(self)
    }

    async fn create_index(
        &self,
        columns: &[&str],
        index_type: IndexType,
        name: Option<&str>,
    ) -> Result<&Self, BundlebaseError> {
        let cols: Vec<String> = columns.iter().map(|s| s.to_string()).collect();
        exec_cmd(self,CreateIndexCommand::new(
            cols,
            index_type,
            name.map(|s| s.to_string()),
        ))
        .await?;
        Ok(self)
    }

    async fn drop_index(&self, column: &str) -> Result<&Self, BundlebaseError> {
        exec_cmd(self,DropIndexCommand::new(column))
            .await?;
        Ok(self)
    }

    async fn rebuild_index(&self, column: &str) -> Result<&Self, BundlebaseError> {
        exec_cmd(self,RebuildIndexCommand::new(column))
            .await?;
        Ok(self)
    }

    async fn reindex(&self) -> Result<&Self, BundlebaseError> {
        exec_cmd(self,ReindexCommand::new())
            .await?;
        Ok(self)
    }

    async fn verify_data(
        &self,
        update_versions: bool,
    ) -> Result<bundlebase::bundle::verification::VerificationResults, BundlebaseError> {
        let cmd = VerifyDataCommand::new(update_versions);
        Box::new(cmd).execute(self).await
    }
}

/// Implementation for `Arc<BundleBuilder>` — delegates to inner BundleBuilder.
#[async_trait]
impl BundleBuilderExt for Arc<BundleBuilder> {
    async fn attach(&self, path: &str, pack: Option<&str>) -> Result<&Self, BundlebaseError> {
        (**self).attach(path, pack).await?; Ok(self)
    }
    async fn detach_block(&self, location: &str) -> Result<&Self, BundlebaseError> {
        (**self).detach_block(location).await?; Ok(self)
    }
    async fn replace_block(&self, old_location: &str, new_location: &str) -> Result<&Self, BundlebaseError> {
        (**self).replace_block(old_location, new_location).await?; Ok(self)
    }
    async fn create_source(&self, connector: &str, args: HashMap<String, String>, pack: Option<&str>) -> Result<&Self, BundlebaseError> {
        (**self).create_source(connector, args, pack).await?; Ok(self)
    }
    async fn import_connector(&self, name: &str, from: &str, platform: &str) -> Result<&Self, BundlebaseError> {
        (**self).import_connector(name, from, platform).await?; Ok(self)
    }
    async fn rename_connector(&self, old_name: &str, new_name: &str) -> Result<&Self, BundlebaseError> {
        (**self).rename_connector(old_name, new_name).await?; Ok(self)
    }
    async fn drop_connector(&self, name: &str, platform: Option<&str>) -> Result<&Self, BundlebaseError> {
        (**self).drop_connector(name, platform).await?; Ok(self)
    }
    async fn fetch(&self, pack: &str, mode: SyncMode) -> Result<Vec<FetchResults>, BundlebaseError> {
        (**self).fetch(pack, mode).await
    }
    async fn fetch_all(&self, mode: SyncMode) -> Result<Vec<FetchResults>, BundlebaseError> {
        (**self).fetch_all(mode).await
    }
    async fn create_view(&self, name: &str, sql: &str) -> Result<Arc<BundleBuilder>, BundlebaseError> {
        (**self).create_view(name, sql).await
    }
    async fn rename_view(&self, old_name: &str, new_name: &str) -> Result<&Self, BundlebaseError> {
        (**self).rename_view(old_name, new_name).await?; Ok(self)
    }
    async fn drop_view(&self, view_name: &str) -> Result<&Self, BundlebaseError> {
        (**self).drop_view(view_name).await?; Ok(self)
    }
    async fn drop_join(&self, join_name: &str) -> Result<&Self, BundlebaseError> {
        (**self).drop_join(join_name).await?; Ok(self)
    }
    async fn rename_join(&self, old_name: &str, new_name: &str) -> Result<&Self, BundlebaseError> {
        (**self).rename_join(old_name, new_name).await?; Ok(self)
    }
    async fn drop_column(&self, name: &str) -> Result<&Self, BundlebaseError> {
        (**self).drop_column(name).await?; Ok(self)
    }
    async fn rename_column(&self, old_name: &str, new_name: &str) -> Result<&Self, BundlebaseError> {
        (**self).rename_column(old_name, new_name).await?; Ok(self)
    }
    async fn add_column(&self, name: &str, expression: &str) -> Result<&Self, BundlebaseError> {
        (**self).add_column(name, expression).await?; Ok(self)
    }
    async fn cast_column(&self, name: &str, new_type: &str, clean: Option<String>) -> Result<&Self, BundlebaseError> {
        (**self).cast_column(name, new_type, clean).await?; Ok(self)
    }
    async fn standardize_column_names(&self) -> Result<&Self, BundlebaseError> {
        (**self).standardize_column_names().await?; Ok(self)
    }
    async fn filter(&self, query: &str, params: Vec<ScalarValue>) -> Result<&Self, BundlebaseError> {
        (**self).filter(query, params).await?; Ok(self)
    }
    async fn delete(&self, where_clause: &str) -> Result<usize, BundlebaseError> {
        (**self).delete(where_clause).await
    }
    async fn join(&self, name: &str, expression: &str, location: Option<&str>, join_type: JoinTypeOption) -> Result<&Self, BundlebaseError> {
        (**self).join(name, expression, location, join_type).await?; Ok(self)
    }
    async fn import_function(&self, name: &str, from: &str, platform: &str) -> Result<&Self, BundlebaseError> {
        (**self).import_function(name, from, platform).await?; Ok(self)
    }
    async fn rename_function(&self, old_name: &str, new_name: &str) -> Result<&Self, BundlebaseError> {
        (**self).rename_function(old_name, new_name).await?; Ok(self)
    }
    async fn drop_function(&self, name: &str, platform: Option<&str>) -> Result<&Self, BundlebaseError> {
        (**self).drop_function(name, platform).await?; Ok(self)
    }
    async fn set_name(&self, name: &str) -> Result<&Self, BundlebaseError> {
        (**self).set_name(name).await?; Ok(self)
    }
    async fn set_description(&self, description: &str) -> Result<&Self, BundlebaseError> {
        (**self).set_description(description).await?; Ok(self)
    }
    async fn save_config(&self, scope: &Scope, key: &str, value: &str) -> Result<&Self, BundlebaseError> {
        (**self).save_config(scope, key, value).await?; Ok(self)
    }
    async fn create_index(&self, columns: &[&str], index_type: IndexType, name: Option<&str>) -> Result<&Self, BundlebaseError> {
        (**self).create_index(columns, index_type, name).await?; Ok(self)
    }
    async fn drop_index(&self, column: &str) -> Result<&Self, BundlebaseError> {
        (**self).drop_index(column).await?; Ok(self)
    }
    async fn rebuild_index(&self, column: &str) -> Result<&Self, BundlebaseError> {
        (**self).rebuild_index(column).await?; Ok(self)
    }
    async fn reindex(&self) -> Result<&Self, BundlebaseError> {
        (**self).reindex().await?; Ok(self)
    }
    async fn verify_data(&self, update_versions: bool) -> Result<bundlebase::bundle::verification::VerificationResults, BundlebaseError> {
        (**self).verify_data(update_versions).await
    }
}
