use crate::bundle::operation::Operation;
use crate::bundle::BundleFacade;
use crate::{Bundle, BundleBuilder, BundlebaseError};
use datafusion::common::DataFusionError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DropReportOp {
    pub id: String,
}

impl DropReportOp {
    pub fn setup(report_id: &str, builder: &BundleBuilder) -> Result<Self, BundlebaseError> {
        let reports = builder.reports();
        if !reports.contains_key(report_id) {
            let available: Vec<&String> = reports.keys().collect();
            let available_list = if available.is_empty() {
                "none".to_string()
            } else {
                available
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            return Err(BundlebaseError::from(format!(
                "Report '{}' not found. Available reports: {}",
                report_id, available_list
            )));
        }
        Ok(Self {
            id: report_id.to_string(),
        })
    }
}

impl Operation for DropReportOp {
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        if !bundle.reports.read().contains_key(&self.id) {
            return Err(format!("Report '{}' not found", self.id).into());
        }
        Ok(())
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        bundle.reports.write().remove(&self.id);
        Ok(())
    }

    fn describe(&self) -> String {
        format!("DROP REPORT: {}", self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let op = DropReportOp {
            id: "monthly-sales".into(),
        };
        assert_eq!(op.describe(), "DROP REPORT: monthly-sales");
    }

    #[test]
    fn test_serialization_round_trip() {
        let op = DropReportOp {
            id: "test-report".into(),
        };
        let yaml = serde_yaml_ng::to_string(&op).expect("serialize");
        let deserialized: DropReportOp = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(op, deserialized);
    }
}
