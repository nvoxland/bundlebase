//! Typst compilation: assembles report elements into a Typst document and compiles to PDF.

use bundlebase_common::BundlebaseError;
use typst::layout::PagedDocument;
use typst_as_lib::TypstEngine;

/// A resolved report element ready for Typst rendering.
pub enum ResolvedElement {
    /// Markdown text converted to Typst markup.
    Text(String),
    /// Chart Typst markup (cetz-plot calls).
    Chart(String),
    /// Table Typst markup.
    Table(String),
}

/// Assemble resolved elements into a complete Typst document source.
pub fn assemble_document(elements: &[ResolvedElement], show_branding: bool) -> String {
    let mut doc = crate::template::template_preamble(show_branding);

    for element in elements {
        match element {
            ResolvedElement::Text(md) => {
                doc.push_str(&crate::template::markdown_to_typst(md));
                doc.push('\n');
            }
            ResolvedElement::Chart(markup) => {
                doc.push_str(markup);
                doc.push('\n');
            }
            ResolvedElement::Table(markup) => {
                doc.push_str(markup);
                doc.push('\n');
            }
        }
    }

    doc
}

/// Compile a Typst document source to PDF bytes.
pub fn compile_to_pdf(typst_source: &str) -> Result<Vec<u8>, BundlebaseError> {
    let engine = TypstEngine::builder()
        .main_file(typst_source)
        .search_fonts_with(typst_as_lib::typst_kit_options::TypstKitFontOptions::default())
        .with_package_file_resolver()
        .build();

    let compiled = engine.compile::<PagedDocument>();

    // Log warnings
    for warning in &compiled.warnings {
        tracing::warn!("Typst warning: {:?}", warning.message);
    }

    let document = compiled.output.map_err(|e| {
        BundlebaseError::from(format!("Typst compilation failed: {:?}", e))
    })?;

    let options = typst_pdf::PdfOptions::default();
    let pdf_bytes = typst_pdf::pdf(&document, &options).map_err(|e| {
        BundlebaseError::from(format!("PDF generation failed: {:?}", e))
    })?;

    Ok(pdf_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_assemble_text_only() {
        let elements = vec![ResolvedElement::Text("# Hello\n\nWorld.".to_string())];
        let doc = assemble_document(&elements, false);
        assert!(doc.contains("= Hello"));
        assert!(doc.contains("World."));
        assert!(!doc.contains("Created by Bundlebase"));
    }

    #[test]
    fn test_assemble_with_branding() {
        let elements = vec![ResolvedElement::Text("Content".to_string())];
        let doc = assemble_document(&elements, true);
        assert!(doc.contains("Created by Bundlebase"));
    }

    #[test]
    fn test_assemble_mixed_elements() {
        let elements = vec![
            ResolvedElement::Text("# Report".to_string()),
            ResolvedElement::Chart("cetz.canvas({ ... })".to_string()),
            ResolvedElement::Text("After chart.".to_string()),
            ResolvedElement::Table("#table(...)".to_string()),
        ];
        let doc = assemble_document(&elements, true);
        assert!(doc.contains("= Report"));
        assert!(doc.contains("cetz.canvas"));
        assert!(doc.contains("#table"));
    }
}
