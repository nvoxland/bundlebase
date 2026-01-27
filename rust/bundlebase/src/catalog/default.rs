mod bundle_table;

use crate::bundle::DataFrameHolder;
use async_trait::async_trait;
use bundle_table::BundleTable;
use datafusion::catalog::{SchemaProvider, TableProvider};
use std::sync::Arc;

/// Alias dataframe is registered in the ctx under. User can select from this
pub static BUNDLE_TABLE: &str = "bundle";

/// SchemaProvider that exposes the bundle's cached dataframe as a "bundle" table
#[derive(Debug)]
pub struct DefaultSchemaProvider {
    dataframe: DataFrameHolder,
}

impl DefaultSchemaProvider {
    pub fn new(dataframe: DataFrameHolder) -> Self {
        Self { dataframe }
    }
}

#[async_trait]
impl SchemaProvider for DefaultSchemaProvider {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn table_names(&self) -> Vec<String> {
        vec![BUNDLE_TABLE.to_string()]
    }

    async fn table(&self, name: &str) -> datafusion::error::Result<Option<Arc<dyn TableProvider>>> {
        if name == BUNDLE_TABLE {
            Ok(Some(Arc::new(BundleTable::new(
                self.dataframe.clone(),
            ))))
        } else {
            Ok(None)
        }
    }

    fn table_exist(&self, name: &str) -> bool {
        name == BUNDLE_TABLE
    }
}
