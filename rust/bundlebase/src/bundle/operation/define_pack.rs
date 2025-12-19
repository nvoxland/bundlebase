use crate::bundle::operation::Operation;
use crate::data_reader::DataPack;
use crate::data_storage::ObjectId;
use crate::{BundlebaseError, Bundle};
use async_trait::async_trait;
use datafusion::error::DataFusionError;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DefinePackOp {
    pub id: ObjectId,
}

impl DefinePackOp {
    pub async fn setup(id: &ObjectId) -> Result<Self, BundlebaseError> {
        Ok(Self { id: id.clone() })
    }
}

#[async_trait]
impl Operation for DefinePackOp {
    fn describe(&self) -> String {
        format!("CREATE PACK {}", self.id)
    }

    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        Ok(())
    }

    async fn apply(&self, bundle: &mut Bundle) -> Result<(), DataFusionError> {
        bundle.add_pack(self.id.clone(), Arc::new(DataPack::new(self.id.clone())));
        if bundle.base_pack.is_none() {
            bundle.base_pack = Some(self.id.clone());
        }

        Ok(())
    }
}
