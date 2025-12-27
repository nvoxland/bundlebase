use crate::bundle::commit::BundleCommit;
use crate::bundle::init::{InitCommit, INIT_FILENAME};
use crate::bundle::operation::{AnyOperation, BundleChange, Operation};
use crate::{Bundle, BundleBuilder, BundlebaseError};
use crate::data::ObjectId;
use async_trait::async_trait;
use datafusion::common::DataFusionError;
use datafusion::prelude::DataFrame;
use datafusion::execution::context::SessionContext;
use log::debug;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use url::Url;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AttachViewOp {
    pub name: String,
    pub view_id: ObjectId,
}

impl AttachViewOp {
    pub async fn setup(
        name: &str,
        source_builder: &BundleBuilder,
        parent_builder: &BundleBuilder,
    ) -> Result<Self, BundlebaseError> {
        debug!("Setting up view '{}' from source builder", name);

        // 1. Generate view ID
        let view_id = ObjectId::generate();
        debug!("Generated view ID: {}", view_id);

        // 2. Extract uncommitted operations from source_builder
        let operations: Vec<AnyOperation> = source_builder.status().operations();
        debug!("Captured {} operations from source builder", operations.len());
        for (i, op) in operations.iter().enumerate() {
            debug!("  Captured op {}: {}", i, op.describe());
        }

        // 3. Create view directory: _manifest/view_{id}/_manifest/
        let view_dir = parent_builder
            .data_dir()
            .subdir(&format!("_manifest/view_{}", view_id))?;
        let view_manifest_dir = view_dir.subdir("_manifest")?;
        debug!("Creating view manifest directory");

        // 4. Create init commit with from pointing to parent container
        // Use parent's data_dir URL as the from reference
        let parent_url = parent_builder.data_dir().url().clone();
        let init = InitCommit::new(Some(&parent_url));
        view_manifest_dir
            .file(INIT_FILENAME)?
            .write_yaml(&init)
            .await?;
        debug!("Wrote init commit with from='../../..'");

        // 5. Create first commit with captured operations
        let now = std::time::SystemTime::now();
        let timestamp = {
            use chrono::DateTime;
            let datetime: DateTime<chrono::Utc> = now.into();
            datetime.format("%Y-%m-%dT%H:%M:%SZ").to_string()
        };

        let author = std::env::var("BUNDLEBASE_AUTHOR")
            .unwrap_or_else(|_| std::env::var("USER").unwrap_or_else(|_| "unknown".to_string()));

        let commit = BundleCommit {
            url: None,
            data_dir: None,
            message: format!("View: {}", name),
            author,
            timestamp,
            changes: vec![BundleChange {
                id: Uuid::new_v4(),
                description: format!("Define view '{}'", name),
                operations: operations.clone(),
            }],
        };

        // 6. Write commit: 00001{hash}.yaml
        let yaml = serde_yaml::to_string(&commit)?;
        let mut hasher = Sha256::new();
        hasher.update(yaml.as_bytes());
        let hash_bytes = hasher.finalize();
        let hash_hex = hex::encode(hash_bytes);
        let hash_short = &hash_hex[..12];

        let filename = format!("00001{}.yaml", hash_short);
        let data = bytes::Bytes::from(yaml);
        let stream = futures::stream::iter(vec![Ok::<_, std::io::Error>(data)]);
        view_manifest_dir.file(&filename)?.write_stream(stream).await?;
        debug!("Wrote view commit: {} with {} operations", filename, operations.len());

        Ok(AttachViewOp {
            name: name.to_string(),
            view_id,
        })
    }
}

#[async_trait]
impl Operation for AttachViewOp {
    fn describe(&self) -> String {
        format!("ATTACH VIEW: '{}'", self.name)
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Check view name doesn't already exist
        if bundle.views.contains_key(&self.name) {
            return Err(format!("View '{}' already exists", self.name).into());
        }
        Ok(())
    }

    async fn apply(&self, bundle: &mut Bundle) -> Result<(), DataFusionError> {
        // Store view name->id mapping
        bundle.views.insert(self.name.clone(), self.view_id.clone());
        Ok(())
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        _ctx: Arc<SessionContext>,
    ) -> Result<DataFrame, BundlebaseError> {
        // AttachViewOp doesn't modify the dataframe
        Ok(df)
    }

    fn version(&self) -> String {
        // Compute version hash based on the operation's content
        let mut hasher = Sha256::new();
        hasher.update(self.name.as_bytes());
        hasher.update(self.view_id.to_string().as_bytes());
        let hash_bytes = hasher.finalize();
        hex::encode(hash_bytes)
    }
}
