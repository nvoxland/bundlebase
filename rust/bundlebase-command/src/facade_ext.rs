//! Extension trait for command execution on BundleFacade.
//!
//! This module provides the `BundleFacadeCommandExt` trait that adds command-related
//! methods to any `BundleFacade` implementor. These methods were previously part of
//! the `BundleFacade` trait itself but are separated to keep the command system
//! in its own crate.

use bundlebase::bundle::Bundle;
use bundlebase::{BundleFacade, BundleBuilder};
use bundlebase_common::BundlebaseError;
use bundlebase_common::Platform;
use datafusion::common::ScalarValue;
use datafusion::execution::SendableRecordBatchStream;
use crate::response::{CommandResponse, OutputShape};
use crate::{BundleCommand, FacadeCommand};
use crate::parser::{is_command_statement, parse_command};
use crate::{DescribeDataCommand, ExplainPlanCommand, ImportTempConnectorCommand, ImportTempFunctionCommand};
use crate::facade::describe_data::DescribeDataColumnSpec;
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use std::sync::Arc;

/// Extension trait that adds command execution capabilities to `BundleFacade`.
///
/// Import this trait to call `execute()`, `response_schema()`, `explain()`,
/// `execute_facade_command()`, `execute_command()`, `import_temp_connector()`,
/// and `import_temp_function()` on any `BundleFacade` implementor.
///
/// # Example
/// ```ignore
/// use bundlebase_command::BundleFacadeCommandExt;
///
/// let stream = bundle.execute("SELECT * FROM bundle", vec![]).await?;
/// ```
#[async_trait]
pub trait BundleFacadeCommandExt {
    /// Execute a SQL statement or command, returning streaming results.
    ///
    /// This unified method handles both regular SQL queries and bundlebase commands
    /// (like ATTACH, FILTER, EXPLAIN, etc.), always returning a `SendableRecordBatchStream`.
    async fn execute(
        &self,
        sql: &str,
        params: Vec<ScalarValue>,
    ) -> Result<SendableRecordBatchStream, BundlebaseError>;

    /// Get the schema and output shape that will be returned by executing a SQL statement.
    ///
    /// This method determines the output schema and display shape without executing the statement.
    async fn response_schema(&self, sql: &str) -> Result<(SchemaRef, OutputShape), BundlebaseError>;

    /// Returns the query execution plan as a stream.
    async fn explain(
        &self,
        verbose: bool,
        analyze: bool,
        format: datafusion::logical_expr::ExplainFormat,
        sql: Option<&str>,
    ) -> Result<SendableRecordBatchStream, BundlebaseError>;

    /// Load a temporary connector at runtime only (not persisted).
    async fn import_temp_connector(
        &self,
        name: &str,
        from: &str,
        platform: &str,
    ) -> Result<(), BundlebaseError>;

    /// Load a temporary function at runtime only (not persisted).
    async fn import_temp_function(
        &self,
        name: &str,
        from: &str,
        platform: &str,
    ) -> Result<(), BundlebaseError>;

    /// Analyze data quality and statistics for specified columns.
    ///
    /// Returns per-column stats: min, max, avg, null counts, top values, and
    /// invalid values (when expected types are specified).
    async fn describe_data(
        &self,
        columns: Vec<(String, Option<String>)>,
    ) -> Result<SendableRecordBatchStream, BundlebaseError>;

    /// Execute a read-only command on this bundle.
    async fn execute_facade_command(
        &self,
        cmd: FacadeCommand,
    ) -> Result<Box<dyn CommandResponse>, BundlebaseError>;

    /// Execute any bundlebase command on this bundle.
    ///
    /// For `BundleBuilder`, this executes the command directly.
    /// For `Bundle` (read-only), this returns an error for mutating commands.
    async fn execute_command(
        &self,
        cmd: BundleCommand,
    ) -> Result<Box<dyn CommandResponse>, BundlebaseError>;
}

// Helper: shared default implementations
async fn default_execute(
    facade: &dyn BundleFacade,
    ext: &(dyn BundleFacadeCommandExt + Send + Sync),
    sql: &str,
    params: Vec<ScalarValue>,
) -> Result<SendableRecordBatchStream, BundlebaseError> {
    if is_command_statement(sql) {
        let cmd = parse_command(sql)?;
        let output = ext.execute_command(cmd).await?;
        output.into_stream()
    } else {
        facade.query(sql, params, None).await
    }
}

async fn default_response_schema(
    facade: &dyn BundleFacade,
    sql: &str,
) -> Result<(SchemaRef, OutputShape), BundlebaseError> {
    let sql = sql.trim();
    if is_command_statement(sql) {
        let cmd = parse_command(sql)?;
        Ok((cmd.output_schema(), cmd.output_shape()))
    } else {
        let stream = facade.query(sql, vec![], None).await?;
        Ok((stream.schema().clone(), OutputShape::Table))
    }
}

async fn default_explain(
    ext: &(dyn BundleFacadeCommandExt + Send + Sync),
    verbose: bool,
    analyze: bool,
    format: datafusion::logical_expr::ExplainFormat,
    sql: Option<&str>,
) -> Result<SendableRecordBatchStream, BundlebaseError> {
    let format_str = match format {
        datafusion::logical_expr::ExplainFormat::Tree => Some("TREE".to_string()),
        datafusion::logical_expr::ExplainFormat::Graphviz => Some("GRAPHVIZ".to_string()),
        _ => None,
    };
    let cmd = ExplainPlanCommand {
        verbose,
        analyze,
        format: format_str,
        sql: sql.map(|s| s.to_string()),
    };
    let response = ext.execute_facade_command(FacadeCommand::ExplainPlan(cmd)).await?;
    response.into_stream()
}

async fn default_describe_data(
    ext: &(dyn BundleFacadeCommandExt + Send + Sync),
    columns: Vec<(String, Option<String>)>,
) -> Result<SendableRecordBatchStream, BundlebaseError> {
    let specs: Vec<DescribeDataColumnSpec> = columns
        .into_iter()
        .map(|(name, expected_type)| DescribeDataColumnSpec {
            name,
            expected_type,
        })
        .collect();
    let cmd = DescribeDataCommand { columns: specs };
    let response = ext
        .execute_facade_command(FacadeCommand::DescribeData(cmd))
        .await?;
    response.into_stream()
}

async fn default_import_temp_connector(
    ext: &(dyn BundleFacadeCommandExt + Send + Sync),
    name: &str,
    from: &str,
    platform: &str,
) -> Result<(), BundlebaseError> {
    let platform: Platform = platform.parse()?;
    let cmd = ImportTempConnectorCommand::new(name, from, platform);
    ext.execute_facade_command(FacadeCommand::ImportTempConnector(cmd)).await?;
    Ok(())
}

async fn default_import_temp_function(
    ext: &(dyn BundleFacadeCommandExt + Send + Sync),
    name: &str,
    from: &str,
    platform: &str,
) -> Result<(), BundlebaseError> {
    let platform: Platform = platform.parse()?;
    let cmd = ImportTempFunctionCommand::new(name, from, platform);
    ext.execute_facade_command(FacadeCommand::ImportTempFunction(cmd)).await?;
    Ok(())
}

#[async_trait]
impl BundleFacadeCommandExt for Bundle {
    async fn execute(
        &self,
        sql: &str,
        params: Vec<ScalarValue>,
    ) -> Result<SendableRecordBatchStream, BundlebaseError> {
        default_execute(self, self, sql, params).await
    }

    async fn response_schema(&self, sql: &str) -> Result<(SchemaRef, OutputShape), BundlebaseError> {
        default_response_schema(self, sql).await
    }

    async fn explain(
        &self,
        verbose: bool,
        analyze: bool,
        format: datafusion::logical_expr::ExplainFormat,
        sql: Option<&str>,
    ) -> Result<SendableRecordBatchStream, BundlebaseError> {
        default_explain(self, verbose, analyze, format, sql).await
    }

    async fn import_temp_connector(
        &self,
        name: &str,
        from: &str,
        platform: &str,
    ) -> Result<(), BundlebaseError> {
        default_import_temp_connector(self, name, from, platform).await
    }

    async fn import_temp_function(
        &self,
        name: &str,
        from: &str,
        platform: &str,
    ) -> Result<(), BundlebaseError> {
        default_import_temp_function(self, name, from, platform).await
    }

    async fn describe_data(
        &self,
        columns: Vec<(String, Option<String>)>,
    ) -> Result<SendableRecordBatchStream, BundlebaseError> {
        default_describe_data(self, columns).await
    }

    async fn execute_facade_command(
        &self,
        cmd: FacadeCommand,
    ) -> Result<Box<dyn CommandResponse>, BundlebaseError> {
        cmd.execute(self).await
    }

    async fn execute_command(
        &self,
        cmd: BundleCommand,
    ) -> Result<Box<dyn CommandResponse>, BundlebaseError> {
        // Bundle is read-only, so only facade commands are allowed.
        let facade_cmd = cmd.into_facade_command()?;
        self.execute_facade_command(facade_cmd).await
    }
}

#[async_trait]
impl BundleFacadeCommandExt for BundleBuilder {
    async fn execute(
        &self,
        sql: &str,
        params: Vec<ScalarValue>,
    ) -> Result<SendableRecordBatchStream, BundlebaseError> {
        default_execute(self, self, sql, params).await
    }

    async fn response_schema(&self, sql: &str) -> Result<(SchemaRef, OutputShape), BundlebaseError> {
        default_response_schema(self, sql).await
    }

    async fn explain(
        &self,
        verbose: bool,
        analyze: bool,
        format: datafusion::logical_expr::ExplainFormat,
        sql: Option<&str>,
    ) -> Result<SendableRecordBatchStream, BundlebaseError> {
        default_explain(self, verbose, analyze, format, sql).await
    }

    async fn import_temp_connector(
        &self,
        name: &str,
        from: &str,
        platform: &str,
    ) -> Result<(), BundlebaseError> {
        default_import_temp_connector(self, name, from, platform).await
    }

    async fn import_temp_function(
        &self,
        name: &str,
        from: &str,
        platform: &str,
    ) -> Result<(), BundlebaseError> {
        default_import_temp_function(self, name, from, platform).await
    }

    async fn describe_data(
        &self,
        columns: Vec<(String, Option<String>)>,
    ) -> Result<SendableRecordBatchStream, BundlebaseError> {
        default_describe_data(self, columns).await
    }

    async fn execute_facade_command(
        &self,
        cmd: FacadeCommand,
    ) -> Result<Box<dyn CommandResponse>, BundlebaseError> {
        cmd.execute(self).await
    }

    async fn execute_command(
        &self,
        cmd: BundleCommand,
    ) -> Result<Box<dyn CommandResponse>, BundlebaseError> {
        // BundleBuilder can execute all commands
        cmd.execute(self).await
    }
}

/// Implementation for `Arc<dyn BundleFacade>` which dispatches dynamically.
///
/// Uses `as_any()` to downcast to `BundleBuilder` for mutating commands.
/// Falls back to facade-only (read-only) execution if not a BundleBuilder.
#[async_trait]
impl BundleFacadeCommandExt for Arc<dyn BundleFacade> {
    async fn execute(
        &self,
        sql: &str,
        params: Vec<ScalarValue>,
    ) -> Result<SendableRecordBatchStream, BundlebaseError> {
        if is_command_statement(sql) {
            let cmd = parse_command(sql)?;
            let output = self.execute_command(cmd).await?;
            output.into_stream()
        } else {
            self.as_ref().query(sql, params, None).await
        }
    }

    async fn response_schema(&self, sql: &str) -> Result<(SchemaRef, OutputShape), BundlebaseError> {
        default_response_schema(self.as_ref(), sql).await
    }

    async fn explain(
        &self,
        verbose: bool,
        analyze: bool,
        format: datafusion::logical_expr::ExplainFormat,
        sql: Option<&str>,
    ) -> Result<SendableRecordBatchStream, BundlebaseError> {
        default_explain(self, verbose, analyze, format, sql).await
    }

    async fn import_temp_connector(
        &self,
        name: &str,
        from: &str,
        platform: &str,
    ) -> Result<(), BundlebaseError> {
        default_import_temp_connector(self, name, from, platform).await
    }

    async fn import_temp_function(
        &self,
        name: &str,
        from: &str,
        platform: &str,
    ) -> Result<(), BundlebaseError> {
        default_import_temp_function(self, name, from, platform).await
    }

    async fn describe_data(
        &self,
        columns: Vec<(String, Option<String>)>,
    ) -> Result<SendableRecordBatchStream, BundlebaseError> {
        default_describe_data(self, columns).await
    }

    async fn execute_facade_command(
        &self,
        cmd: FacadeCommand,
    ) -> Result<Box<dyn CommandResponse>, BundlebaseError> {
        cmd.execute(self.as_ref()).await
    }

    async fn execute_command(
        &self,
        cmd: BundleCommand,
    ) -> Result<Box<dyn CommandResponse>, BundlebaseError> {
        // Try to downcast to BundleBuilder for full command support
        if let Some(builder) = self.as_any().downcast_ref::<BundleBuilder>() {
            cmd.execute(builder).await
        } else {
            // Read-only: only facade commands
            let facade_cmd = cmd.into_facade_command()?;
            self.execute_facade_command(facade_cmd).await
        }
    }
}

/// Implementation for `Arc<Bundle>` - delegates to inner Bundle.
#[async_trait]
impl BundleFacadeCommandExt for Arc<Bundle> {
    async fn execute(&self, sql: &str, params: Vec<ScalarValue>) -> Result<SendableRecordBatchStream, BundlebaseError> { (**self).execute(sql, params).await }
    async fn response_schema(&self, sql: &str) -> Result<(SchemaRef, OutputShape), BundlebaseError> { (**self).response_schema(sql).await }
    async fn explain(&self, verbose: bool, analyze: bool, format: datafusion::logical_expr::ExplainFormat, sql: Option<&str>) -> Result<SendableRecordBatchStream, BundlebaseError> { (**self).explain(verbose, analyze, format, sql).await }
    async fn import_temp_connector(&self, name: &str, from: &str, platform: &str) -> Result<(), BundlebaseError> { (**self).import_temp_connector(name, from, platform).await }
    async fn import_temp_function(&self, name: &str, from: &str, platform: &str) -> Result<(), BundlebaseError> { (**self).import_temp_function(name, from, platform).await }
    async fn describe_data(&self, columns: Vec<(String, Option<String>)>) -> Result<SendableRecordBatchStream, BundlebaseError> { (**self).describe_data(columns).await }
    async fn execute_facade_command(&self, cmd: FacadeCommand) -> Result<Box<dyn CommandResponse>, BundlebaseError> { (**self).execute_facade_command(cmd).await }
    async fn execute_command(&self, cmd: BundleCommand) -> Result<Box<dyn CommandResponse>, BundlebaseError> {
        let facade_cmd = cmd.into_facade_command()?;
        self.execute_facade_command(facade_cmd).await
    }
}

/// Implementation for `Arc<BundleBuilder>` - delegates to inner BundleBuilder.
#[async_trait]
impl BundleFacadeCommandExt for Arc<BundleBuilder> {
    async fn execute(&self, sql: &str, params: Vec<ScalarValue>) -> Result<SendableRecordBatchStream, BundlebaseError> { (**self).execute(sql, params).await }
    async fn response_schema(&self, sql: &str) -> Result<(SchemaRef, OutputShape), BundlebaseError> { (**self).response_schema(sql).await }
    async fn explain(&self, verbose: bool, analyze: bool, format: datafusion::logical_expr::ExplainFormat, sql: Option<&str>) -> Result<SendableRecordBatchStream, BundlebaseError> { (**self).explain(verbose, analyze, format, sql).await }
    async fn import_temp_connector(&self, name: &str, from: &str, platform: &str) -> Result<(), BundlebaseError> { (**self).import_temp_connector(name, from, platform).await }
    async fn import_temp_function(&self, name: &str, from: &str, platform: &str) -> Result<(), BundlebaseError> { (**self).import_temp_function(name, from, platform).await }
    async fn describe_data(&self, columns: Vec<(String, Option<String>)>) -> Result<SendableRecordBatchStream, BundlebaseError> { (**self).describe_data(columns).await }
    async fn execute_facade_command(&self, cmd: FacadeCommand) -> Result<Box<dyn CommandResponse>, BundlebaseError> { (**self).execute_facade_command(cmd).await }
    async fn execute_command(&self, cmd: BundleCommand) -> Result<Box<dyn CommandResponse>, BundlebaseError> {
        cmd.execute(&**self).await
    }
}
