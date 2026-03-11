//! DataFusion ScalarUDFImpl for scalar functions.
//!
//! Bridges `FunctionEntry` definitions to DataFusion's UDF system.
//! Dispatches by runner: Python (via PyArrow), IPC/Java/Docker (via Arrow IPC),
//! Lib (via FFI).

use crate::bundle::connector_definition::Runner;
use crate::bundle::function_definition::FunctionEntry;
use crate::NamespacedName;
use crate::function::ipc_bridge::{invoke_ipc_scalar, SubprocessCache};
use crate::function::lib_bridge::{invoke_lib_scalar, parse_lib_logic};
use crate::function::parse_python_logic;
use crate::function::python_bridge::get_python_function_bridge;
use crate::BundlebaseError;
use arrow::array::ArrayRef;
use arrow::datatypes::DataType;
use datafusion::logical_expr::{
    ColumnarValue, ScalarUDFImpl, Signature, TypeSignature, Volatility,
};
use std::any::Any;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Invoke a scalar function entry with the given args.
fn invoke_entry(
    name: &str,
    entry: &FunctionEntry,
    args: &datafusion::logical_expr::ScalarFunctionArgs,
    subprocess_cache: &SubprocessCache,
) -> datafusion::common::Result<ColumnarValue> {
    match entry.runner {
        Runner::Python => invoke_python(name, &entry.logic, args),
        Runner::Ipc | Runner::Java | Runner::Docker => invoke_ipc(name, entry, args, subprocess_cache),
        Runner::Lib => invoke_lib(name, entry, args),
    }
}

/// Find the overload whose input_types match the actual argument types.
fn find_matching_overload<'a>(
    overloads: &'a [FunctionEntry],
    arg_types: &[DataType],
) -> Option<&'a FunctionEntry> {
    overloads.iter().find(|entry| entry.input_types == arg_types)
}

/// A DataFusion scalar function backed by one or more FunctionEntry overloads.
///
/// When there is a single overload, behaves identically to the old single-entry approach.
/// With multiple overloads, uses `TypeSignature::OneOf` and dispatches by matching
/// argument types at invocation time.
#[derive(Debug)]
pub struct ScalarFunction {
    name: String,
    signature: Signature,
    overloads: Vec<FunctionEntry>,
    subprocess_cache: SubprocessCache,
}

impl PartialEq for ScalarFunction {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.overloads == other.overloads
    }
}

impl Eq for ScalarFunction {}

impl Hash for ScalarFunction {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

impl ScalarFunction {
    /// Create a new scalar function from a single FunctionEntry.
    pub fn new(entry: FunctionEntry, subprocess_cache: SubprocessCache) -> Result<Self, BundlebaseError> {
        let name = entry.name.to_string();
        let signature = Signature::new(
            TypeSignature::Exact(entry.input_types.clone()),
            Volatility::Volatile,
        );
        Ok(Self {
            name,
            signature,
            overloads: vec![entry],
            subprocess_cache,
        })
    }

    /// Create a composite scalar function from multiple overloads.
    ///
    /// All entries must share the same name and be scalar kind.
    /// Uses `TypeSignature::OneOf` to advertise all accepted signatures.
    pub fn new_composite(overloads: Vec<FunctionEntry>, subprocess_cache: SubprocessCache) -> Result<Self, BundlebaseError> {
        if overloads.is_empty() {
            return Err("Cannot create composite scalar function with no overloads".into());
        }
        let name = overloads[0].name.to_string();
        let type_sigs: Vec<TypeSignature> = overloads
            .iter()
            .map(|e| TypeSignature::Exact(e.input_types.clone()))
            .collect();
        let signature = if type_sigs.len() == 1 {
            Signature::new(type_sigs.into_iter().next().expect("checked non-empty"), Volatility::Volatile)
        } else {
            Signature::new(TypeSignature::OneOf(type_sigs), Volatility::Volatile)
        };
        Ok(Self {
            name,
            signature,
            overloads,
            subprocess_cache,
        })
    }
}

impl ScalarUDFImpl for ScalarFunction {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, arg_types: &[DataType]) -> datafusion::common::Result<DataType> {
        if self.overloads.len() == 1 {
            return Ok(self.overloads[0].return_type.clone());
        }
        // Find matching overload by arg types
        match find_matching_overload(&self.overloads, arg_types) {
            Some(entry) => Ok(entry.return_type.clone()),
            None => {
                let available: Vec<String> = self.overloads.iter()
                    .map(|e| format!("({}) -> {}", e.input_types.iter().map(|t| format!("{}", t)).collect::<Vec<_>>().join(", "), e.return_type))
                    .collect();
                Err(datafusion::common::DataFusionError::Plan(format!(
                    "No overload of function '{}' matches argument types {:?}. Available signatures: {}",
                    self.name, arg_types, available.join("; ")
                )))
            }
        }
    }

    fn invoke_with_args(
        &self,
        args: datafusion::logical_expr::ScalarFunctionArgs,
    ) -> datafusion::common::Result<ColumnarValue> {
        if self.overloads.len() == 1 {
            return invoke_entry(&self.name, &self.overloads[0], &args, &self.subprocess_cache);
        }
        // Determine actual arg types for dispatch
        let arg_types: Vec<DataType> = args.args.iter().map(|cv| cv.data_type()).collect();
        let entry = find_matching_overload(&self.overloads, &arg_types)
            .ok_or_else(|| {
                let available: Vec<String> = self.overloads.iter()
                    .map(|e| format!("({}) -> {}", e.input_types.iter().map(|t| format!("{}", t)).collect::<Vec<_>>().join(", "), e.return_type))
                    .collect();
                datafusion::common::DataFusionError::Execution(format!(
                    "No overload of function '{}' matches argument types {:?}. Available signatures: {}",
                    self.name, arg_types, available.join("; ")
                ))
            })?;
        invoke_entry(&self.name, entry, &args, &self.subprocess_cache)
    }

}

/// Invoke a Python function via the registered bridge.
fn invoke_python(
    name: &str,
    logic: &str,
    args: &datafusion::logical_expr::ScalarFunctionArgs,
) -> datafusion::common::Result<ColumnarValue> {
    let bridge = get_python_function_bridge().map_err(|e| {
        datafusion::common::DataFusionError::Execution(format!(
            "Cannot invoke Python function '{}': {}",
            name, e
        ))
    })?;

    let (module, function) = parse_python_logic(logic)?;

    // Convert ColumnarValue args to Arrow arrays
    let arrays: Vec<ArrayRef> = args
        .args
        .iter()
        .map(|cv| match cv {
            ColumnarValue::Array(arr) => Ok(Arc::clone(arr)),
            ColumnarValue::Scalar(scalar) => scalar
                .to_array_of_size(args.number_rows)
                .map_err(|e| datafusion::common::DataFusionError::Execution(e.to_string())),
        })
        .collect::<datafusion::common::Result<Vec<_>>>()?;

    let result = bridge
        .invoke(module, function, &arrays, args.number_rows)
        .map_err(|e| {
            datafusion::common::DataFusionError::Execution(format!(
                "Python function '{}' ({}:{}) failed: {}",
                name, module, function, e
            ))
        })?;

    Ok(ColumnarValue::Array(result))
}

/// Invoke an IPC/Java/Docker function via the subprocess bridge.
fn invoke_ipc(
    name: &str,
    entry: &FunctionEntry,
    args: &datafusion::logical_expr::ScalarFunctionArgs,
    subprocess_cache: &SubprocessCache,
) -> datafusion::common::Result<ColumnarValue> {
    // Convert ColumnarValue args to Arrow arrays
    let arrays: Vec<ArrayRef> = args
        .args
        .iter()
        .map(|cv| match cv {
            ColumnarValue::Array(arr) => Ok(Arc::clone(arr)),
            ColumnarValue::Scalar(scalar) => scalar
                .to_array_of_size(args.number_rows)
                .map_err(|e| datafusion::common::DataFusionError::Execution(e.to_string())),
        })
        .collect::<datafusion::common::Result<Vec<_>>>()?;

    let result = invoke_ipc_scalar(subprocess_cache, &entry.logic, &entry.name.name, &arrays).map_err(|e| {
        datafusion::common::DataFusionError::Execution(format!(
            "IPC function '{}' ({}) failed: {}",
            name, entry.logic, e
        ))
    })?;

    Ok(ColumnarValue::Array(result))
}

/// Invoke a native lib function via the FFI bridge.
fn invoke_lib(
    name: &str,
    entry: &FunctionEntry,
    args: &datafusion::logical_expr::ScalarFunctionArgs,
) -> datafusion::common::Result<ColumnarValue> {
    let (lib_path, symbol_opt) = parse_lib_logic(&entry.logic).map_err(|e| {
        datafusion::common::DataFusionError::Execution(format!(
            "Invalid lib logic for function '{}': {}",
            name, e
        ))
    })?;

    // Default symbol to the short name from NamespacedName
    let symbol = symbol_opt.unwrap_or(&entry.name.name);

    // Convert ColumnarValue args to Arrow arrays
    let arrays: Vec<ArrayRef> = args
        .args
        .iter()
        .map(|cv| match cv {
            ColumnarValue::Array(arr) => Ok(Arc::clone(arr)),
            ColumnarValue::Scalar(scalar) => scalar
                .to_array_of_size(args.number_rows)
                .map_err(|e| datafusion::common::DataFusionError::Execution(e.to_string())),
        })
        .collect::<datafusion::common::Result<Vec<_>>>()?;

    let result = invoke_lib_scalar(lib_path, symbol, &arrays).map_err(|e| {
        datafusion::common::DataFusionError::Execution(format!(
            "Lib function '{}' ({}:{}) failed: {}",
            name, lib_path, symbol, e
        ))
    })?;

    Ok(ColumnarValue::Array(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::connector_definition::Platform;
    use crate::bundle::function_definition::FunctionKind;
    use crate::data::ObjectId;
    use crate::function::ipc_bridge::new_subprocess_cache;

    #[test]
    fn test_signature_construction() {
        let entry = FunctionEntry {
            id: ObjectId::generate(),
            name: NamespacedName::new("acme", "double_val"),
            input_types: vec![DataType::Int64],
            return_type: DataType::Int64,
            runner: Runner::Ipc,
            logic: "./my_func".to_string(),
            platform: Platform::any(),
            temporary: false,
            kind: FunctionKind::Scalar,
        };

        let func = ScalarFunction::new(entry, new_subprocess_cache()).expect("should create");
        assert_eq!(func.name(), "acme.double_val");
        assert_eq!(
            func.return_type(&[DataType::Int64]).unwrap(),
            DataType::Int64
        );
    }

    #[test]
    fn test_signature_multi_arg() {
        let entry = FunctionEntry {
            id: ObjectId::generate(),
            name: NamespacedName::new("acme", "add"),
            input_types: vec![DataType::Int64, DataType::Int64],
            return_type: DataType::Int64,
            runner: Runner::Ipc,
            logic: "./add_func".to_string(),
            platform: Platform::any(),
            temporary: false,
            kind: FunctionKind::Scalar,
        };

        let func = ScalarFunction::new(entry, new_subprocess_cache()).expect("should create");
        assert_eq!(func.name(), "acme.add");
    }

    #[test]
    fn test_parse_python_logic_valid() {
        let (module, func) = parse_python_logic("my_module:my_function").unwrap();
        assert_eq!(module, "my_module");
        assert_eq!(func, "my_function");
    }

    #[test]
    fn test_parse_python_logic_dotted_module() {
        let (module, func) = parse_python_logic("pkg.subpkg.mod:func_name").unwrap();
        assert_eq!(module, "pkg.subpkg.mod");
        assert_eq!(func, "func_name");
    }

    #[test]
    fn test_parse_python_logic_invalid() {
        assert!(parse_python_logic("no_colon").is_err());
        assert!(parse_python_logic(":no_module").is_err());
        assert!(parse_python_logic("no_func:").is_err());
    }

    #[test]
    fn test_composite_empty_overloads() {
        let result = ScalarFunction::new_composite(vec![], new_subprocess_cache());
        assert!(result.is_err());
    }

    #[test]
    fn test_composite_single_overload() {
        let entry = FunctionEntry {
            id: ObjectId::generate(),
            name: NamespacedName::new("acme", "double_val"),
            input_types: vec![DataType::Int64],
            return_type: DataType::Int64,
            runner: Runner::Ipc,
            logic: "./my_func".to_string(),
            platform: Platform::any(),
            temporary: false,
            kind: FunctionKind::Scalar,
        };
        let func = ScalarFunction::new_composite(vec![entry], new_subprocess_cache()).expect("should create");
        assert_eq!(func.name(), "acme.double_val");
        assert_eq!(func.return_type(&[DataType::Int64]).unwrap(), DataType::Int64);
    }

    #[test]
    fn test_composite_multiple_overloads() {
        let int_entry = FunctionEntry {
            id: ObjectId::generate(),
            name: NamespacedName::new("acme", "convert"),
            input_types: vec![DataType::Int64],
            return_type: DataType::Utf8,
            runner: Runner::Ipc,
            logic: "./int_convert".to_string(),
            platform: Platform::any(),
            temporary: false,
            kind: FunctionKind::Scalar,
        };
        let float_entry = FunctionEntry {
            id: ObjectId::generate(),
            name: NamespacedName::new("acme", "convert"),
            input_types: vec![DataType::Float64],
            return_type: DataType::Utf8,
            runner: Runner::Ipc,
            logic: "./float_convert".to_string(),
            platform: Platform::any(),
            temporary: false,
            kind: FunctionKind::Scalar,
        };
        let func = ScalarFunction::new_composite(vec![int_entry, float_entry], new_subprocess_cache()).expect("should create");
        assert_eq!(func.name(), "acme.convert");
        // return_type should dispatch based on arg types
        assert_eq!(func.return_type(&[DataType::Int64]).unwrap(), DataType::Utf8);
        assert_eq!(func.return_type(&[DataType::Float64]).unwrap(), DataType::Utf8);
        // Non-matching should error
        assert!(func.return_type(&[DataType::Boolean]).is_err());
    }

    #[test]
    fn test_composite_different_return_types() {
        let int_entry = FunctionEntry {
            id: ObjectId::generate(),
            name: NamespacedName::new("acme", "transform"),
            input_types: vec![DataType::Int64],
            return_type: DataType::Int64,
            runner: Runner::Ipc,
            logic: "./int_transform".to_string(),
            platform: Platform::any(),
            temporary: false,
            kind: FunctionKind::Scalar,
        };
        let str_entry = FunctionEntry {
            id: ObjectId::generate(),
            name: NamespacedName::new("acme", "transform"),
            input_types: vec![DataType::Utf8],
            return_type: DataType::Utf8,
            runner: Runner::Ipc,
            logic: "./str_transform".to_string(),
            platform: Platform::any(),
            temporary: false,
            kind: FunctionKind::Scalar,
        };
        let func = ScalarFunction::new_composite(vec![int_entry, str_entry], new_subprocess_cache()).expect("should create");
        assert_eq!(func.return_type(&[DataType::Int64]).unwrap(), DataType::Int64);
        assert_eq!(func.return_type(&[DataType::Utf8]).unwrap(), DataType::Utf8);
    }
}
