use serde::{Deserialize, Serialize};
use url::Url;

pub static INIT_FILENAME: &str = "00000000000000000.yaml";

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InitCommit {
    pub id: String,
    pub from: Option<Url>,
}

impl InitCommit {
    pub fn new(from: Option<&Url>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            from: from.cloned(),
        }
    }
}
