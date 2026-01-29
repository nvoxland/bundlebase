use super::command::parser::{is_command_statement, parse_command};
use super::command::{BundleCommand, FacadeCommand, CommandResponse, OutputShape};
use super::operation::BundleChange;
use crate::bundle::BundleCommit;
use crate::bundle::BundleStatus;
use crate::bundle::Pack;
use crate::index::IndexDefinition;
use crate::io::{IOReadWriteDir, ObjectId};
use crate::{AnyOperation, Bundle, BundleBuilder, BundleConfig, BundlebaseError};
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use datafusion::common::ScalarValue;
use datafusion::dataframe::DataFrame;
use datafusion::execution::SendableRecordBatchStream;
use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
use datafusion::prelude::SessionContext;
use std::collections::HashMap;
use std::sync::Arc;
use url::Url;

#[async_trait]
pub trait BundleFacade: Send + Sync {
    /// The id of the bundle
    fn id(&self) -> String;

    /// Retrieve the bundle name, if set.
    fn name(&self) -> Option<String>;

    /// Retrieve the bundle description, if set.
    fn description(&self) -> Option<String>;

    /// Retrieve the URL of the base bundle this was loaded from, if any.
    fn url(&self) -> Url;

    /// The base bundle this was extended from
    fn from(&self) -> Option<Url>;

    /// Unique version for this bundle
    fn version(&self) -> String;

    /// Returns the commit history for this bundle, including any base bundles
    fn history(&self) -> Vec<BundleCommit>;

    /// All operations applied to this bundle
    fn operations(&self) -> Vec<AnyOperation>;

    async fn schema(&self) -> Result<SchemaRef, BundlebaseError>;

    /// Computes the number of rows in the bundle
    async fn num_rows(&self) -> Result<usize, BundlebaseError>;

    /// Builds and returns the final DataFrame
    async fn dataframe(&self) -> Result<Arc<DataFrame>, BundlebaseError>;

    // todo: don't extend bundles when uncommitted changes
    /// Extends this bundle to create a new BundleBuilder.
    ///
    /// This is the primary way to create a new BundleBuilder from an existing bundle.
    /// The new builder can optionally have a different data directory.
    ///
    /// # Arguments
    /// * `data_dir` - Optional new data directory. If None, uses the current bundle's data_dir.
    ///
    /// # Returns
    /// A new BundleBuilder extending from this bundle.
    ///
    /// # Example
    /// ```ignore
    /// // Extend with a new data directory
    /// let builder = bundle.extend(Some("s3://bucket/new"))?;
    ///
    /// // Extend and then filter
    /// let builder = bundle.extend(None)?;
    /// builder.filter("active = true", vec![]).await?;
    /// ```
    fn extend(
        &self,
        data_dir: Option<&str>,
    ) -> Result<Arc<BundleBuilder>, BundlebaseError>;

    /// Executes a SQL query and returns streaming results directly.
    ///
    /// Unlike `extend()` with SQL, this does NOT create a new BundleBuilder.
    /// It directly executes the query and streams the results. Use this when
    /// you want to read data from the bundle without creating a new builder.
    ///
    /// # Arguments
    /// * `sql` - SQL query string (e.g., "SELECT * FROM bundle WHERE id > 10")
    /// * `params` - Query parameters for parameterized queries ($1, $2, etc.)
    ///
    /// # Returns
    /// A streaming result set that can be consumed incrementally.
    ///
    /// # Example
    /// ```ignore
    /// let stream = bundle.query("SELECT COUNT(*) FROM bundle", vec![]).await?;
    /// while let Some(batch) = stream.next().await {
    ///     // Process batch
    /// }
    /// ```
    async fn query(
        &self,
        sql: &str,
        params: Vec<ScalarValue>,
    ) -> Result<SendableRecordBatchStream, BundlebaseError>;

    /// Execute a SQL statement or command, returning streaming results.
    ///
    /// This unified method handles both regular SQL queries and bundlebase commands
    /// (like ATTACH, FILTER, EXPLAIN, etc.), always returning a `SendableRecordBatchStream`.
    ///
    /// # Arguments
    /// * `sql` - SQL query or command string
    /// * `params` - Query parameters for parameterized queries ($1, $2, etc.)
    ///
    /// # Returns
    /// A streaming result set. For commands, the output is converted to a single-batch stream.
    ///
    /// # Example
    /// ```ignore
    /// // Regular query
    /// let stream = bundle.execute("SELECT * FROM bundle", vec![]).await?;
    ///
    /// // Command
    /// let stream = bundle.execute("ATTACH 'data.csv'", vec![]).await?;
    /// ```
    async fn execute(
        &self,
        sql: &str,
        params: Vec<ScalarValue>,
    ) -> Result<SendableRecordBatchStream, BundlebaseError> {
        if is_command_statement(sql) {
            // Parse and execute as command via execute_command (object-safe)
            let cmd = parse_command(sql)?;
            let output = self.execute_command(cmd).await?;

            // Convert to stream using dyn methods
            let schema = output.dyn_schema();
            let batch = output.to_record_batch()?;
            let stream = futures::stream::iter(vec![Ok(batch)]);
            Ok(Box::pin(RecordBatchStreamAdapter::new(schema, stream)))
        } else {
            // Execute as regular SQL query
            self.query(sql, params).await
        }
    }

    /// Get the schema and output shape that will be returned by executing a SQL statement.
    ///
    /// This method determines the output schema and display shape without executing the statement.
    /// For bundlebase commands (ATTACH, FILTER, etc.), it parses the command and
    /// returns the known schema and shape. For regular SQL queries, it plans the query to
    /// determine the schema and returns `OutputShape::Table`.
    ///
    /// # Arguments
    /// * `sql` - SQL statement or bundlebase command
    ///
    /// # Returns
    /// * `Ok((SchemaRef, OutputShape))` - The schema and output shape of the result
    /// * `Err(BundlebaseError)` - Invalid statement or planning failed
    ///
    /// # Example
    /// ```ignore
    /// // Get schema for a command
    /// let (schema, shape) = bundle.response_schema("EXPLAIN PLAN").await?;
    ///
    /// // Get schema for a SQL query
    /// let (schema, shape) = bundle.response_schema("SELECT id, name FROM bundle").await?;
    /// ```
    async fn response_schema(&self, sql: &str) -> Result<(SchemaRef, OutputShape), BundlebaseError> {
        let sql = sql.trim();

        if is_command_statement(sql) {
            // Parse command and return its known schema and shape
            let cmd = parse_command(sql)?;
            Ok((cmd.output_schema(), cmd.output_shape()))
        } else {
            // Plan the SQL query to get its schema; SQL queries are always tables
            let stream = self.query(sql, vec![]).await?;
            Ok((stream.schema().clone(), OutputShape::Table))
        }
    }

    /// Returns a map of view IDs to view names for all views in this container
    fn views(&self) -> HashMap<ObjectId, String>;

    /// Open a view by name or ID, returning a read-only Bundle
    ///
    /// Looks up the view by name or ID and opens it as a Bundle. The view automatically
    /// inherits all changes from its parent bundle through the FROM mechanism.
    ///
    /// # Arguments
    /// * `identifier` - Name or ID of the view to open
    ///
    /// # Returns
    /// A read-only Bundle representing the view
    ///
    /// # Errors
    /// Returns an error if the view doesn't exist or if the identifier is ambiguous
    ///
    /// # Example
    /// ```no_run
    /// # use bundlebase::{Bundle, BundleBuilder, BundlebaseError, BundleFacade};
    /// # async fn example(c: &BundleBuilder) -> Result<(), BundlebaseError> {
    /// // Open by name
    /// let view = c.view("adults").await?;
    ///
    /// // Or open by ID
    /// let view = c.view("abc123def456").await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn view(&self, identifier: &str) -> Result<Arc<Bundle>, BundlebaseError>;

    /// Exports the bundle's data directory to an uncompressed tar archive.
    ///
    /// Creates a tar file containing all bundle data including:
    /// - `_bundlebase/` directory with all commit manifests
    /// - All data files (parquet, CSV, etc.)
    /// - All index files
    /// - All layout files
    ///
    /// The resulting tar file can be opened as a bundle and supports
    /// further commits via append-only mode since bundlebase never modifies
    /// existing files.
    ///
    /// # Arguments
    /// * `tar_path` - Path where the tar file should be created
    ///
    /// # Returns
    /// Success message with the tar file path
    ///
    /// # Errors
    /// Returns an error if the tar file cannot be created or if there are
    /// uncommitted changes (for BundleBuilder instances).
    ///
    /// # Example
    /// ```ignore
    /// bundle.export_tar("archive.tar").await?;
    /// let archived = Bundle::open("archive.tar", None).await?;
    /// ```
    async fn export_tar(&self, tar_path: &str) -> Result<String, BundlebaseError>;

    /// Returns the query execution plan as a formatted string
    async fn explain(&self) -> Result<String, BundlebaseError>;

    /// Returns uncommitted changes (empty for Bundle, populated for BundleBuilder)
    fn status_changes(&self) -> Vec<BundleChange>;

    /// Returns the current bundle status
    fn status(&self) -> BundleStatus;

    /// Returns index definitions
    fn indexes(&self) -> Vec<Arc<IndexDefinition>>;

    /// Returns packs (id -> pack)
    fn packs(&self) -> HashMap<ObjectId, Arc<Pack>>;

    /// Returns views by name (name -> id mapping)
    fn views_by_name(&self) -> HashMap<String, ObjectId>;

    /// Returns the data directory for this bundle
    fn data_dir(&self) -> Arc<dyn IOReadWriteDir>;

    /// Returns the bundle configuration
    fn config(&self) -> Arc<BundleConfig>;

    /// Returns the DataFusion session context
    fn ctx(&self) -> Arc<SessionContext>;

    /// Execute a read-only command on this bundle.
    ///
    /// This method executes commands that don't require mutation (like EXPLAIN).
    /// For mutating commands, use `BundleBuilder::execute_command()`.
    ///
    /// # Arguments
    /// * `cmd` - The facade command to execute
    ///
    /// # Returns
    /// * `Ok(Box<dyn CommandResponse>)` - Command's output on success
    /// * `Err(BundlebaseError)` - Execution failed
    ///
    /// # Example
    /// ```ignore
    /// use bundlebase::bundle::command::{FacadeCommand, ExplainPlanCommand};
    ///
    /// let cmd = FacadeCommand::ExplainPlan(ExplainPlanCommand::new());
    /// let output = bundle.execute_facade_command(cmd).await?;
    /// ```
    async fn execute_facade_command(
        &self,
        cmd: FacadeCommand,
    ) -> Result<Box<dyn CommandResponse>, BundlebaseError>;

    /// Execute any bundlebase command on this bundle.
    ///
    /// For `BundleBuilder`, this executes the command directly.
    /// For `Bundle` (read-only), this returns an error for mutating commands.
    ///
    /// # Arguments
    /// * `cmd` - The bundlebase command to execute
    ///
    /// # Returns
    /// * `Ok(Box<dyn CommandResponse>)` - Command's output on success
    /// * `Err(BundlebaseError)` - Command not supported or execution failed
    async fn execute_command(
        &self,
        cmd: BundleCommand,
    ) -> Result<Box<dyn CommandResponse>, BundlebaseError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::test_datafile;
    use crate::Bundle;
    use futures::StreamExt;

    // ==================== BundleBuilder Tests ====================

    #[tokio::test]
    async fn test_builder_execute_sql_query() {
        let builder = BundleBuilder::create("memory:///test_execute", None)
            .await
            .unwrap();
        builder
            .attach(test_datafile("userdata.parquet"), None)
            .await
            .unwrap();

        // Execute a SQL query via execute()
        let mut stream = builder
            .as_ref()
            .execute("SELECT id, first_name FROM bundle LIMIT 5", vec![])
            .await
            .unwrap();

        let mut row_count = 0;
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result.unwrap();
            row_count += batch.num_rows();
            // Verify schema has the expected columns
            assert_eq!(batch.num_columns(), 2);
        }
        assert_eq!(row_count, 5);
    }

    #[tokio::test]
    async fn test_builder_execute_explain_command() {
        let builder = BundleBuilder::create("memory:///test_execute_explain", None)
            .await
            .unwrap();
        builder
            .attach(test_datafile("userdata.parquet"), None)
            .await
            .unwrap();

        // Execute EXPLAIN PLAN command via execute()
        let mut stream = builder
            .as_ref()
            .execute("EXPLAIN PLAN", vec![])
            .await
            .unwrap();

        let mut batches = Vec::new();
        while let Some(batch_result) = stream.next().await {
            batches.push(batch_result.unwrap());
        }

        // EXPLAIN should return a single batch with the plan
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].num_columns(), 1); // Single "message" column
        assert!(batches[0].num_rows() > 0);
    }

    #[tokio::test]
    async fn test_builder_execute_mutating_command_succeeds() {
        let builder = BundleBuilder::create("memory:///test_execute_mutate", None)
            .await
            .unwrap();

        // BundleBuilder can execute mutating commands via execute()
        // This will fail because the file doesn't exist, but it should attempt execution
        let result = builder
            .as_ref()
            .execute(&format!("ATTACH '{}'", test_datafile("userdata.parquet")), vec![])
            .await;

        // Should succeed (attach the file)
        assert!(result.is_ok(), "ATTACH should succeed on BundleBuilder: {:?}", result.err());

        // Verify the attachment worked
        let schema = builder.schema().await.unwrap();
        assert!(!schema.fields().is_empty(), "Schema should have fields after attach");
    }

    #[tokio::test]
    async fn test_builder_execute_filter_command_succeeds() {
        let builder = BundleBuilder::create("memory:///test_execute_filter", None)
            .await
            .unwrap();
        builder
            .attach(test_datafile("userdata.parquet"), None)
            .await
            .unwrap();

        let initial_rows = builder.num_rows().await.unwrap();

        // BundleBuilder can execute FILTER via execute()
        let result = builder
            .as_ref()
            .execute("FILTER WITH SELECT * FROM bundle WHERE id > 10", vec![])
            .await;

        assert!(result.is_ok(), "FILTER should succeed on BundleBuilder: {:?}", result.err());

        // Verify the filter was applied (fewer rows)
        let filtered_rows = builder.num_rows().await.unwrap();
        assert!(filtered_rows < initial_rows, "FILTER should reduce row count");
    }

    // ==================== Bundle (Read-Only) Tests ====================

    #[tokio::test]
    async fn test_bundle_execute_sql_query() {
        // Create and commit a bundle first
        let builder = BundleBuilder::create("memory:///test_bundle_query", None)
            .await
            .unwrap();
        builder
            .attach(test_datafile("userdata.parquet"), None)
            .await
            .unwrap();
        builder.commit("Test commit").await.unwrap();
        let bundle_url = builder.url().to_string();

        // Open the committed bundle (read-only)
        let bundle = Bundle::open(&bundle_url, None).await.unwrap();

        // Execute a SQL query via execute()
        let mut stream = bundle
            .as_ref()
            .execute("SELECT COUNT(*) as cnt FROM bundle", vec![])
            .await
            .unwrap();

        let mut batches = Vec::new();
        while let Some(batch_result) = stream.next().await {
            batches.push(batch_result.unwrap());
        }

        assert!(!batches.is_empty());
        // COUNT(*) should return one row
        let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total_rows, 1);
    }

    #[tokio::test]
    async fn test_bundle_execute_explain_command() {
        // Create and commit a bundle first
        let builder = BundleBuilder::create("memory:///test_bundle_explain", None)
            .await
            .unwrap();
        builder
            .attach(test_datafile("userdata.parquet"), None)
            .await
            .unwrap();
        builder.commit("Test commit").await.unwrap();
        let bundle_url = builder.url().to_string();

        // Open the committed bundle (read-only)
        let bundle = Bundle::open(&bundle_url, None).await.unwrap();

        // Execute EXPLAIN PLAN command via execute()
        let mut stream = bundle
            .as_ref()
            .execute("EXPLAIN PLAN", vec![])
            .await
            .unwrap();

        let mut batches = Vec::new();
        while let Some(batch_result) = stream.next().await {
            batches.push(batch_result.unwrap());
        }

        // EXPLAIN should return a single batch with the plan
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].num_columns(), 1);
        assert!(batches[0].num_rows() > 0);
    }

    #[tokio::test]
    async fn test_bundle_execute_mutating_command_fails() {
        // Create and commit a bundle first
        let builder = BundleBuilder::create("memory:///test_bundle_mutate", None)
            .await
            .unwrap();
        builder
            .attach(test_datafile("userdata.parquet"), None)
            .await
            .unwrap();
        builder.commit("Test commit").await.unwrap();
        let bundle_url = builder.url().to_string();

        // Open the committed bundle (read-only)
        let bundle = Bundle::open(&bundle_url, None).await.unwrap();

        // Attempting to execute a mutating command should fail
        let result = bundle
            .as_ref()
            .execute("ATTACH 'another_file.parquet'", vec![])
            .await;

        match result {
            Ok(_) => panic!("Expected error for mutating command on read-only bundle"),
            Err(e) => {
                let err_msg = e.to_string();
                assert!(
                    err_msg.contains("Cannot execute 'ATTACH' on read-only bundle"),
                    "Expected error about mutating command, got: {}",
                    err_msg
                );
            }
        }
    }

    #[tokio::test]
    async fn test_bundle_execute_commit_command_fails() {
        // Create and commit a bundle first
        let builder = BundleBuilder::create("memory:///test_bundle_commit", None)
            .await
            .unwrap();
        builder
            .attach(test_datafile("userdata.parquet"), None)
            .await
            .unwrap();
        builder.commit("Test commit").await.unwrap();
        let bundle_url = builder.url().to_string();

        // Open the committed bundle (read-only)
        let bundle = Bundle::open(&bundle_url, None).await.unwrap();

        // COMMIT is a mutating command, should fail
        let result = bundle
            .as_ref()
            .execute("COMMIT 'Another commit'", vec![])
            .await;

        match result {
            Ok(_) => panic!("Expected error for COMMIT command on read-only bundle"),
            Err(e) => {
                let err_msg = e.to_string();
                assert!(
                    err_msg.contains("Cannot execute 'COMMIT' on read-only bundle"),
                    "Expected error about COMMIT command, got: {}",
                    err_msg
                );
            }
        }
    }

    // ==================== Edge Cases ====================

    #[tokio::test]
    async fn test_execute_with_params() {
        let builder = BundleBuilder::create("memory:///test_execute_params", None)
            .await
            .unwrap();
        builder
            .attach(test_datafile("userdata.parquet"), None)
            .await
            .unwrap();

        // Execute a parameterized query
        let mut stream = builder
            .as_ref()
            .execute(
                "SELECT * FROM bundle WHERE id = $1",
                vec![ScalarValue::Int64(Some(1))],
            )
            .await
            .unwrap();

        let mut row_count = 0;
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result.unwrap();
            row_count += batch.num_rows();
        }
        // Should find exactly one row with id=1
        assert_eq!(row_count, 1);
    }

    #[tokio::test]
    async fn test_execute_empty_bundle() {
        let builder = BundleBuilder::create("memory:///test_execute_empty", None)
            .await
            .unwrap();

        // Execute on empty bundle should work (returns empty result)
        let mut stream = builder
            .as_ref()
            .execute("SELECT * FROM bundle", vec![])
            .await
            .unwrap();

        let mut row_count = 0;
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result.unwrap();
            row_count += batch.num_rows();
        }
        assert_eq!(row_count, 0);
    }
}
