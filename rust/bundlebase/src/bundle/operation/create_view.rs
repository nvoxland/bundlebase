use crate::bundle::commit::BundleCommit;
use crate::bundle::operation::{AnyOperation, BundleChange, FilterOp, Operation};
use crate::bundle::META_DIR;
use crate::data::ObjectId;
use crate::io::write_yaml;
use crate::{Bundle, BundleBuilder, BundleFacade, BundlebaseError};
use async_trait::async_trait;
use datafusion::common::DataFusionError;
use datafusion::execution::context::SessionContext;
use datafusion::prelude::DataFrame;
use log::debug;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CreateViewOp {
    pub name: String,
    pub id: ObjectId,
}

impl CreateViewOp {
    /// Setup a view from an SQL statement.
    ///
    /// The SQL statement defines the view's query. The view will be stored
    /// in a subdirectory of the parent bundle.
    ///
    /// Returns the CreateViewOp and the view's BundleBuilder.
    pub async fn setup(
        name: &str,
        sql: &str,
        parent_builder: &BundleBuilder,
    ) -> Result<(Self, Arc<BundleBuilder>), BundlebaseError> {
        debug!("Setting up view '{}' with SQL: {}", name, sql);

        // 1. Generate view ID
        let view_id = ObjectId::generate();
        debug!("Generated view ID: {}", view_id);

        // 2. Create a FilterOp from the SQL
        let filter_op = FilterOp::new(sql.to_string(), vec![]);
        let operations: Vec<AnyOperation> = vec![AnyOperation::Filter(filter_op)];
        debug!("Created FilterOp for view");

        // 3. Create view builder by extending parent to view location
        // This handles bundle cloning, config preservation, and directory setup
        let view_dir_path = parent_builder
            .data_dir()
            .writable_subdir(&format!("view_{}", view_id))?
            .url()
            .to_string();

        let view_builder =
            BundleBuilder::extend(Arc::new(parent_builder.bundle().clone()), Some(&view_dir_path))?;

        // Note: We do NOT apply operations to the view bundle during setup.
        // Operations are stored in the commit file and will be applied when
        // the view is opened via Bundle::open().

        // 4. Write view manifest files
        // We manually write init and commit files since we can't access private
        // status/do_change APIs. This follows the same pattern as commit() but
        // simplified for the view creation case.

        // Get timestamp and author
        let now = std::time::SystemTime::now();
        let timestamp = {
            use chrono::DateTime;
            let datetime: DateTime<chrono::Utc> = now.into();
            datetime.format("%Y-%m-%dT%H:%M:%SZ").to_string()
        };

        let author = std::env::var("BUNDLEBASE_AUTHOR")
            .unwrap_or_else(|_| std::env::var("USER").unwrap_or_else(|_| "unknown".to_string()));

        // Create commit structure
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

        // Write commit and init files to view manifest directory
        let manifest_dir = view_builder.data_dir().writable_subdir(META_DIR)?;

        // Write commit: 00001{hash}.yaml
        let yaml = serde_yaml_ng::to_string(&commit)?;
        let mut hasher = Sha256::new();
        hasher.update(yaml.as_bytes());
        let hash_bytes = hasher.finalize();
        let hash_hex = hex::encode(hash_bytes);
        let hash_short = &hash_hex[..12];

        let filename = format!("00001{}.yaml", hash_short);
        let data = bytes::Bytes::from(yaml);
        let stream = futures::stream::iter(vec![Ok::<_, std::io::Error>(data)]);
        manifest_dir.writable_file(&filename)?.write_stream(Box::pin(stream)).await?;
        debug!(
            "Wrote view commit: {} with {} operations",
            filename,
            operations.len()
        );

        // Write init commit with VIEW field
        use crate::bundle::init::{InitCommit, INIT_FILENAME};
        let init = InitCommit::new_view(&view_id.to_string());
        write_yaml(manifest_dir.writable_file(INIT_FILENAME)?.as_ref(), &init).await?;
        debug!("Wrote init commit with VIEW={}", view_id);

        debug!("View '{}' created at {}", name, view_dir_path);

        Ok((CreateViewOp {
            name: name.to_string(),
            id: view_id,
        }, view_builder))
    }
}

#[async_trait]
impl Operation for CreateViewOp {
    fn describe(&self) -> String {
        format!("CREATE VIEW: '{}'", self.name)
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Check view name doesn't already exist
        let views = bundle.views.read();
        debug!(
            "Checking if view '{}' exists. Current views: {:?}",
            self.name,
            views.keys().collect::<Vec<_>>()
        );
        if views.contains_key(&self.name) {
            return Err(format!("View '{}' already exists", self.name).into());
        }
        Ok(())
    }

    fn allowed_on_view(&self) -> bool {
        false
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        // Store view name->id mapping
        bundle.views.write().insert(self.name.clone(), self.id);
        Ok(())
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        _ctx: Arc<SessionContext>,
    ) -> Result<DataFrame, BundlebaseError> {
        // CreateViewOp doesn't modify the dataframe
        Ok(df)
    }

    fn version(&self) -> String {
        // Compute version hash based on the operation's content
        let mut hasher = Sha256::new();
        hasher.update(self.name.as_bytes());
        hasher.update(self.id.to_string().as_bytes());
        let hash_bytes = hasher.finalize();
        hex::encode(hash_bytes)
    }
}
