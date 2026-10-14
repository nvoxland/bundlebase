use super::bundle_schema::BundleSchema;
use super::operation::BundleChange;
use crate::bundle::BundleCommit;
use crate::bundle::BundleStatus;
use crate::bundle::Pack;
use crate::bundle::Source;
use crate::bundle::{AlwaysUpdateRule, ReportEntry};
use crate::bundle_config::Scope;
use crate::index::IndexDefinition;
use crate::io::{IOReadWriteDir, ObjectId};
use crate::object_id::ColumnId;
use crate::{AnyOperation, Bundle, BundleBuilder, BundleConfig, BundlebaseError};
use arrow_schema::{Field, Schema, SchemaRef};
use async_trait::async_trait;
use datafusion::common::ScalarValue;
use datafusion::dataframe::DataFrame;
use datafusion::execution::SendableRecordBatchStream;
use datafusion::prelude::SessionContext;
use std::collections::HashMap;
use std::sync::Arc;
use url::Url;

mod data_quality;
pub use data_quality::{ColumnQuality, DataQualityReport};

#[async_trait]
pub trait BundleFacade: Send + Sync {
    /// Returns self as `&dyn Any` for downcasting to concrete types.
    fn as_any(&self) -> &dyn std::any::Any;

    /// The id of the bundle
    fn id(&self) -> String;

    /// Retrieve the bundle name, if set.
    fn name(&self) -> Option<String>;

    /// Retrieve the bundle description, if set.
    fn description(&self) -> Option<String>;

    /// Retrieve the URL of the base bundle this was loaded from, if any.
    fn url(&self) -> Url;

    /// The base bundle this was extended from
    fn from(&self) -> Option<Url>;

    /// Unique version for this bundle
    fn version(&self) -> String;

    /// Returns the commit history for this bundle, including any base bundles
    fn history(&self) -> Vec<BundleCommit>;

    /// All operations applied to this bundle
    fn operations(&self) -> Vec<AnyOperation>;

    /// Returns the full BundleSchema with column registry and cached schemas.
    fn bundle_schema(&self) -> BundleSchema;

    /// Resolve a column name to its ColumnId using the current operations.
    fn column_id(&self, name: &str) -> Option<ColumnId> {
        self.bundle_schema().column_id(name)
    }

    /// Resolve a ColumnId to its current column name using the current operations.
    fn column_name(&self, id: &ColumnId) -> Option<String> {
        self.bundle_schema().column_name(id)
    }

    /// Rewrite SQL stored in an operation (which references columns as
    /// `col_<id>` and imported functions as `fn_<id>`) to the *current*
    /// user-visible names. The result is valid against the `bundle` table via
    /// [`query`](Self::query), and reads correctly after renames.
    fn display_sql(&self, sql: &str) -> String {
        let columns_resolved = self.bundle_schema().display_sql(sql);
        let fn_map: HashMap<String, String> = self
            .function_registry()
            .read()
            .name_to_internal_name()
            .into_iter()
            .map(|(name, internal)| (internal, name))
            .collect();
        if fn_map.is_empty() {
            return columns_resolved;
        }
        super::bundle_schema::translate_identifiers_in_sql(&columns_resolved, &fn_map)
    }

    /// Resolve the bundle's metrics (from `DefineMetric` operations),
    /// last-write-wins by name, in definition order. Metrics are metadata
    /// replayed from the operation list, so this reflects committed +
    /// in-session state. Expressions are stored against stable column ids and
    /// returned here with the columns' *current* user-visible names.
    fn metrics(&self) -> Vec<super::operation::MetricDefinition> {
        use super::operation::MetricDefinition;
        let mut by_name: std::collections::HashMap<String, MetricDefinition> =
            std::collections::HashMap::new();
        let mut order: Vec<String> = Vec::new();
        for op in self.operations() {
            match op {
                AnyOperation::DefineMetric(m) => {
                    if !by_name.contains_key(&m.name) {
                        order.push(m.name.clone());
                    }
                    by_name.insert(
                        m.name.clone(),
                        MetricDefinition {
                            name: m.name,
                            expression: m.expression,
                            description: m.description,
                        },
                    );
                }
                // A DROP removes the name; a later DEFINE can re-add it (the
                // trailing filter_map drops any `order` entry no longer present).
                AnyOperation::DropMetric(d) => {
                    by_name.remove(&d.name);
                }
                _ => {}
            }
        }
        order
            .into_iter()
            .filter_map(|n| by_name.remove(&n))
            .map(|m| MetricDefinition {
                expression: self.display_sql(&m.expression),
                ..m
            })
            .collect()
    }

    /// Resolve column descriptions (from `DescribeColumn` operations) keyed by
    /// `ColumnId`, last-write-wins. Descriptions are metadata replayed from the
    /// operation list, so this reflects the current committed + in-session state.
    fn column_descriptions(&self) -> HashMap<ColumnId, String> {
        let mut out = HashMap::new();
        for op in self.operations() {
            if let AnyOperation::DescribeColumn(d) = op {
                out.insert(d.id, d.description);
            }
        }
        out
    }

    /// Column descriptions keyed by each column's *current* user-visible name
    /// (descriptions are stored by `ColumnId`, so they follow renames).
    /// Columns that no longer exist are omitted.
    fn column_descriptions_by_name(&self) -> HashMap<String, String> {
        let descriptions = self.column_descriptions();
        if descriptions.is_empty() {
            return HashMap::new();
        }
        let bs = self.bundle_schema();
        descriptions
            .into_iter()
            .filter_map(|(id, desc)| bs.column_name(&id).map(|name| (name, desc)))
            .collect()
    }

    /// Evaluate a named metric, optionally grouped by `dimensions` (columns of
    /// the bundle), returning a stream of rows: one column per dimension plus
    /// a column named after the metric. Grouped results are ordered by the
    /// dimensions. `limit` caps the number of rows returned.
    async fn compute_metric(
        &self,
        name: &str,
        dimensions: &[String],
        limit: Option<usize>,
    ) -> Result<SendableRecordBatchStream, BundlebaseError> {
        let metrics = self.metrics();
        let metric = metrics.iter().find(|m| m.name == name).ok_or_else(|| {
            let available: Vec<&str> = metrics.iter().map(|m| m.name.as_str()).collect();
            let msg = if available.is_empty() {
                format!(
                    "No metric named '{}'. This bundle defines no metrics; create one with \
                     CREATE METRIC <name> AS '<expression>'.",
                    name
                )
            } else {
                format!(
                    "No metric named '{}'. Available metrics: {}.",
                    name,
                    available.join(", ")
                )
            };
            BundlebaseError::from(msg)
        })?;

        // Only real columns may be used as dimensions: this guards the
        // generated SQL and gives a self-correcting error.
        let schema = self.schema().await?;
        for d in dimensions {
            if schema.field_with_name(d).is_err() {
                let mut cols: Vec<&str> =
                    schema.fields().iter().map(|f| f.name().as_str()).collect();
                cols.sort_unstable();
                return Err(format!(
                    "Unknown dimension column '{}'. Available columns: {}.",
                    d,
                    cols.join(", ")
                )
                .into());
            }
        }

        let q = |ident: &str| format!("\"{}\"", ident.replace('"', "\"\""));
        let dims: Vec<String> = dimensions.iter().map(|d| q(d)).collect();
        let mut select_parts = dims.clone();
        // `metrics()` already rendered the expression with current column names.
        select_parts.push(format!("({}) AS {}", metric.expression, q(name)));

        let mut sql = format!("SELECT {} FROM bundle", select_parts.join(", "));
        if !dims.is_empty() {
            let dims_sql = dims.join(", ");
            sql.push_str(&format!(" GROUP BY {} ORDER BY {}", dims_sql, dims_sql));
        }
        if let Some(n) = limit {
            sql.push_str(&format!(" LIMIT {}", n));
        }
        self.query(&sql, vec![], limit).await
    }

    /// Profile every column in one scan: null and distinct counts, plus — for
    /// text columns — whether every value parses as a number or date (see
    /// [`DataQualityReport`]). The empty-bundle sentinel column is skipped.
    async fn data_quality(&self) -> Result<DataQualityReport, BundlebaseError> {
        let schema = self.schema().await?;
        let bs = self.bundle_schema();
        data_quality::compute(self, &schema, &bs).await
    }

    async fn schema(&self) -> Result<SchemaRef, BundlebaseError>;

    /// The user-visible schema with column descriptions (from `DescribeColumn`)
    /// attached as Arrow field metadata under the key `"description"`. This is
    /// how the semantic layer reaches Arrow/Flight/BI consumers that read field
    /// metadata — not just MCP agents that read `describe`. When a bundle has no
    /// column descriptions the schema is returned unchanged (same `Arc`), so
    /// there is no cost or metadata drift on the common path.
    async fn described_schema(&self) -> Result<SchemaRef, BundlebaseError> {
        let schema = self.schema().await?;
        let descriptions = self.column_descriptions_by_name();
        if descriptions.is_empty() {
            return Ok(schema);
        }
        let fields: Vec<Arc<Field>> = schema
            .fields()
            .iter()
            .map(|f| {
                match descriptions.get(f.name()) {
                    Some(desc) => {
                        let mut metadata = f.metadata().clone();
                        metadata.insert("description".to_string(), desc.clone());
                        Arc::new(f.as_ref().clone().with_metadata(metadata))
                    }
                    None => f.clone(),
                }
            })
            .collect();
        Ok(Arc::new(Schema::new_with_metadata(
            fields,
            schema.metadata().clone(),
        )))
    }

    /// Returns the schema with internal column names instead of user-visible names.
    ///
    /// Uses the BundleSchema's cached internal schema if available,
    /// otherwise computes it from the user-visible schema and column registry.
    async fn internal_schema(&self) -> Result<SchemaRef, BundlebaseError> {
        let bs = self.bundle_schema();
        if let Some(cached) = bs.internal_schema() {
            return Ok(cached);
        }
        // Fallback: compute from schema + column_names
        use super::bundle_schema;
        let schema = self.schema().await?;
        let col_id_fields: Vec<Arc<arrow_schema::Field>> = schema
            .fields()
            .iter()
            .filter_map(|f| {
                bs.iter()
                    .find(|(_, name)| name.as_str() == f.name())
                    .map(|(id, _)| {
                        Arc::new(
                            f.as_ref()
                                .clone()
                                .with_name(bundle_schema::generate_internal_name(id)),
                        )
                    })
            })
            .collect();
        Ok(Arc::new(arrow_schema::Schema::new(col_id_fields)))
    }

    /// Computes the number of rows in the bundle
    async fn num_rows(&self) -> Result<usize, BundlebaseError>;

    /// Builds and returns the final DataFrame
    async fn dataframe(&self) -> Result<Arc<DataFrame>, BundlebaseError>;

    /// Extends this bundle to create a new BundleBuilder.
    async fn extend(&self, data_dir: Option<&str>) -> Result<Arc<BundleBuilder>, BundlebaseError>;

    /// Executes a SQL query and returns streaming results directly.
    async fn query(
        &self,
        sql: &str,
        params: Vec<ScalarValue>,
        hard_limit: Option<usize>,
    ) -> Result<SendableRecordBatchStream, BundlebaseError>;

    /// Returns a map of view IDs to view names for all views in this container
    fn views(&self) -> HashMap<ObjectId, String>;

    /// Open a view by name or ID, returning a read-only Bundle
    async fn view(&self, identifier: &str) -> Result<Arc<Bundle>, BundlebaseError>;

    /// Exports the bundle's data directory to an uncompressed tar archive.
    async fn export_tar(&self, tar_path: &str, gzip: bool) -> Result<String, BundlebaseError>;

    /// Returns uncommitted changes (empty for Bundle, populated for BundleBuilder)
    fn status_changes(&self) -> Vec<BundleChange>;

    /// Returns the current bundle status
    fn status(&self) -> BundleStatus;

    /// Returns index definitions
    fn indexes(&self) -> Vec<Arc<IndexDefinition>>;

    /// Returns packs (id -> pack)
    fn packs(&self) -> HashMap<ObjectId, Arc<Pack>>;

    /// Returns sources (id -> source)
    fn sources(&self) -> HashMap<ObjectId, Arc<Source>>;

    /// Returns views by name (name -> id mapping)
    fn views_by_name(&self) -> HashMap<String, ObjectId>;

    /// Returns all stored reports (id -> entry)
    fn reports(&self) -> HashMap<String, ReportEntry>;

    /// Returns the current always-delete rules (WHERE clauses)
    fn always_delete_rules(&self) -> Vec<String>;

    /// Returns the current always-update rules (SET/WHERE clauses)
    fn always_update_rules(&self) -> Vec<AlwaysUpdateRule>;

    /// Returns the data directory for this bundle
    fn data_dir(&self) -> Arc<dyn IOReadWriteDir>;

    /// Returns the bundle configuration
    fn config(&self) -> Arc<BundleConfig>;

    /// Remove runtime-only connector for a defined source.
    async fn drop_temp_connector(
        &self,
        name: &str,
        platform: Option<&crate::platform::Platform>,
    ) -> Result<usize, BundlebaseError>;

    /// Remove runtime-only function entries.
    async fn drop_temp_function(
        &self,
        name: &str,
        platform: Option<&crate::platform::Platform>,
    ) -> Result<usize, BundlebaseError>;

    /// Rename runtime-only connector entries.
    async fn rename_temp_connector(
        &self,
        old_name: &str,
        new_name: &str,
    ) -> Result<(), BundlebaseError>;

    /// Rename runtime-only function entries.
    async fn rename_temp_function(
        &self,
        old_name: &str,
        new_name: &str,
    ) -> Result<(), BundlebaseError>;

    /// Set a runtime config value (session-only, highest priority).
    async fn set_config(
        &self,
        scope: &Scope,
        key: &str,
        value: &str,
    ) -> Result<(), BundlebaseError>;

    /// Returns the connector registry.
    fn connector_registry(&self) -> Arc<parking_lot::RwLock<crate::source::ConnectorRegistry>>;

    /// Returns the function registry.
    fn function_registry(
        &self,
    ) -> Arc<parking_lot::RwLock<crate::bundle::function_entry::FunctionRegistry>>;

    /// Returns the DataFusion session context
    fn ctx(&self) -> Arc<SessionContext>;
}
