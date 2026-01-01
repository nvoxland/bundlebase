use crate::bundle::operation::Operation;
use crate::data::{DataBlock, ObjectId};
use crate::progress::ProgressScope;
use crate::{Bundle, BundleBuilder, BundlebaseError};
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use datafusion::common::DataFusionError;
use log::debug;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AttachBlockOp {
    pub source: String,
    pub version: String,
    pub id: ObjectId,
    pub pack_id: ObjectId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_rows: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<usize>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "super::serde_util::serialize_schema_option",
        deserialize_with = "super::serde_util::deserialize_schema_option"
    )]
    pub schema: Option<SchemaRef>,
}

impl AttachBlockOp {
    pub async fn setup(
        pack_id: &ObjectId,
        source: &str,
        builder: &BundleBuilder,
    ) -> Result<Self, BundlebaseError> {
        // Create progress scope (indeterminate - we don't know how many steps)
        let _progress = ProgressScope::new(
            &format!("Attaching '{}'", source),
            None, // indeterminate progress
        );

        let block_id = ObjectId::generate();

        _progress.update(1, Some("Creating adapter"));
        let adapter = builder
            .bundle
            .adapter_factory
            .reader(source, &block_id, builder.bundle(), None, None)
            .await?;

        _progress.update(2, Some("Reading version"));
        let version = adapter.read_version().await?;

        _progress.update(3, Some("Reading schema"));
        let schema = adapter.read_schema().await?;

        let mut op = AttachBlockOp {
            source: source.to_string(),
            num_rows: None,
            bytes: None,
            version,
            schema,
            id: block_id,
            pack_id: pack_id.clone(),
            layout: None,
        };

        _progress.update(4, Some("Reading statistics"));
        match adapter.read_statistics().await? {
            Some(stats) => {
                op.num_rows = stats.num_rows.get_value().copied();
                op.bytes = stats.total_byte_size.get_value().copied();
            }
            None => {
                debug!("No statistics available for adapter at {}", adapter.url());
            }
        }

        _progress.update(5, Some("Building layout"));
        op.layout = adapter.build_layout(builder.data_dir()).await?;

        Ok(op)
    }
}

#[async_trait]
impl Operation for AttachBlockOp {
    fn describe(&self) -> String {
        format!("ATTACH: {}", self.source)
    }

    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        Ok(())
    }

    fn allowed_on_view(&self) -> bool {
        false
    }

    async fn apply(&self, bundle: &mut Bundle) -> Result<(), DataFusionError> {
        let reader = bundle
            .adapter_factory
            .reader(
                self.source.as_str(),
                &self.id,
                bundle,
                self.schema.clone(),
                self.layout.clone(),
            )
            .await?;

        let block = Arc::new(DataBlock::new(
            self.id.clone(),
            self.schema.clone().unwrap(),
            &self.version,
            reader,
            bundle.indexes().clone(),
            Arc::new(bundle.data_dir().clone()),
            bundle.config(),
        ));

        let pack = bundle.get_pack(&self.pack_id).expect("Cannot find pack");
        pack.add_block(block);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::BundleFacade;
    use crate::io::ObjectStoreFile;
    use crate::test_utils::{empty_bundle, for_yaml, test_datafile};
    use crate::BundleConfig;
    use url::Url;

    #[tokio::test]
    async fn test_describe() {
        let op = AttachBlockOp {
            source: "file:///test/data.csv".to_string(),
            version: "test-version".to_string(),
            id: ObjectId::from(1),
            pack_id: ObjectId::from(2),
            num_rows: None,
            bytes: None,
            schema: None,
            layout: None,
        };

        assert_eq!(op.describe(), "ATTACH: file:///test/data.csv");
    }

    #[tokio::test]
    async fn test_setup() -> Result<(), BundlebaseError> {
        let datafile = test_datafile("userdata.parquet");
        let op =
            AttachBlockOp::setup(&ObjectId::generate(), datafile, &empty_bundle().await).await?;
        let block_id = String::from(op.id.clone());
        let pack_id = String::from(op.pack_id.clone());
        let version = ObjectStoreFile::from_url(
            &Url::parse(datafile).unwrap(),
            BundleConfig::default().into(),
        )?
        .version()
        .await?;

        assert_eq!(
            format!(
                r#"source: memory:///test_data/userdata.parquet
version: {}
id: {}
packId: {}
numRows: 1000
bytes: 113629
schema:
  fields:
  - name: registration_dttm
    data_type:
      type: Timestamp
      unit: Nanosecond
      timezone: null
    nullable: true
    dict_id: 0
    dict_is_ordered: false
    metadata: {{}}
  - name: id
    data_type: Int32
    nullable: true
    dict_id: 0
    dict_is_ordered: false
    metadata: {{}}
  - name: first_name
    data_type: Utf8View
    nullable: true
    dict_id: 0
    dict_is_ordered: false
    metadata: {{}}
  - name: last_name
    data_type: Utf8View
    nullable: true
    dict_id: 0
    dict_is_ordered: false
    metadata: {{}}
  - name: email
    data_type: Utf8View
    nullable: true
    dict_id: 0
    dict_is_ordered: false
    metadata: {{}}
  - name: gender
    data_type: Utf8View
    nullable: true
    dict_id: 0
    dict_is_ordered: false
    metadata: {{}}
  - name: ip_address
    data_type: Utf8View
    nullable: true
    dict_id: 0
    dict_is_ordered: false
    metadata: {{}}
  - name: cc
    data_type: Utf8View
    nullable: true
    dict_id: 0
    dict_is_ordered: false
    metadata: {{}}
  - name: country
    data_type: Utf8View
    nullable: true
    dict_id: 0
    dict_is_ordered: false
    metadata: {{}}
  - name: birthdate
    data_type: Utf8View
    nullable: true
    dict_id: 0
    dict_is_ordered: false
    metadata: {{}}
  - name: salary
    data_type: Float64
    nullable: true
    dict_id: 0
    dict_is_ordered: false
    metadata: {{}}
  - name: title
    data_type: Utf8View
    nullable: true
    dict_id: 0
    dict_is_ordered: false
    metadata: {{}}
  - name: comments
    data_type: Utf8View
    nullable: true
    dict_id: 0
    dict_is_ordered: false
    metadata: {{}}
  metadata: {{}}
"#,
                for_yaml(version),
                for_yaml(block_id),
                for_yaml(pack_id),
            ),
            serde_yaml::to_string(&op)?
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_attach_dataframe_schema() -> Result<(), BundlebaseError> {
        let mut bundle = crate::BundleBuilder::create("memory:///test_bundle", None).await?;
        bundle.attach(test_datafile("userdata.parquet")).await?;

        // Get the DataFrame from the bundle
        let df = bundle.dataframe().await?;
        let df_schema = df.schema();

        // Verify DataFrame schema has correct column names and types
        let schema_string = df_schema
            .fields()
            .iter()
            .map(|f| format!("{}: {}", f.name(), f.data_type()))
            .collect::<Vec<_>>()
            .join("\n");

        // Expected schema with all column names and their data types from the parquet file
        let expected_schema = "registration_dttm: Timestamp(ns)\n\
                               id: Int32\n\
                               first_name: Utf8View\n\
                               last_name: Utf8View\n\
                               email: Utf8View\n\
                               gender: Utf8View\n\
                               ip_address: Utf8View\n\
                               cc: Utf8View\n\
                               country: Utf8View\n\
                               birthdate: Utf8View\n\
                               salary: Float64\n\
                               title: Utf8View\n\
                               comments: Utf8View";

        assert_eq!(schema_string, expected_schema,);

        Ok(())
    }

    #[tokio::test]
    async fn test_version() {
        let op = AttachBlockOp {
            source: "file:///test/data.csv".to_string(),
            version: "test-version".to_string(),
            id: ObjectId::from(1),
            pack_id: ObjectId::from(2),
            num_rows: None,
            bytes: None,
            schema: None,
            layout: None,
        };

        let version = op.version();

        assert_eq!(version, "11843d14f9e4");
    }
}
//
