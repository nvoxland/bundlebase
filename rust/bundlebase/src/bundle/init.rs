use serde::{Deserialize, Serialize};
use url::Url;

pub static INIT_FILENAME: &str = "00000000000000000.yaml";

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InitCommit {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_version: Option<String>,
}

impl InitCommit {
    pub fn new(from: Option<&Url>) -> Self {
        let version = bundlebase_common::format_version_string();
        Self {
            // Only set id when creating a new bundle (from is None)
            // When extending (from is Some), id should be None and inherited from parent
            id: if from.is_none() {
                Some(uuid::Uuid::new_v4().to_string())
            } else {
                None
            },
            from: from.cloned(),
            view: None,
            min_version: Some(version.clone()),
            max_version: Some(version),
        }
    }

    pub fn new_view(view_id: &str) -> Self {
        let version = bundlebase_common::format_version_string();
        Self {
            id: None,
            from: None,
            view: Some(view_id.to_string()),
            min_version: Some(version.clone()),
            max_version: Some(version),
        }
    }
}
