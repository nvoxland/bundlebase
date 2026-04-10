use crate::bundle::operation::Operation;
use crate::bundle::ReportEntry;
use crate::io::IOReadDir;
use crate::{Bundle, BundleBuilder, BundlebaseError};
use bytes::Bytes;
use datafusion::common::DataFusionError;
use futures::stream;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CreateReportOp {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Relative path to the content file within the bundle's data directory.
    pub path: String,
}

impl CreateReportOp {
    /// Write the markdown content to the bundle's data directory and create the operation.
    pub async fn setup(
        id: String,
        name: String,
        description: String,
        content: String,
        builder: &BundleBuilder,
    ) -> Result<Self, BundlebaseError> {
        let data_dir = builder.bundle().data_dir();
        let byte_stream = stream::once(async move {
            Ok::<Bytes, std::io::Error>(Bytes::from(content.into_bytes()))
        });
        let address = bundlebase_common::ContentAddress::new(
            bundlebase_common::ContentCategory::Report,
            bundlebase_common::ContentFormat::Md,
        );
        let result = data_dir
            .write_stream(Box::pin(byte_stream), &address)
            .await?;
        let path = data_dir.relative_path(result.file.as_ref())?;

        Ok(Self {
            id,
            name,
            description,
            path,
        })
    }

    /// Read the markdown content from the bundle's data directory.
    async fn read_content(&self, data_dir: &Arc<dyn crate::io::IOReadWriteDir>) -> Result<String, BundlebaseError> {
        let file = data_dir.file(&self.path)?;
        file.read_str()
            .await?
            .ok_or_else(|| BundlebaseError::from(format!(
                "Report content file not found: {}",
                self.path
            )))
    }
}

impl Operation for CreateReportOp {
    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Auto-replace semantics: no check needed
        Ok(())
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        let data_dir = bundle.data_dir();
        let content = self.read_content(&data_dir).await.map_err(|e| {
            DataFusionError::Internal(format!("Failed to read report content: {}", e))
        })?;

        let entry = ReportEntry {
            id: self.id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            content,
        };
        bundle.reports.write().insert(self.id.clone(), entry);
        Ok(())
    }

    fn describe(&self) -> String {
        format!("CREATE REPORT: {} ({})", self.name, self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let op = CreateReportOp {
            id: "monthly-sales".into(),
            name: "Monthly Sales".into(),
            description: "desc".into(),
            path: "ab/cdef01234567.report.md".into(),
        };
        assert_eq!(op.describe(), "CREATE REPORT: Monthly Sales (monthly-sales)");
    }

    #[test]
    fn test_serialization_round_trip() {
        let op = CreateReportOp {
            id: "my-report".into(),
            name: "My Report".into(),
            description: "Description".into(),
            path: "ab/cdef01234567.report.md".into(),
        };
        let yaml = serde_yaml_ng::to_string(&op).expect("serialize");
        let deserialized: CreateReportOp = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(op, deserialized);
    }
}
