mod add_column;
mod always_delete;
mod always_update;
mod attach_block;
mod cast_column;
mod create_index;
mod create_join;
mod create_report;
mod create_source;
mod create_view;
mod delete;
mod detach_block;
mod drop_always_delete;
mod drop_always_update;
mod drop_cast_column;
mod drop_column;
mod drop_connector;
mod drop_function;
mod drop_index;
mod drop_join;
mod drop_report;
mod drop_view;
mod filter;
mod import_connector;
mod import_function;
mod index_blocks;
mod parameter_value;
mod rename_column;
mod rename_connector;
mod rename_function;
mod rename_join;
mod rename_view;
mod replace_block;
mod save_config;
pub(crate) mod serde_util;
mod set_description;
mod set_max_version;
mod set_min_version;
mod set_name;
mod update_data;
mod update_version;

use crate::bundle::bundle_schema::BundleSchema;
pub use crate::bundle::operation::add_column::AddColumnOp;
pub use crate::bundle::operation::always_delete::AlwaysDeleteOp;
pub use crate::bundle::operation::always_update::AlwaysUpdateOp;
pub use crate::bundle::operation::attach_block::{
    AttachBlockOp, BatchedSource, SharedAttachContext, SourceInfo,
};
pub use crate::bundle::operation::cast_column::CastColumnOp;
pub use crate::bundle::operation::create_index::CreateIndexOp;
pub use crate::bundle::operation::create_join::CreateJoinOp;
pub use crate::bundle::operation::create_report::CreateReportOp;
pub use crate::bundle::operation::create_source::CreateSourceOp;
pub use crate::bundle::operation::create_view::CreateViewOp;
pub use crate::bundle::operation::delete::DeleteOp;
pub use crate::bundle::operation::detach_block::DetachBlockOp;
pub use crate::bundle::operation::drop_always_delete::DropAlwaysDeleteOp;
pub use crate::bundle::operation::drop_always_update::DropAlwaysUpdateOp;
pub use crate::bundle::operation::drop_cast_column::DropCastColumnOp;
pub use crate::bundle::operation::drop_column::DropColumnOp;
pub use crate::bundle::operation::drop_connector::DropConnectorOp;
pub use crate::bundle::operation::drop_function::DropFunctionOp;
pub use crate::bundle::operation::drop_index::DropIndexOp;
pub use crate::bundle::operation::drop_join::DropJoinOp;
pub use crate::bundle::operation::drop_report::DropReportOp;
pub use crate::bundle::operation::drop_view::DropViewOp;
pub use crate::bundle::operation::filter::FilterOp;
pub use crate::bundle::operation::import_connector::ImportConnectorOp;
pub use crate::bundle::operation::import_function::ImportFunctionOp;
pub use crate::bundle::operation::index_blocks::IndexBlocksOp;
pub use crate::bundle::operation::rename_column::RenameColumnOp;
pub use crate::bundle::operation::rename_connector::RenameConnectorOp;
pub use crate::bundle::operation::rename_function::RenameFunctionOp;
pub use crate::bundle::operation::rename_join::RenameJoinOp;
pub use crate::bundle::operation::rename_view::RenameViewOp;
pub use crate::bundle::operation::replace_block::ReplaceBlockOp;
pub use crate::bundle::operation::save_config::SaveConfigOp;
pub use crate::bundle::operation::set_description::SetDescriptionOp;
pub use crate::bundle::operation::set_max_version::SetMaxVersionOp;
pub use crate::bundle::operation::set_min_version::SetMinVersionOp;
pub use crate::bundle::operation::set_name::SetNameOp;
pub use crate::bundle::operation::update_data::UpdateDataOp;
pub use crate::bundle::operation::update_version::UpdateVersionOp;
use crate::data::ObjectId;
use crate::object_id::ColumnId;
use crate::{versioning, Bundle, BundlebaseError};
use arrow::datatypes::DataType;
use datafusion::error::DataFusionError;
use datafusion::prelude::{DataFrame, SessionContext};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::{Debug, Display, Formatter};
use std::sync::Arc;
use uuid::Uuid;

pub use crate::bundle::operation::create_source::ExpectedColumn;

/// Context for empty export — maps source ObjectId → columns seen in fetched data.
///
/// Built by walking all `AttachBlock` operations in history, then passed into
/// `to_empty()` on each operation so each op can decide what to do.
pub struct EmptyContext {
    /// Maps source ObjectId → Vec<(column_name, ColumnId, DataType)>
    /// Built from AttachBlock history. Most recent schema seen per source wins.
    pub source_schemas: HashMap<ObjectId, Vec<(String, ColumnId, DataType)>>,
}

/// A logical change a user made. It contains one or more operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleChange {
    pub id: Uuid,
    pub description: String,
    pub operations: Vec<AnyOperation>,
    /// Runtime-only flag: when set, the auto-reindex hook in
    /// `BundleBuilder::do_change` / `run_command` is skipped for this change
    /// even if it contains AttachBlock/ReplaceBlock ops. Set by commands like
    /// `ATTACH … NO INDEX` and `FETCH … NO INDEX` so users can defer
    /// indexing until they're ready to run an explicit REINDEX.
    #[serde(skip)]
    pub suppress_auto_reindex: bool,
}

impl BundleChange {
    pub fn new(description: &str) -> Self {
        Self {
            id: Uuid::new_v4(),
            description: description.to_string(),
            operations: Vec::new(),
            suppress_auto_reindex: false,
        }
    }
}

impl Display for BundleChange {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Change: {}", self.description,)
    }
}

/// Trait for all operations
pub trait Operation: Send + Sync + Clone + Serialize + Debug + Into<AnyOperation> {
    /// Get a human-readable description of this operation
    fn describe(&self) -> String;

    /// Check that this operation is valid for the given bundle.
    /// This is called before applying the operation to ensure that the bundle is in a valid state.
    /// For example, this can be used to check that a block is attached before applying a filter operation.
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError>;

    /// Apply this operation to the bundle using interior mutability.
    /// For example, this can be used to set the bundle name.
    /// The default implementation does nothing.
    /// TODO: should return the result object, even if it's just a message
    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError>;

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        _ctx: Arc<SessionContext>,
        _bundle_schema: &mut BundleSchema,
    ) -> Result<DataFrame, BundlebaseError> {
        Ok(df)
    }

    /// Compute a content-based version hash for this operation.
    /// Default implementation uses the describe() string.
    /// Can be overridden per operation for custom versioning.
    fn version(&self) -> String {
        versioning::hash_config(self)
    }

    /// Returns whether this operation is allowed to be executed on a view.
    /// Default implementation returns true (operation is allowed on views).
    /// Override to return false for operations that should not be allowed on views.
    fn allowed_on_view(&self) -> bool {
        true
    }

    /// Returns the empty-export version of this operation, or None if it should be excluded.
    ///
    /// Used by `EXPORT EMPTY TO` to strip data-containing operations while preserving structure.
    /// Default: include as-is. Override to return `None` (exclude) or a modified copy.
    fn to_empty(&self, _context: &EmptyContext) -> Option<AnyOperation> {
        Some(self.clone().into())
    }
}

/// Macro to generate the AnyOperation enum, Operation trait impl, and From impls.
///
/// This eliminates boilerplate when adding new operations. To add a new operation:
/// 1. Create the operation module and struct
/// 2. Add it to the module declarations at the top of this file
/// 3. Add a single line to the macro invocation below
macro_rules! define_any_operation {
    (
        $(
            $variant:ident($op_type:ty)
        ),* $(,)?
    ) => {
        /// Enum wrapping all concrete operation types.
        /// This allows storing heterogeneous operations in a single Vec while maintaining type safety.
        #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
        #[serde(tag = "type", rename_all = "camelCase")]
        pub enum AnyOperation {
            $( $variant($op_type), )*
        }

        impl Operation for AnyOperation {
            fn describe(&self) -> String {
                match self {
                    $( AnyOperation::$variant(op) => op.describe(), )*
                }
            }

            async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
                match self {
                    $( AnyOperation::$variant(op) => op.check(bundle).await, )*
                }
            }

            async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
                match self {
                    $( AnyOperation::$variant(op) => op.apply(bundle).await, )*
                }
            }

            async fn apply_dataframe(
                &self,
                df: DataFrame,
                ctx: Arc<SessionContext>,
                bundle_schema: &mut BundleSchema,
            ) -> Result<DataFrame, BundlebaseError> {
                match self {
                    $( AnyOperation::$variant(op) => op.apply_dataframe(df, ctx, bundle_schema).await, )*
                }
            }

            fn version(&self) -> String {
                match self {
                    $( AnyOperation::$variant(op) => op.version(), )*
                }
            }

            fn allowed_on_view(&self) -> bool {
                match self {
                    $( AnyOperation::$variant(op) => op.allowed_on_view(), )*
                }
            }

            fn to_empty(&self, context: &EmptyContext) -> Option<AnyOperation> {
                match self {
                    $( AnyOperation::$variant(op) => op.to_empty(context), )*
                }
            }
        }

        // Generate From impls for each operation type
        $(
            impl From<$op_type> for AnyOperation {
                fn from(op: $op_type) -> Self {
                    AnyOperation::$variant(op)
                }
            }
        )*
    };
}

// Define all operations in one place.
// To add a new operation, simply add a line here.
define_any_operation! {
    AddColumn(AddColumnOp),
    AlwaysDelete(AlwaysDeleteOp),
    AlwaysUpdate(AlwaysUpdateOp),
    AttachBlock(AttachBlockOp),
    CastColumn(CastColumnOp),
    DropCastColumn(DropCastColumnOp),
    ImportFunction(ImportFunctionOp),
    CreateIndex(CreateIndexOp),
    CreateJoin(CreateJoinOp),
    CreateReport(CreateReportOp),
    CreateSource(CreateSourceOp),
    CreateView(CreateViewOp),
    Delete(DeleteOp),
    DropAlwaysDelete(DropAlwaysDeleteOp),
    DropAlwaysUpdate(DropAlwaysUpdateOp),
    UpdateData(UpdateDataOp),
    ImportConnector(ImportConnectorOp),
    DetachBlock(DetachBlockOp),
    DropColumn(DropColumnOp),
    DropIndex(DropIndexOp),
    DropConnector(DropConnectorOp),
    DropFunction(DropFunctionOp),
    DropJoin(DropJoinOp),
    DropReport(DropReportOp),
    DropView(DropViewOp),
    Filter(FilterOp),
    IndexBlocks(IndexBlocksOp),
    RenameColumn(RenameColumnOp),
    RenameConnector(RenameConnectorOp),
    RenameFunction(RenameFunctionOp),
    RenameJoin(RenameJoinOp),
    RenameView(RenameViewOp),
    ReplaceBlock(ReplaceBlockOp),
    SaveConfig(SaveConfigOp),
    SetDescription(SetDescriptionOp),
    SetMaxVersion(SetMaxVersionOp),
    SetMinVersion(SetMinVersionOp),
    SetName(SetNameOp),
    UpdateVersion(UpdateVersionOp),
}

impl Display for AnyOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.describe())
    }
}

/// Compute which operations in `ops` are active (should be applied to the DataFrame).
///
/// `DropCastColumn` cancels the most recent `CastColumn` for the same column —
/// both the cast and the drop are marked inactive so neither appears in the pipeline.
/// This is called before every `apply_dataframe` loop to handle cast reverts
/// correctly regardless of operation ordering.
///
/// Returns a `Vec<bool>` parallel to `ops`: `true` = apply, `false` = skip.
pub fn resolve_cast_ops(ops: &[AnyOperation]) -> Vec<bool> {
    let mut active = vec![true; ops.len()];
    // Stack of op indices per column: pushed on CastColumn, popped (and cancelled) on DropCastColumn.
    let mut cast_stacks: HashMap<ColumnId, Vec<usize>> = HashMap::new();

    for (i, op) in ops.iter().enumerate() {
        match op {
            AnyOperation::CastColumn(c) => {
                cast_stacks.entry(c.id).or_default().push(i);
            }
            AnyOperation::DropCastColumn(d) => {
                if let Some(stack) = cast_stacks.get_mut(&d.id) {
                    if let Some(cancelled) = stack.pop() {
                        active[cancelled] = false;
                    }
                }
                active[i] = false; // the drop itself is never applied directly
            }
            _ => {}
        }
    }

    active
}
