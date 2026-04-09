use crate::parser::{extract_identifier, quote_identifier};
use crate::response::OutputShape;
use crate::{BundleFacadeCommand, CommandParsing, Rule};
use arrow::array::{BinaryArray, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use bundlebase::bundle::BundleFacade;
use bundlebase::Bundle;
use bundlebase_common::BundlebaseError;
use bundlebase_report::BundleResolver;
use datafusion::catalog::TableProvider;
use datafusion::datasource::MemTable;
use datafusion::execution::SendableRecordBatchStream;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Command to generate a PDF from a stored report template.
#[derive(Debug, Clone)]
pub struct GenerateReportCommand {
    pub id: String,
}

impl GenerateReportCommand {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }

    pub fn output_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("name", DataType::Utf8, false),
            Field::new("description", DataType::Utf8, false),
            Field::new("pdf", DataType::Binary, false),
        ]))
    }

    pub fn output_shape() -> OutputShape {
        OutputShape::Table
    }
}

impl CommandParsing for GenerateReportCommand {
    fn rule() -> Rule {
        Rule::generate_report_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut id = None;

        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::report_id {
                id = Some(extract_identifier(&inner));
            }
        }

        let id =
            id.ok_or_else(|| -> BundlebaseError { "GENERATE REPORT missing id".into() })?;
        Ok(GenerateReportCommand::new(id))
    }

    fn to_statement(&self) -> String {
        format!("GENERATE REPORT {}", quote_identifier(&self.id))
    }
}

/// Bundle resolver that wraps an existing facade for "." and "bundle" references,
/// and opens other bundles by path.
struct FacadeBundleResolver {
    /// The current bundle, wrapped in Arc for the BundleResolver trait.
    /// This preserves uncommitted state (reports created before COMMIT).
    facade: Arc<dyn BundleFacade>,
    cache: Mutex<HashMap<String, Arc<dyn BundleFacade>>>,
}

impl FacadeBundleResolver {
    fn new(facade: Arc<dyn BundleFacade>) -> Self {
        Self {
            facade,
            cache: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait::async_trait]
impl BundleResolver for FacadeBundleResolver {
    async fn resolve(
        &self,
        bundle_ref: &str,
    ) -> Result<Arc<dyn BundleFacade>, BundlebaseError> {
        if bundle_ref == "." || bundle_ref == "bundle" {
            return Ok(self.facade.clone());
        }

        // Check cache
        {
            let cache = self.cache.lock().await;
            if let Some(bundle) = cache.get(bundle_ref) {
                return Ok(bundle.clone());
            }
        }

        // Open bundle by path
        let bundle = Bundle::open(bundle_ref, None).await.map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to open bundle '{}': {}",
                bundle_ref, e
            ))
        })?;

        let arc_bundle: Arc<dyn BundleFacade> = bundle;
        self.cache
            .lock()
            .await
            .insert(bundle_ref.to_string(), arc_bundle.clone());

        Ok(arc_bundle)
    }
}

impl BundleFacadeCommand for GenerateReportCommand {
    type Output = SendableRecordBatchStream;

    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<SendableRecordBatchStream, BundlebaseError> {
        // Look up report
        let reports = facade.reports();
        let report = reports.get(&self.id).ok_or_else(|| {
            let available: Vec<&String> = reports.keys().collect();
            let list = if available.is_empty() {
                "none".to_string()
            } else {
                available.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
            };
            BundlebaseError::from(format!(
                "Report '{}' not found. Available reports: {}",
                self.id, list
            ))
        })?;

        // Use the facade's URL to open a read-only copy for report generation.
        // We need an Arc<dyn BundleFacade> for the resolver, but only have &dyn.
        // Opening by URL gives us a committed-state snapshot the resolver can own.
        let bundle_url = facade.url().to_string();
        let bundle = Bundle::open(&bundle_url, None).await.map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to open bundle for report generation: {}",
                e
            ))
        })?;
        let resolver = FacadeBundleResolver::new(bundle);

        // Generate PDF bytes
        let pdf_bytes = bundlebase_report::generate_report_bytes(
            &report.content,
            &resolver,
            true,
        )
        .await?;

        // Build result RecordBatch
        let schema = Self::output_schema();
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![report.id.as_str()])),
                Arc::new(StringArray::from(vec![report.name.as_str()])),
                Arc::new(StringArray::from(vec![report.description.as_str()])),
                Arc::new(BinaryArray::from_vec(vec![&pdf_bytes])),
            ],
        )
        .map_err(|e| BundlebaseError::from(format!("Failed to build result batch: {}", e)))?;

        // Return as stream
        let mem_table = MemTable::try_new(schema, vec![vec![batch]])
            .map_err(|e| BundlebaseError::from(format!("Failed to create result table: {}", e)))?;

        let ctx = datafusion::prelude::SessionContext::new();
        let state = ctx.state();
        let plan = mem_table
            .scan(&state, None, &[], None)
            .await
            .map_err(|e| BundlebaseError::from(format!("Failed to scan result: {}", e)))?;

        let stream = datafusion::physical_plan::execute_stream(plan, state.task_ctx())
            .map_err(|e| BundlebaseError::from(format!("Failed to execute result stream: {}", e)))?;

        Ok(stream)
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_generate_report() {
        let input = "GENERATE REPORT monthly-sales";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::GenerateReport(c) => {
                assert_eq!(c.id, "monthly-sales");
            }
            _ => panic!("Expected GenerateReport variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = GenerateReportCommand::new("test-report");
        let statement = cmd.to_statement();
        assert_eq!(statement, "GENERATE REPORT \"test-report\"");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::GenerateReport(c) => {
                assert_eq!(c.id, "test-report");
            }
            _ => panic!("Expected GenerateReport variant"),
        }
    }
}
