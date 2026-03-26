//! PDF report generation for Bundlebase.
//!
//! Takes markdown with embedded `bundlebase` YAML
//! fenced blocks, executes queries against bundles, renders charts and tables
//! via Typst with cetz-plot, and produces styled PDF output.

pub mod defaults;
pub mod parse;
pub mod query;
pub mod template;
pub mod typst_chart;
pub mod typst_render;
pub mod typst_table;

use bundlebase::BundleFacade;
use bundlebase_common::BundlebaseError;
use parse::ReportElement;
use std::sync::Arc;
use typst_render::ResolvedElement;

/// Maximum number of rows returned for table and chart blocks.
pub const MAX_TABLE_ROWS: usize = 20;

/// Resolves bundle references to open bundle facades.
///
/// MCP implementations look up by key in the open bundles map.
/// CLI implementations open bundles by path/URL, caching across blocks.
#[async_trait::async_trait]
pub trait BundleResolver: Send + Sync {
    async fn resolve(&self, bundle_ref: &str) -> Result<Arc<dyn BundleFacade>, BundlebaseError>;
}

/// Generate a PDF report from markdown with embedded query/chart placeholders.
///
/// Parses the markdown, executes queries against resolved bundles,
/// renders charts and tables via Typst, and writes the PDF to `output_path`.
///
/// Returns a success message describing what was generated.
pub async fn generate_report(
    input: &str,
    resolver: &dyn BundleResolver,
    output: &str,
    show_branding: bool,
) -> Result<String, BundlebaseError> {
    // Validate output path
    if !output.ends_with(".pdf") {
        return Err(BundlebaseError::from(format!(
            "Output path must end with .pdf, got: {}",
            output
        )));
    }

    // Phase 1: Parse markdown
    let elements = parse::parse_report(input)?;

    // Phase 2: Resolve elements — execute queries and generate Typst markup
    let mut resolved = Vec::new();
    let mut chart_count = 0usize;
    let mut table_count = 0usize;

    for (idx, element) in elements.iter().enumerate() {
        match element {
            ReportElement::Text(text) => {
                resolved.push(ResolvedElement::Text(text.clone()));
            }
            ReportElement::Chart(chart) => {
                let bundle = resolver.resolve(&chart.bundle).await.map_err(|e| {
                    BundlebaseError::from(format!(
                        "Failed to resolve bundle '{}' for chart block #{}: {}",
                        chart.bundle,
                        idx + 1,
                        e
                    ))
                })?;
                let data = query::execute_bounded_query(&bundle, &chart.query)
                    .await
                    .map_err(|e| {
                        BundlebaseError::from(format!(
                            "Query failed for chart block #{} (bundle '{}', type {}): {}",
                            idx + 1,
                            chart.bundle,
                            chart.chart_type,
                            e
                        ))
                    })?;
                let markup = typst_chart::render_chart(chart, &data)?;
                resolved.push(ResolvedElement::Chart(markup));
                chart_count += 1;
            }
            ReportElement::Table(table) => {
                let bundle = resolver.resolve(&table.bundle).await.map_err(|e| {
                    BundlebaseError::from(format!(
                        "Failed to resolve bundle '{}' for table block #{}: {}",
                        table.bundle,
                        idx + 1,
                        e
                    ))
                })?;
                let data = query::execute_bounded_query(&bundle, &table.query)
                    .await
                    .map_err(|e| {
                        BundlebaseError::from(format!(
                            "Query failed for table block #{} (bundle '{}'): {}",
                            idx + 1,
                            table.bundle,
                            e
                        ))
                    })?;
                let markup = typst_table::render_table(table, &data)?;
                resolved.push(ResolvedElement::Table(markup));
                table_count += 1;
            }
        }
    }

    // Phase 3: Assemble Typst document and compile to PDF
    let typst_source = typst_render::assemble_document(&resolved, show_branding);
    let pdf_bytes = typst_render::compile_to_pdf(&typst_source)?;

    // Phase 4: Write PDF to disk
    tokio::fs::write(output, &pdf_bytes).await.map_err(|e| {
        BundlebaseError::from(format!("Failed to write PDF to '{}': {}", output, e))
    })?;

    Ok(format!(
        "Report generated: {} ({} chart{}, {} table{})",
        output,
        chart_count,
        if chart_count == 1 { "" } else { "s" },
        table_count,
        if table_count == 1 { "" } else { "s" },
    ))
}
