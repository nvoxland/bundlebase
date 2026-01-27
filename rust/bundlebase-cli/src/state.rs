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
    /// Read-only mode with an immutable Bundle (already in Arc from Bundle::open).
    ReadOnly(Arc<Bundle>),
    /// Read-write mode with a BundleBuilder (uses interior mutability).
    ReadWrite(Arc<BundleBuilder>),
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
    pub fn read_only(bundle: Arc<Bundle>) -> Self {
        Self {
            mode: BundleMode::ReadOnly(bundle),
        }
    }

    /// Create a new read-write state wrapping a bundle builder.
    pub fn read_write(builder: Arc<BundleBuilder>) -> Self {
        Self {
            mode: BundleMode::ReadWrite(builder),
        }
    }

    /// Create a new state wrapping a bundle builder (legacy API, same as read_write).
    #[deprecated(since = "0.5.0", note = "Use read_write() instead")]
    pub fn new(bundle: Arc<BundleBuilder>) -> Self {
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
                        BundleMode::ReadOnly(bundle) => {
                            // In read-only mode, only facade commands are allowed
                            let facade_cmd = cmd.into_facade_command()?;
                            let output = facade_cmd.execute(bundle.as_ref()).await?;
                            Ok(SqlResult::Output(output))
                        }
                        BundleMode::ReadWrite(builder) => {
                            // In read-write mode, all commands are allowed
                            // BundleBuilder uses interior mutability, so we can use &self methods
                            let output = cmd.execute(builder.as_ref()).await?;
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
            BundleMode::ReadOnly(bundle) => {
                if Self::references_bundle_table(sql) {
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
            BundleMode::ReadWrite(builder) => {
                if Self::references_bundle_table(sql) {
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

    /// Check if SQL references the "bundle" table (not bundle_info or other bundle_* tables).
    ///
    /// Uses word boundary detection to avoid false positives like "bundle_info" matching.
    fn references_bundle_table(sql: &str) -> bool {
        let sql_upper = sql.to_uppercase();

        // Check for "FROM BUNDLE" or "JOIN BUNDLE" followed by a word boundary
        // (whitespace, comma, closing paren, semicolon, end of string, or WHERE/ORDER/etc)
        for pattern in ["FROM BUNDLE", "JOIN BUNDLE"] {
            if let Some(pos) = sql_upper.find(pattern) {
                let after_pos = pos + pattern.len();
                if after_pos >= sql_upper.len() {
                    // Pattern at end of string
                    return true;
                }
                let next_char = sql_upper.chars().nth(after_pos);
                match next_char {
                    // Word boundary characters that indicate "bundle" is the full table name
                    Some(' ') | Some('\t') | Some('\n') | Some('\r') | Some(',') | Some(')')
                    | Some(';') => return true,
                    // Not a word boundary - could be "bundle_info" etc.
                    _ => continue,
                }
            }
        }
        false
    }

    // =========================================================================
    // Facade methods - delegate to the underlying Bundle or BundleBuilder
    // =========================================================================

    /// Get the bundle schema.
    pub async fn schema(&self) -> Result<SchemaRef, BundlebaseError> {
        match &self.mode {
            BundleMode::ReadOnly(bundle) => bundle.schema().await,
            BundleMode::ReadWrite(builder) => builder.schema().await,
        }
    }

    /// Get the number of rows in the bundle.
    pub async fn num_rows(&self) -> Result<usize, BundlebaseError> {
        match &self.mode {
            BundleMode::ReadOnly(bundle) => bundle.num_rows().await,
            BundleMode::ReadWrite(builder) => builder.num_rows().await,
        }
    }

    /// Get the commit history.
    pub fn history(&self) -> Vec<BundleCommit> {
        match &self.mode {
            BundleMode::ReadOnly(bundle) => bundle.history(),
            BundleMode::ReadWrite(builder) => builder.history(),
        }
    }

    /// Get the bundle status (uncommitted changes).
    ///
    /// Returns `None` for read-only bundles (no uncommitted changes possible).
    pub fn status(&self) -> Option<BundleStatus> {
        match &self.mode {
            BundleMode::ReadOnly(_) => None,
            BundleMode::ReadWrite(builder) => Some(builder.status().clone()),
        }
    }

    /// Get a clone of the dataframe.
    pub async fn dataframe(
        &self,
    ) -> Result<std::sync::Arc<datafusion::prelude::DataFrame>, BundlebaseError> {
        match &self.mode {
            BundleMode::ReadOnly(bundle) => bundle.dataframe().await,
            BundleMode::ReadWrite(builder) => builder.dataframe().await,
        }
    }

    /// Get the bundle URL.
    pub fn url(&self) -> String {
        match &self.mode {
            BundleMode::ReadOnly(bundle) => bundle.url().to_string(),
            BundleMode::ReadWrite(builder) => builder.bundle().url().to_string(),
        }
    }

    /// Get the schema for a SQL query by planning it.
    ///
    /// This method is used to determine the output schema of a query without
    /// actually executing it. Useful for protocols that need to describe result
    /// sets upfront.
    pub async fn get_query_schema(&self, sql: &str) -> Result<SchemaRef, BundlebaseError> {
        match &self.mode {
            BundleMode::ReadOnly(bundle) => {
                if Self::references_bundle_table(sql) {
                    let result_builder = bundle.select(sql, vec![]).await?;
                    let df = result_builder.dataframe().await?;
                    Ok(df.schema().inner().clone())
                } else {
                    // Execute directly via the SessionContext for non-bundle queries
                    let ctx = bundle.ctx();
                    let df = ctx.sql(sql).await?;
                    Ok(df.schema().inner().clone())
                }
            }
            BundleMode::ReadWrite(builder) => {
                if Self::references_bundle_table(sql) {
                    let result_builder = builder.select(sql, vec![]).await?;
                    let df = result_builder.dataframe().await?;
                    Ok(df.schema().inner().clone())
                } else {
                    // Execute directly via the SessionContext for non-bundle queries
                    let ctx = builder.bundle().ctx();
                    let df = ctx.sql(sql).await?;
                    Ok(df.schema().inner().clone())
                }
            }
        }
    }

    // =========================================================================
    // Read-write only methods
    // =========================================================================

    /// Get a reference to the builder (read-write mode only).
    ///
    /// Returns `None` in read-only mode.
    pub fn builder(&self) -> Option<&Arc<BundleBuilder>> {
        match &self.mode {
            BundleMode::ReadOnly(_) => None,
            BundleMode::ReadWrite(builder) => Some(builder),
        }
    }
}

// Type alias for backwards compatibility
#[deprecated(since = "0.4.0", note = "Use BundleState instead")]
pub type State = BundleState;
