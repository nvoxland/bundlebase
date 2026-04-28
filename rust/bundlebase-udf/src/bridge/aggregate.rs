//! DataFusion AggregateUDFImpl for aggregate functions.
//!
//! Bridges `FunctionEntry` definitions (with `kind == Aggregate`) to DataFusion's UDAF system.
//! Dispatches to Python via the `PythonFunctionBridge` aggregate methods.

use crate::bridge::ipc_bridge::{self, SubprocessCache};
use crate::bridge::python_bridge::get_python_function_bridge;
use crate::function_entry::FunctionEntry;
use arrow::datatypes::DataType;
use bundlebase_common::BundlebaseError;
use datafusion::common::Result as DFResult;
use datafusion::logical_expr::{
    Accumulator, AggregateUDFImpl, Signature, TypeSignature, Volatility,
};
use datafusion::scalar::ScalarValue;
use std::any::Any;
use std::fmt::Debug;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Create an accumulator for a single function entry.
fn create_accumulator_for_entry(
    name: &str,
    entry: &FunctionEntry,
    subprocess_cache: &SubprocessCache,
) -> DFResult<Box<dyn Accumulator>> {
    entry
        .from
        .create_accumulator(name, &entry.name.name, &entry.return_type, subprocess_cache)
}

/// Find the overload whose input_types match the actual argument types.
fn find_matching_overload<'a>(
    overloads: &'a [FunctionEntry],
    arg_types: &[DataType],
) -> Option<&'a FunctionEntry> {
    overloads
        .iter()
        .find(|entry| entry.input_types == arg_types)
}

/// A DataFusion aggregate function backed by one or more FunctionEntry overloads.
#[derive(Debug)]
pub struct AggregateFunction {
    name: String,
    signature: Signature,
    overloads: Vec<FunctionEntry>,
    subprocess_cache: SubprocessCache,
}

impl PartialEq for AggregateFunction {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.overloads == other.overloads
    }
}

impl Eq for AggregateFunction {}

impl Hash for AggregateFunction {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

impl AggregateFunction {
    /// Create a new aggregate function from a single FunctionEntry.
    pub fn new(
        entry: FunctionEntry,
        subprocess_cache: SubprocessCache,
    ) -> Result<Self, BundlebaseError> {
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

    /// Create a composite aggregate function from multiple overloads.
    pub fn new_composite(
        overloads: Vec<FunctionEntry>,
        subprocess_cache: SubprocessCache,
    ) -> Result<Self, BundlebaseError> {
        if overloads.is_empty() {
            return Err("Cannot create composite aggregate function with no overloads".into());
        }
        let name = overloads[0].name.to_string();
        let type_sigs: Vec<TypeSignature> = overloads
            .iter()
            .map(|e| TypeSignature::Exact(e.input_types.clone()))
            .collect();
        let signature = if type_sigs.len() == 1 {
            Signature::new(
                type_sigs.into_iter().next().expect("checked non-empty"),
                Volatility::Volatile,
            )
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

    /// Replace the DataFusion-visible name. Mirrors `ScalarFunction::with_name`.
    pub fn with_name(mut self, name: String) -> Self {
        self.name = name;
        self
    }
}

impl AggregateUDFImpl for AggregateFunction {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, arg_types: &[DataType]) -> DFResult<DataType> {
        if self.overloads.len() == 1 {
            return Ok(self.overloads[0].return_type.clone());
        }
        match find_matching_overload(&self.overloads, arg_types) {
            Some(entry) => Ok(entry.return_type.clone()),
            None => Err(datafusion::common::DataFusionError::Plan(format!(
                "No overload of aggregate '{}' matches argument types {:?}",
                self.name, arg_types
            ))),
        }
    }

    fn state_fields(
        &self,
        args: datafusion::logical_expr::function::StateFieldsArgs,
    ) -> DFResult<Vec<Arc<arrow::datatypes::Field>>> {
        // Find matching overload for state type
        let entry = if self.overloads.len() == 1 {
            &self.overloads[0]
        } else {
            let arg_types: Vec<DataType> = args
                .input_fields
                .iter()
                .map(|f| f.data_type().clone())
                .collect();
            find_matching_overload(&self.overloads, &arg_types).ok_or_else(|| {
                datafusion::common::DataFusionError::Plan(format!(
                    "No overload of aggregate '{}' matches argument types {:?}",
                    self.name, arg_types
                ))
            })?
        };

        // IPC accumulators use Utf8 state (opaque state ID), others use return type
        let state_type = entry.from.aggregate_state_type(&entry.return_type);

        Ok(vec![Arc::new(arrow::datatypes::Field::new(
            "state", state_type, true,
        ))])
    }

    fn accumulator(
        &self,
        acc_args: datafusion::logical_expr::function::AccumulatorArgs,
    ) -> DFResult<Box<dyn Accumulator>> {
        let entry = if self.overloads.len() == 1 {
            &self.overloads[0]
        } else {
            let arg_types: Vec<DataType> = acc_args
                .expr_fields
                .iter()
                .map(|f| f.data_type().clone())
                .collect();
            find_matching_overload(&self.overloads, &arg_types).ok_or_else(|| {
                datafusion::common::DataFusionError::Execution(format!(
                    "No overload of aggregate '{}' matches argument types {:?}",
                    self.name, arg_types
                ))
            })?
        };
        create_accumulator_for_entry(&self.name, entry, &self.subprocess_cache)
    }
}

/// Accumulator that delegates to Python aggregate class methods.
#[derive(Debug)]
pub(crate) struct PythonAccumulator {
    pub(crate) module: String,
    pub(crate) class_name: String,
    pub(crate) state: ScalarValue,
    pub(crate) function_name: String,
}

impl Accumulator for PythonAccumulator {
    fn update_batch(&mut self, values: &[arrow::array::ArrayRef]) -> DFResult<()> {
        let bridge = get_python_function_bridge().map_err(|e| {
            datafusion::common::DataFusionError::Execution(format!(
                "Cannot invoke accumulate for '{}': {}",
                self.function_name, e
            ))
        })?;

        self.state = bridge
            .aggregate_accumulate(&self.module, &self.class_name, &self.state, values)
            .map_err(|e| {
                datafusion::common::DataFusionError::Execution(format!(
                    "Python accumulate for '{}' failed: {}",
                    self.function_name, e
                ))
            })?;

        Ok(())
    }

    fn merge_batch(&mut self, states: &[arrow::array::ArrayRef]) -> DFResult<()> {
        let bridge = get_python_function_bridge().map_err(|e| {
            datafusion::common::DataFusionError::Execution(format!(
                "Cannot invoke merge for '{}': {}",
                self.function_name, e
            ))
        })?;

        // states is a slice of arrays, one per state field. We have one state field.
        if states.is_empty() {
            return Ok(());
        }

        let state_array = &states[0];
        for i in 0..state_array.len() {
            let other_state = ScalarValue::try_from_array(state_array, i)?;
            self.state = bridge
                .aggregate_merge(&self.module, &self.class_name, &self.state, &other_state)
                .map_err(|e| {
                    datafusion::common::DataFusionError::Execution(format!(
                        "Python merge for '{}' failed: {}",
                        self.function_name, e
                    ))
                })?;
        }

        Ok(())
    }

    fn evaluate(&mut self) -> DFResult<ScalarValue> {
        let bridge = get_python_function_bridge().map_err(|e| {
            datafusion::common::DataFusionError::Execution(format!(
                "Cannot invoke evaluate for '{}': {}",
                self.function_name, e
            ))
        })?;

        bridge
            .aggregate_evaluate(&self.module, &self.class_name, &self.state)
            .map_err(|e| {
                datafusion::common::DataFusionError::Execution(format!(
                    "Python evaluate for '{}' failed: {}",
                    self.function_name, e
                ))
            })
    }

    fn state(&mut self) -> DFResult<Vec<ScalarValue>> {
        Ok(vec![self.state.clone()])
    }

    fn size(&self) -> usize {
        std::mem::size_of_val(self) + self.module.len() + self.class_name.len()
    }
}

/// Accumulator that delegates to an IPC subprocess via JSON-RPC + Arrow IPC.
///
/// State is held server-side in the subprocess; the accumulator only holds an opaque state ID.
#[derive(Debug)]
pub(crate) struct IpcAccumulator {
    pub(crate) entrypoint: String,
    pub(crate) function_name: String,
    pub(crate) display_name: String,
    pub(crate) state_id: String,
    pub(crate) return_type: DataType,
    pub(crate) subprocess_cache: SubprocessCache,
}

impl Accumulator for IpcAccumulator {
    fn update_batch(&mut self, values: &[arrow::array::ArrayRef]) -> DFResult<()> {
        ipc_bridge::ipc_aggregate_accumulate(
            &self.subprocess_cache,
            &self.entrypoint,
            &self.function_name,
            &self.state_id,
            values,
        )
        .map_err(|e| {
            datafusion::common::DataFusionError::Execution(format!(
                "IPC accumulate for '{}' failed: {}",
                self.display_name, e
            ))
        })
    }

    fn merge_batch(&mut self, states: &[arrow::array::ArrayRef]) -> DFResult<()> {
        if states.is_empty() {
            return Ok(());
        }

        // Each element in the state array is an opaque state ID (as Utf8).
        // We need to merge them one by one into our current state.
        let state_array = &states[0];
        for i in 0..state_array.len() {
            let other_state = ScalarValue::try_from_array(state_array, i)?;
            if let ScalarValue::Utf8(Some(other_id)) = &other_state {
                let merged_id = ipc_bridge::ipc_aggregate_merge(
                    &self.subprocess_cache,
                    &self.entrypoint,
                    &self.function_name,
                    &self.state_id,
                    other_id,
                )
                .map_err(|e| {
                    datafusion::common::DataFusionError::Execution(format!(
                        "IPC merge for '{}' failed: {}",
                        self.display_name, e
                    ))
                })?;
                self.state_id = merged_id;
            }
        }

        Ok(())
    }

    fn evaluate(&mut self) -> DFResult<ScalarValue> {
        ipc_bridge::ipc_aggregate_evaluate(
            &self.subprocess_cache,
            &self.entrypoint,
            &self.function_name,
            &self.state_id,
            &self.return_type,
        )
        .map_err(|e| {
            datafusion::common::DataFusionError::Execution(format!(
                "IPC evaluate for '{}' failed: {}",
                self.display_name, e
            ))
        })
    }

    fn state(&mut self) -> DFResult<Vec<ScalarValue>> {
        // Return the state ID as a Utf8 scalar so DataFusion can serialize/merge it
        Ok(vec![ScalarValue::Utf8(Some(self.state_id.clone()))])
    }

    fn size(&self) -> usize {
        std::mem::size_of_val(self)
            + self.entrypoint.len()
            + self.function_name.len()
            + self.display_name.len()
            + self.state_id.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::ipc_bridge::new_subprocess_cache;
    use crate::function_entry::FunctionKind;
    use crate::parse_python_entrypoint;
    use crate::runtime::UdfRuntime;
    use bundlebase_common::namespaced_name::NamespacedName;
    use bundlebase_common::object_id::ObjectId;
    use bundlebase_common::platform::Platform;

    #[test]
    fn test_signature_construction() {
        let entry = FunctionEntry {
            id: ObjectId::generate(),
            name: NamespacedName::new("acme", "my_sum"),
            input_types: vec![DataType::Int64],
            return_type: DataType::Int64,
            from: UdfRuntime::parse_from("python::my_module:MySum").unwrap(),
            platform: Platform::any(),
            temporary: true,
            kind: FunctionKind::Aggregate,
        };

        let agg = AggregateFunction::new(entry, new_subprocess_cache()).expect("should create");
        assert_eq!(agg.name(), "acme.my_sum");
        assert_eq!(
            agg.return_type(&[DataType::Int64]).unwrap(),
            DataType::Int64
        );
    }

    #[test]
    fn test_parse_python_entrypoint_valid() {
        let (module, class) = parse_python_entrypoint("my_module:MySum").unwrap();
        assert_eq!(module, "my_module");
        assert_eq!(class, "MySum");
    }

    #[test]
    fn test_parse_python_entrypoint_dotted_module() {
        let (module, class) = parse_python_entrypoint("pkg.subpkg.mod:MyAgg").unwrap();
        assert_eq!(module, "pkg.subpkg.mod");
        assert_eq!(class, "MyAgg");
    }

    #[test]
    fn test_parse_python_entrypoint_invalid() {
        assert!(parse_python_entrypoint("no_colon").is_err());
        assert!(parse_python_entrypoint(":no_module").is_err());
        assert!(parse_python_entrypoint("no_func:").is_err());
    }

    #[test]
    fn test_composite_empty_overloads() {
        let result = AggregateFunction::new_composite(vec![], new_subprocess_cache());
        assert!(result.is_err());
    }

    #[test]
    fn test_composite_single_overload() {
        let entry = FunctionEntry {
            id: ObjectId::generate(),
            name: NamespacedName::new("acme", "my_sum"),
            input_types: vec![DataType::Int64],
            return_type: DataType::Int64,
            from: UdfRuntime::parse_from("python::my_module:MySum").unwrap(),
            platform: Platform::any(),
            temporary: true,
            kind: FunctionKind::Aggregate,
        };
        let agg = AggregateFunction::new_composite(vec![entry], new_subprocess_cache())
            .expect("should create");
        assert_eq!(agg.name(), "acme.my_sum");
        assert_eq!(
            agg.return_type(&[DataType::Int64]).unwrap(),
            DataType::Int64
        );
    }

    #[test]
    fn test_composite_multiple_overloads() {
        let int_entry = FunctionEntry {
            id: ObjectId::generate(),
            name: NamespacedName::new("acme", "my_sum"),
            input_types: vec![DataType::Int64],
            return_type: DataType::Int64,
            from: UdfRuntime::parse_from("python::my_module:IntSum").unwrap(),
            platform: Platform::any(),
            temporary: true,
            kind: FunctionKind::Aggregate,
        };
        let float_entry = FunctionEntry {
            id: ObjectId::generate(),
            name: NamespacedName::new("acme", "my_sum"),
            input_types: vec![DataType::Float64],
            return_type: DataType::Float64,
            from: UdfRuntime::parse_from("python::my_module:FloatSum").unwrap(),
            platform: Platform::any(),
            temporary: true,
            kind: FunctionKind::Aggregate,
        };
        let agg =
            AggregateFunction::new_composite(vec![int_entry, float_entry], new_subprocess_cache())
                .expect("should create");
        assert_eq!(agg.name(), "acme.my_sum");
        assert_eq!(
            agg.return_type(&[DataType::Int64]).unwrap(),
            DataType::Int64
        );
        assert_eq!(
            agg.return_type(&[DataType::Float64]).unwrap(),
            DataType::Float64
        );
        assert!(agg.return_type(&[DataType::Boolean]).is_err());
    }
}
