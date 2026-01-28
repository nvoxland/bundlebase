use ::bundlebase::bundle::AnyOperation;
use ::bundlebase::Operation;
use pyo3::prelude::*;

#[pyclass]
#[derive(Clone)]
pub struct PyOperation {
    inner: AnyOperation,
}

#[pymethods]
impl PyOperation {
    #[getter]
    fn op_type(&self) -> String {
        match &self.inner {
            AnyOperation::AttachBlock(_) => "attachBlock".to_string(),
            AnyOperation::CreateView(_) => "CreateView".to_string(),
            AnyOperation::DropColumn(_) => "dropColumn".to_string(),
            AnyOperation::RenameColumn(_) => "renameColumn".to_string(),
            AnyOperation::RenameJoin(_) => "renameJoin".to_string(),
            AnyOperation::RenameView(_) => "renameView".to_string(),
            AnyOperation::Filter(_) => "filter".to_string(),
            AnyOperation::CreateFunction(_) => "createFunction".to_string(),
            AnyOperation::SetConfig(_) => "setConfig".to_string(),
            AnyOperation::SetName(_) => "setName".to_string(),
            AnyOperation::SetDescription(_) => "setDescription".to_string(),
            AnyOperation::IndexBlocks(_) => "indexBlocks".to_string(),
            AnyOperation::CreateIndex(_) => "createIndex".to_string(),
            AnyOperation::CreateJoin(_) => "createJoin".to_string(),
            AnyOperation::DropIndex(_) => "dropIndex".to_string(),
            AnyOperation::DropJoin(_) => "dropJoin".to_string(),
            AnyOperation::DropView(_) => "dropView".to_string(),
            AnyOperation::RebuildIndex(_) => "rebuildIndex".to_string(),
            AnyOperation::CreateSource(_) => "createSource".to_string(),
            AnyOperation::DetachBlock(_) => "detachBlock".to_string(),
            AnyOperation::ReplaceBlock(_) => "replaceBlock".to_string(),
            AnyOperation::UpdateVersion(_) => "updateVersion".to_string(),
        }
    }

    #[getter]
    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn __repr__(&self) -> String {
        format!("PyOperation({})", self.op_type())
    }

    fn __str__(&self) -> String {
        self.describe()
    }
}

impl PyOperation {
    pub fn new(inner: AnyOperation) -> Self {
        PyOperation { inner }
    }
}
