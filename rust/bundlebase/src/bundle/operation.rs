mod attach_block;
mod create_view;
mod create_function;
mod create_index;
mod create_join;
mod create_source;
mod detach_block;
mod drop_index;
mod drop_join;
mod drop_view;
mod replace_block;
mod filter;
mod index_blocks;
mod drop_column;
mod parameter_value;
mod rebuild_index;
mod rename_column;
mod rename_join;
mod rename_view;
mod serde_util;
mod set_config;
mod set_description;
mod set_name;
mod update_version;

pub use crate::bundle::operation::attach_block::{AttachBlockOp, SourceInfo};
pub use crate::bundle::operation::create_view::CreateViewOp;
pub use crate::bundle::operation::create_function::CreateFunctionOp;
pub use crate::bundle::operation::create_index::CreateIndexOp;
pub use crate::bundle::operation::create_join::CreateJoinOp;
pub use crate::bundle::operation::create_source::CreateSourceOp;
pub use crate::bundle::operation::detach_block::DetachBlockOp;
pub use crate::bundle::operation::drop_index::DropIndexOp;
pub use crate::bundle::operation::drop_join::DropJoinOp;
pub use crate::bundle::operation::drop_view::DropViewOp;
pub use crate::bundle::operation::filter::FilterOp;
pub use crate::bundle::operation::replace_block::ReplaceBlockOp;
pub use crate::bundle::operation::index_blocks::IndexBlocksOp;
pub use crate::bundle::operation::drop_column::DropColumnOp;
pub use crate::bundle::operation::rebuild_index::RebuildIndexOp;
pub use crate::bundle::operation::rename_column::RenameColumnOp;
pub use crate::bundle::operation::rename_join::RenameJoinOp;
pub use crate::bundle::operation::rename_view::RenameViewOp;
pub use crate::bundle::operation::set_config::SetConfigOp;
pub use crate::bundle::operation::set_description::SetDescriptionOp;
pub use crate::bundle::operation::set_name::SetNameOp;
pub use crate::bundle::operation::update_version::UpdateVersionOp;
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

    /// Apply this operation to the bundle using interior mutability.
    /// For example, this can be used to set the bundle name.
    /// The default implementation does nothing.
    /// TODO: should return the result object, even if it's just a message
    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError>;

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

        #[async_trait]
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
            ) -> Result<DataFrame, BundlebaseError> {
                match self {
                    $( AnyOperation::$variant(op) => op.apply_dataframe(df, ctx).await, )*
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
    AttachBlock(AttachBlockOp),
    CreateFunction(CreateFunctionOp),
    CreateIndex(CreateIndexOp),
    CreateJoin(CreateJoinOp),
    CreateSource(CreateSourceOp),
    CreateView(CreateViewOp),
    DetachBlock(DetachBlockOp),
    DropColumn(DropColumnOp),
    DropIndex(DropIndexOp),
    DropJoin(DropJoinOp),
    DropView(DropViewOp),
    Filter(FilterOp),
    IndexBlocks(IndexBlocksOp),
    RebuildIndex(RebuildIndexOp),
    RenameColumn(RenameColumnOp),
    RenameJoin(RenameJoinOp),
    RenameView(RenameViewOp),
    ReplaceBlock(ReplaceBlockOp),
    SetConfig(SetConfigOp),
    SetDescription(SetDescriptionOp),
    SetName(SetNameOp),
    UpdateVersion(UpdateVersionOp),
}

impl Display for AnyOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.describe())
    }
}
