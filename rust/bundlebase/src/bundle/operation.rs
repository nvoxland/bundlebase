mod attach_block;
mod create_view;
mod define_function;
mod define_index;
mod define_pack;
mod drop_index;
mod drop_view;
mod filter;
mod index_blocks;
mod join;
mod rebuild_index;
mod remove_columns;
mod rename_column;
mod rename_view;
mod select;
mod serde_util;
mod set_config;
mod set_description;
mod set_name;

pub use crate::bundle::operation::attach_block::AttachBlockOp;
pub use crate::bundle::operation::create_view::CreateViewOp;
pub use crate::bundle::operation::define_function::DefineFunctionOp;
pub use crate::bundle::operation::define_index::DefineIndexOp;
pub use crate::bundle::operation::define_pack::DefinePackOp;
pub use crate::bundle::operation::drop_index::DropIndexOp;
pub use crate::bundle::operation::drop_view::DropViewOp;
pub use crate::bundle::operation::filter::FilterOp;
pub use crate::bundle::operation::index_blocks::IndexBlocksOp;
pub use crate::bundle::operation::join::{JoinOp, JoinTypeOption};
pub use crate::bundle::operation::rebuild_index::RebuildIndexOp;
pub use crate::bundle::operation::remove_columns::RemoveColumnsOp;
pub use crate::bundle::operation::rename_column::RenameColumnOp;
pub use crate::bundle::operation::rename_view::RenameViewOp;
pub use crate::bundle::operation::select::SelectOp;
pub use crate::bundle::operation::set_config::SetConfigOp;
pub use crate::bundle::operation::set_description::SetDescriptionOp;
pub use crate::bundle::operation::set_name::SetNameOp;
use crate::{versioning, Bundle, BundlebaseError};
use async_trait::async_trait;
use datafusion::error::DataFusionError;
use datafusion::prelude::{DataFrame, SessionContext};
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Display, Formatter};
use std::sync::Arc;
use uuid::Uuid;

/// A logical change a user made. It contains one or more operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleChange {
    pub id: Uuid,
    pub description: String,
    pub operations: Vec<AnyOperation>,
}

impl BundleChange {
    pub fn new(description: &str) -> Self {
        Self {
            id: Uuid::new_v4(),
            description: description.to_string(),
            operations: Vec::new(),
        }
    }
}

impl Display for BundleChange {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Change: {}", self.description,)
    }
}

/// Trait for all operations
#[async_trait]
pub trait Operation: Send + Sync + Clone + Serialize + Debug {
    /// Get a human-readable description of this operation
    fn describe(&self) -> String;

    /// Check that this operation is valid for the given bundle.
    /// This is called before applying the operation to ensure that the bundle is in a valid state.
    /// For example, this can be used to check that a block is attached before applying a filter operation.
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError>;

    /// Apply this operation to the bundle.
    /// For example, this can be used to set the bundle name.
    /// The default implementation does nothing.
    async fn apply(&self, bundle: &mut Bundle) -> Result<(), DataFusionError>;

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        _ctx: Arc<SessionContext>,
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
}

/// Enum wrapping all concrete operation types
/// This allows storing heterogeneous operations in a single Vec while maintaining type safety
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AnyOperation {
    RemoveColumns(RemoveColumnsOp),
    RenameColumn(RenameColumnOp),
    RenameView(RenameViewOp),
    AttachBlock(AttachBlockOp),
    CreateView(CreateViewOp),
    DefineFunction(DefineFunctionOp),
    Filter(FilterOp),
    IndexBlocks(IndexBlocksOp),
    DefineIndex(DefineIndexOp),
    DefinePack(DefinePackOp),
    DropIndex(DropIndexOp),
    DropView(DropViewOp),
    RebuildIndex(RebuildIndexOp),
    Join(JoinOp),
    Select(SelectOp),
    SetConfig(SetConfigOp),
    SetName(SetNameOp),
    SetDescription(SetDescriptionOp),
}

#[async_trait]
impl Operation for AnyOperation {
    fn describe(&self) -> String {
        match self {
            AnyOperation::RemoveColumns(op) => op.describe(),
            AnyOperation::RenameColumn(op) => op.describe(),
            AnyOperation::RenameView(op) => op.describe(),
            AnyOperation::AttachBlock(op) => op.describe(),
            AnyOperation::CreateView(op) => op.describe(),
            AnyOperation::DefineFunction(op) => op.describe(),
            AnyOperation::Filter(op) => op.describe(),
            AnyOperation::IndexBlocks(op) => op.describe(),
            AnyOperation::DefineIndex(op) => op.describe(),
            AnyOperation::DefinePack(op) => op.describe(),
            AnyOperation::DropIndex(op) => op.describe(),
            AnyOperation::DropView(op) => op.describe(),
            AnyOperation::RebuildIndex(op) => op.describe(),
            AnyOperation::Join(op) => op.describe(),
            AnyOperation::Select(op) => op.describe(),
            AnyOperation::SetConfig(op) => op.describe(),
            AnyOperation::SetName(op) => op.describe(),
            AnyOperation::SetDescription(op) => op.describe(),
        }
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        match self {
            AnyOperation::RemoveColumns(op) => op.check(bundle).await,
            AnyOperation::RenameColumn(op) => op.check(bundle).await,
            AnyOperation::RenameView(op) => op.check(bundle).await,
            AnyOperation::AttachBlock(op) => op.check(bundle).await,
            AnyOperation::CreateView(op) => op.check(bundle).await,
            AnyOperation::DefineFunction(op) => op.check(bundle).await,
            AnyOperation::Filter(op) => op.check(bundle).await,
            AnyOperation::IndexBlocks(op) => op.check(bundle).await,
            AnyOperation::DefineIndex(op) => op.check(bundle).await,
            AnyOperation::DefinePack(op) => op.check(bundle).await,
            AnyOperation::DropIndex(op) => op.check(bundle).await,
            AnyOperation::DropView(op) => op.check(bundle).await,
            AnyOperation::RebuildIndex(op) => op.check(bundle).await,
            AnyOperation::Join(op) => op.check(bundle).await,
            AnyOperation::Select(op) => op.check(bundle).await,
            AnyOperation::SetConfig(op) => op.check(bundle).await,
            AnyOperation::SetName(op) => op.check(bundle).await,
            AnyOperation::SetDescription(op) => op.check(bundle).await,
        }
    }

    async fn apply(&self, bundle: &mut Bundle) -> Result<(), DataFusionError> {
        match self {
            AnyOperation::RemoveColumns(op) => op.apply(bundle).await,
            AnyOperation::RenameColumn(op) => op.apply(bundle).await,
            AnyOperation::RenameView(op) => op.apply(bundle).await,
            AnyOperation::AttachBlock(op) => op.apply(bundle).await,
            AnyOperation::CreateView(op) => op.apply(bundle).await,
            AnyOperation::DefineFunction(op) => op.apply(bundle).await,
            AnyOperation::Filter(op) => op.apply(bundle).await,
            AnyOperation::IndexBlocks(op) => op.apply(bundle).await,
            AnyOperation::DefineIndex(op) => op.apply(bundle).await,
            AnyOperation::DefinePack(op) => op.apply(bundle).await,
            AnyOperation::DropIndex(op) => op.apply(bundle).await,
            AnyOperation::DropView(op) => op.apply(bundle).await,
            AnyOperation::RebuildIndex(op) => op.apply(bundle).await,
            AnyOperation::Join(op) => op.apply(bundle).await,
            AnyOperation::Select(op) => op.apply(bundle).await,
            AnyOperation::SetConfig(op) => op.apply(bundle).await,
            AnyOperation::SetName(op) => op.apply(bundle).await,
            AnyOperation::SetDescription(op) => op.apply(bundle).await,
        }
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        ctx: Arc<SessionContext>,
    ) -> Result<DataFrame, BundlebaseError> {
        match self {
            AnyOperation::RemoveColumns(op) => op.apply_dataframe(df, ctx).await,
            AnyOperation::RenameColumn(op) => op.apply_dataframe(df, ctx).await,
            AnyOperation::RenameView(op) => op.apply_dataframe(df, ctx).await,
            AnyOperation::AttachBlock(op) => op.apply_dataframe(df, ctx).await,
            AnyOperation::CreateView(op) => op.apply_dataframe(df, ctx).await,
            AnyOperation::DefineFunction(op) => op.apply_dataframe(df, ctx).await,
            AnyOperation::Filter(op) => op.apply_dataframe(df, ctx).await,
            AnyOperation::IndexBlocks(op) => op.apply_dataframe(df, ctx).await,
            AnyOperation::DefineIndex(op) => op.apply_dataframe(df, ctx).await,
            AnyOperation::DefinePack(op) => op.apply_dataframe(df, ctx).await,
            AnyOperation::DropIndex(op) => op.apply_dataframe(df, ctx).await,
            AnyOperation::DropView(op) => op.apply_dataframe(df, ctx).await,
            AnyOperation::RebuildIndex(op) => op.apply_dataframe(df, ctx).await,
            AnyOperation::Join(op) => op.apply_dataframe(df, ctx).await,
            AnyOperation::Select(op) => op.apply_dataframe(df, ctx).await,
            AnyOperation::SetConfig(op) => op.apply_dataframe(df, ctx).await,
            AnyOperation::SetName(op) => op.apply_dataframe(df, ctx).await,
            AnyOperation::SetDescription(op) => op.apply_dataframe(df, ctx).await,
        }
    }

    fn version(&self) -> String {
        match self {
            AnyOperation::RemoveColumns(op) => op.version(),
            AnyOperation::RenameColumn(op) => op.version(),
            AnyOperation::RenameView(op) => op.version(),
            AnyOperation::AttachBlock(op) => op.version(),
            AnyOperation::CreateView(op) => op.version(),
            AnyOperation::DefineFunction(op) => op.version(),
            AnyOperation::Filter(op) => op.version(),
            AnyOperation::IndexBlocks(op) => op.version(),
            AnyOperation::DefineIndex(op) => op.version(),
            AnyOperation::DefinePack(op) => op.version(),
            AnyOperation::DropIndex(op) => op.version(),
            AnyOperation::DropView(op) => op.version(),
            AnyOperation::RebuildIndex(op) => op.version(),
            AnyOperation::Join(op) => op.version(),
            AnyOperation::Select(op) => op.version(),
            AnyOperation::SetConfig(op) => op.version(),
            AnyOperation::SetName(op) => op.version(),
            AnyOperation::SetDescription(op) => op.version(),
        }
    }

    fn allowed_on_view(&self) -> bool {
        match self {
            AnyOperation::RemoveColumns(op) => op.allowed_on_view(),
            AnyOperation::RenameColumn(op) => op.allowed_on_view(),
            AnyOperation::RenameView(op) => op.allowed_on_view(),
            AnyOperation::AttachBlock(op) => op.allowed_on_view(),
            AnyOperation::CreateView(op) => op.allowed_on_view(),
            AnyOperation::DefineFunction(op) => op.allowed_on_view(),
            AnyOperation::Filter(op) => op.allowed_on_view(),
            AnyOperation::IndexBlocks(op) => op.allowed_on_view(),
            AnyOperation::DefineIndex(op) => op.allowed_on_view(),
            AnyOperation::DefinePack(op) => op.allowed_on_view(),
            AnyOperation::DropIndex(op) => op.allowed_on_view(),
            AnyOperation::DropView(op) => op.allowed_on_view(),
            AnyOperation::RebuildIndex(op) => op.allowed_on_view(),
            AnyOperation::Join(op) => op.allowed_on_view(),
            AnyOperation::Select(op) => op.allowed_on_view(),
            AnyOperation::SetConfig(op) => op.allowed_on_view(),
            AnyOperation::SetName(op) => op.allowed_on_view(),
            AnyOperation::SetDescription(op) => op.allowed_on_view(),
        }
    }
}

// Into conversions for each config type
impl From<RemoveColumnsOp> for AnyOperation {
    fn from(config: RemoveColumnsOp) -> Self {
        AnyOperation::RemoveColumns(config)
    }
}

impl From<RenameColumnOp> for AnyOperation {
    fn from(config: RenameColumnOp) -> Self {
        AnyOperation::RenameColumn(config)
    }
}

impl From<RenameViewOp> for AnyOperation {
    fn from(config: RenameViewOp) -> Self {
        AnyOperation::RenameView(config)
    }
}

impl From<AttachBlockOp> for AnyOperation {
    fn from(config: AttachBlockOp) -> Self {
        AnyOperation::AttachBlock(config)
    }
}

impl From<CreateViewOp> for AnyOperation {
    fn from(config: CreateViewOp) -> Self {
        AnyOperation::CreateView(config)
    }
}

impl From<DefineFunctionOp> for AnyOperation {
    fn from(config: DefineFunctionOp) -> Self {
        AnyOperation::DefineFunction(config)
    }
}

impl From<FilterOp> for AnyOperation {
    fn from(config: FilterOp) -> Self {
        AnyOperation::Filter(config)
    }
}

impl From<IndexBlocksOp> for AnyOperation {
    fn from(config: IndexBlocksOp) -> Self {
        AnyOperation::IndexBlocks(config)
    }
}

impl From<JoinOp> for AnyOperation {
    fn from(config: JoinOp) -> Self {
        AnyOperation::Join(config)
    }
}

impl From<SelectOp> for AnyOperation {
    fn from(config: SelectOp) -> Self {
        AnyOperation::Select(config)
    }
}

impl From<SetNameOp> for AnyOperation {
    fn from(config: SetNameOp) -> Self {
        AnyOperation::SetName(config)
    }
}

impl From<SetDescriptionOp> for AnyOperation {
    fn from(config: SetDescriptionOp) -> Self {
        AnyOperation::SetDescription(config)
    }
}

impl From<DefineIndexOp> for AnyOperation {
    fn from(config: DefineIndexOp) -> Self {
        AnyOperation::DefineIndex(config)
    }
}

impl From<DefinePackOp> for AnyOperation {
    fn from(config: DefinePackOp) -> Self {
        AnyOperation::DefinePack(config)
    }
}

impl From<DropIndexOp> for AnyOperation {
    fn from(config: DropIndexOp) -> Self {
        AnyOperation::DropIndex(config)
    }
}

impl From<DropViewOp> for AnyOperation {
    fn from(config: DropViewOp) -> Self {
        AnyOperation::DropView(config)
    }
}

impl From<RebuildIndexOp> for AnyOperation {
    fn from(config: RebuildIndexOp) -> Self {
        AnyOperation::RebuildIndex(config)
    }
}

impl From<SetConfigOp> for AnyOperation {
    fn from(config: SetConfigOp) -> Self {
        AnyOperation::SetConfig(config)
    }
}

impl Display for AnyOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.describe())
    }
}
