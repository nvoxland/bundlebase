//! Shared state for the bundlebase CLI.
//!
//! This module provides the `BundleState` type that wraps either a `Bundle` (read-only)
//! or `BundleBuilder` (read-write) with thread-safe access for use across different
//! CLI modes (REPL, Flight, etc.).

use arrow_schema::SchemaRef;
use bundlebase::bundle::{
    is_command_statement, parse_command, BundleCommit, BundleFacade, BundleStatus, CommandOutput,
};
use bundlebase::{Bundle, BundleBuilder, BundlebaseError};
use datafusion::execution::SendableRecordBatchStream;
use parking_lot::RwLock;
use std::sync::Arc;

/// Result of SQL execution.
///
/// Distinguishes between streaming query results (SELECT) and
/// command outputs (BundleCommands like ATTACH, FILTER, etc.).
pub enum SqlResult {
    /// Streaming result from a SELECT query.
    Stream(SendableRecordBatchStream),
    /// Command output from a BundleCommand.
    Output(CommandOutput),
}

/// Mode of the bundle state - read-only or read-write.
enum BundleMode {
    /// Read-only mode with an immutable Bundle.
    ReadOnly(RwLock<Bundle>),
    /// Read-write mode with a mutable BundleBuilder.
    ReadWrite(RwLock<BundleBuilder>),
}

/// Shared state containing the bundle being worked on.
///
/// This type is designed to be wrapped in an `Arc` and shared across
/// async tasks and different CLI components. It supports two modes:
///
/// - **Read-only mode**: Wraps a `Bundle` and only allows read-only commands
///   like SELECT, EXPLAIN PLAN, and standard SQL queries.
/// - **Read-write mode**: Wraps a `BundleBuilder` and allows all commands
///   including ATTACH, FILTER, COMMIT, etc.
pub struct BundleState {
    mode: BundleMode,
}

impl BundleState {
    /// Create a new read-only state wrapping a bundle.
    pub fn read_only(bundle: Bundle) -> Self {
        Self {
            mode: BundleMode::ReadOnly(RwLock::new(bundle)),
        }
    }

    /// Create a new read-write state wrapping a bundle builder.
    pub fn read_write(builder: BundleBuilder) -> Self {
        Self {
            mode: BundleMode::ReadWrite(RwLock::new(builder)),
        }
    }

    /// Create a new state wrapping a bundle builder (legacy API, same as read_write).
    #[deprecated(since = "0.5.0", note = "Use read_write() instead")]
    pub fn new(bundle: BundleBuilder) -> Self {
        Self::read_write(bundle)
    }

    /// Returns true if this state is in read-only mode.
    pub fn is_read_only(&self) -> bool {
        matches!(self.mode, BundleMode::ReadOnly(_))
    }

    /// Execute SQL against the bundle state.
    ///
    /// This function handles both bundlebase commands (ATTACH, FILTER, etc.)
    /// and standard SQL queries (SELECT). For bundlebase commands, it parses
    /// and executes them returning a `CommandOutput`. For SELECT queries
    /// (that are not bundlebase SELECTs), it executes via DataFusion and
    /// returns a streaming result.
    ///
    /// In read-only mode, only facade commands (SELECT, EXPLAIN PLAN) and
    /// standard SQL queries are allowed. Mutating commands will return an error.
    pub async fn execute_sql(&self, sql: &str) -> Result<SqlResult, BundlebaseError> {
        let sql = sql.trim();

        // Check if this is a bundlebase command (ATTACH, FILTER, etc. - but NOT SELECT)
        if is_command_statement(sql) {
            // Try to parse as a bundlebase command
            match parse_command(sql) {
                Ok(cmd) => {
                    match &self.mode {
                        BundleMode::ReadOnly(lock) => {
                            // In read-only mode, only facade commands are allowed
                            let facade_cmd = cmd.into_facade_command()?;
                            // Clone the bundle to avoid holding lock across await
                            let bundle = lock.read().clone();
                            let output = facade_cmd.execute(&bundle).await?;
                            Ok(SqlResult::Output(output))
                        }
                        BundleMode::ReadWrite(lock) => {
                            // In read-write mode, all commands are allowed
                            // Clone-modify-writeback pattern
                            let mut builder = lock.read().clone();
                            let output = cmd.execute(&mut builder).await?;
                            *lock.write() = builder;
                            Ok(SqlResult::Output(output))
                        }
                    }
                }
                Err(e) => {
                    // If parsing fails, it might be standard SQL that happens to start
                    // with one of our keywords. Try as standard SQL.
                    let err_msg = e.to_string();
                    if err_msg.contains("Syntax error") {
                        self.execute_standard_sql(sql).await
                    } else {
                        Err(e)
                    }
                }
            }
        } else {
            // Standard SQL (including SELECT) - execute directly via DataFusion
            self.execute_standard_sql(sql).await
        }
    }

    /// Execute standard SQL via DataFusion.
    async fn execute_standard_sql(&self, sql: &str) -> Result<SqlResult, BundlebaseError> {
        match &self.mode {
            BundleMode::ReadOnly(lock) => {
                // Clone the bundle to avoid holding lock across await
                let bundle = lock.read().clone();
                let sql_upper = sql.to_uppercase();

                if sql_upper.contains("FROM BUNDLE") || sql_upper.contains("JOIN BUNDLE") {
                    // Use select() to get the dataframe with all operations applied
                    let result_builder = bundle.select(sql, vec![]).await?;
                    let df = result_builder.dataframe().await?;
                    let stream = df.as_ref().clone().execute_stream().await?;
                    Ok(SqlResult::Stream(stream))
                } else {
                    // Execute directly via the SessionContext for non-bundle queries
                    let ctx = bundle.ctx();
                    let df = ctx.sql(sql).await?;
                    let stream = df.execute_stream().await?;
                    Ok(SqlResult::Stream(stream))
                }
            }
            BundleMode::ReadWrite(lock) => {
                let builder = lock.read().clone();
                let sql_upper = sql.to_uppercase();

                if sql_upper.contains("FROM BUNDLE") || sql_upper.contains("JOIN BUNDLE") {
                    // Use builder.select() for bundle-referencing queries
                    let result_builder = builder.select(sql, vec![]).await?;
                    let df = result_builder.dataframe().await?;
                    let stream = df.as_ref().clone().execute_stream().await?;
                    Ok(SqlResult::Stream(stream))
                } else {
                    // Execute directly via the SessionContext for non-bundle queries
                    let ctx = builder.bundle().ctx();
                    let df = ctx.sql(sql).await?;
                    let stream = df.execute_stream().await?;
                    Ok(SqlResult::Stream(stream))
                }
            }
        }
    }

    // =========================================================================
    // Facade methods - delegate to the underlying Bundle or BundleBuilder
    // =========================================================================

    /// Get the bundle schema.
    pub async fn schema(&self) -> Result<SchemaRef, BundlebaseError> {
        // Clone the Arc to avoid holding the guard across await
        let facade: Arc<dyn BundleFacade> = match &self.mode {
            BundleMode::ReadOnly(lock) => Arc::new(lock.read().clone()),
            BundleMode::ReadWrite(lock) => Arc::new(lock.read().clone()),
        };
        facade.schema().await
    }

    /// Get the number of rows in the bundle.
    pub async fn num_rows(&self) -> Result<usize, BundlebaseError> {
        match &self.mode {
            BundleMode::ReadOnly(lock) => lock.read().num_rows().await,
            BundleMode::ReadWrite(lock) => lock.read().num_rows().await,
        }
    }

    /// Get the commit history.
    pub fn history(&self) -> Vec<BundleCommit> {
        match &self.mode {
            BundleMode::ReadOnly(lock) => lock.read().history(),
            BundleMode::ReadWrite(lock) => lock.read().history(),
        }
    }

    /// Get the bundle status (uncommitted changes).
    ///
    /// Returns `None` for read-only bundles (no uncommitted changes possible).
    pub fn status(&self) -> Option<BundleStatus> {
        match &self.mode {
            BundleMode::ReadOnly(_) => None,
            BundleMode::ReadWrite(lock) => Some(lock.read().status().clone()),
        }
    }

    /// Get a clone of the dataframe.
    pub async fn dataframe(
        &self,
    ) -> Result<std::sync::Arc<datafusion::prelude::DataFrame>, BundlebaseError> {
        match &self.mode {
            BundleMode::ReadOnly(lock) => lock.read().dataframe().await,
            BundleMode::ReadWrite(lock) => lock.read().dataframe().await,
        }
    }

    /// Get the bundle URL.
    pub fn url(&self) -> String {
        match &self.mode {
            BundleMode::ReadOnly(lock) => lock.read().url().to_string(),
            BundleMode::ReadWrite(lock) => lock.read().bundle().url().to_string(),
        }
    }

    /// Get the schema for a SQL query by planning it.
    ///
    /// This method is used to determine the output schema of a query without
    /// actually executing it. Useful for protocols that need to describe result
    /// sets upfront.
    pub async fn get_query_schema(&self, sql: &str) -> Result<SchemaRef, BundlebaseError> {
        match &self.mode {
            BundleMode::ReadOnly(lock) => {
                let bundle = lock.read().clone();
                let result_builder = bundle.select(sql, vec![]).await?;
                let df = result_builder.dataframe().await?;
                Ok(df.schema().inner().clone())
            }
            BundleMode::ReadWrite(lock) => {
                let builder = lock.read().clone();
                let result_builder = builder.select(sql, vec![]).await?;
                let df = result_builder.dataframe().await?;
                Ok(df.schema().inner().clone())
            }
        }
    }

    // =========================================================================
    // Read-write only methods
    // =========================================================================

    /// Get a mutable reference to the builder (read-write mode only).
    ///
    /// Returns `None` in read-only mode.
    pub fn builder(&self) -> Option<parking_lot::RwLockWriteGuard<'_, BundleBuilder>> {
        match &self.mode {
            BundleMode::ReadOnly(_) => None,
            BundleMode::ReadWrite(lock) => Some(lock.write()),
        }
    }

    /// Get a read reference to the builder (read-write mode only).
    ///
    /// Returns `None` in read-only mode.
    pub fn builder_read(&self) -> Option<parking_lot::RwLockReadGuard<'_, BundleBuilder>> {
        match &self.mode {
            BundleMode::ReadOnly(_) => None,
            BundleMode::ReadWrite(lock) => Some(lock.read()),
        }
    }
}

// Type alias for backwards compatibility
#[deprecated(since = "0.4.0", note = "Use BundleState instead")]
pub type State = BundleState;
