//! DataFusion AggregateUDFImpl for aggregate functions.
//!
//! Bridges `FunctionEntry` definitions (with `kind == Aggregate`) to DataFusion's UDAF system.
//! Dispatches to Python via the `PythonFunctionBridge` aggregate methods.

use crate::bundle::connector_definition::Runner;
use crate::bundle::function_definition::FunctionEntry;
use crate::function::lib_bridge::{parse_lib_logic, LibAccumulator};
use crate::function::python_bridge::get_python_function_bridge;
use crate::BundlebaseError;
use arrow::datatypes::DataType;
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
) -> DFResult<Box<dyn Accumulator>> {
    match entry.runner {
        Runner::Python => {
            let (module, class_name) = parse_python_logic(&entry.logic)?;

            let bridge = get_python_function_bridge().map_err(|e| {
                datafusion::common::DataFusionError::Execution(format!(
                    "Cannot create accumulator for '{}': {}",
                    name, e
                ))
            })?;

            let initial_state = bridge
                .aggregate_create_state(module, class_name)
                .map_err(|e| {
                    datafusion::common::DataFusionError::Execution(format!(
                        "Failed to create initial state for '{}': {}",
                        name, e
                    ))
                })?;

            Ok(Box::new(PythonAccumulator {
                module: module.to_string(),
                class_name: class_name.to_string(),
                state: initial_state,
                function_name: name.to_string(),
            }))
        }
        Runner::Lib => {
            let (lib_path, symbol_opt) = parse_lib_logic(&entry.logic).map_err(|e| {
                datafusion::common::DataFusionError::Execution(format!(
                    "Invalid lib logic for aggregate '{}': {}",
                    name, e
                ))
            })?;
            let symbol = symbol_opt.unwrap_or(&entry.name.name);

            let acc = LibAccumulator::new(lib_path, symbol, entry.return_type.clone())
                .map_err(|e| {
                    datafusion::common::DataFusionError::Execution(format!(
                        "Failed to create lib accumulator for '{}': {}",
                        name, e
                    ))
                })?;
            Ok(Box::new(acc))
        }
        _ => Err(datafusion::common::DataFusionError::NotImplemented(format!(
            "Aggregate functions with runner '{}' are not yet supported for '{}'. \
             IPC/Java/Docker aggregate functions require in-process state management. \
             Use the 'python' or 'lib' runner for aggregate functions.",
            entry.runner, name
        ))),
    }
}

/// Find the overload whose input_types match the actual argument types.
fn find_matching_overload<'a>(
    overloads: &'a [FunctionEntry],
    arg_types: &[DataType],
) -> Option<&'a FunctionEntry> {
    overloads.iter().find(|entry| entry.input_types == arg_types)
}

/// A DataFusion aggregate function backed by one or more FunctionEntry overloads.
#[derive(Debug)]
pub struct AggregateFunction {
    name: String,
    signature: Signature,
    overloads: Vec<FunctionEntry>,
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
    pub fn new(entry: FunctionEntry) -> Result<Self, BundlebaseError> {
        let name = entry.name.to_string();
        let signature = Signature::new(
            TypeSignature::Exact(entry.input_types.clone()),
            Volatility::Volatile,
        );
        Ok(Self {
            name,
            signature,
            overloads: vec![entry],
        })
    }

    /// Create a composite aggregate function from multiple overloads.
    pub fn new_composite(overloads: Vec<FunctionEntry>) -> Result<Self, BundlebaseError> {
        if overloads.is_empty() {
            return Err("Cannot create composite aggregate function with no overloads".into());
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
        })
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
        // Find matching overload for state type, or default to first
        let entry = if self.overloads.len() == 1 {
            &self.overloads[0]
        } else {
            let arg_types: Vec<DataType> = args.input_fields.iter().map(|f| f.data_type().clone()).collect();
            find_matching_overload(&self.overloads, &arg_types)
                .unwrap_or(&self.overloads[0])
        };
        Ok(vec![Arc::new(arrow::datatypes::Field::new(
            "state",
            entry.return_type.clone(),
            true,
        ))])
    }

    fn accumulator(
        &self,
        acc_args: datafusion::logical_expr::function::AccumulatorArgs,
    ) -> DFResult<Box<dyn Accumulator>> {
        let entry = if self.overloads.len() == 1 {
            &self.overloads[0]
        } else {
            let arg_types: Vec<DataType> = acc_args.expr_fields.iter().map(|f| f.data_type().clone()).collect();
            find_matching_overload(&self.overloads, &arg_types)
                .ok_or_else(|| datafusion::common::DataFusionError::Execution(format!(
                    "No overload of aggregate '{}' matches argument types {:?}",
                    self.name, arg_types
                )))?
        };
        create_accumulator_for_entry(&self.name, entry)
    }
}

/// Parse a Python logic string in `"module:class"` format.
fn parse_python_logic(logic: &str) -> DFResult<(&str, &str)> {
    let parts: Vec<&str> = logic.rsplitn(2, ':').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(datafusion::common::DataFusionError::Execution(format!(
            "Invalid Python logic '{}'. Expected 'module:class_name' format.",
            logic
        )));
    }
    // rsplitn reverses order
    Ok((parts[1], parts[0]))
}

/// Accumulator that delegates to Python aggregate class methods.
#[derive(Debug)]
struct PythonAccumulator {
    module: String,
    class_name: String,
    state: ScalarValue,
    function_name: String,
}

impl Accumulator for PythonAccumulator {
    fn update_batch(
        &mut self,
        values: &[arrow::array::ArrayRef],
    ) -> DFResult<()> {
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

    fn merge_batch(
        &mut self,
        states: &[arrow::array::ArrayRef],
    ) -> DFResult<()> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::connector_definition::Platform;
    use crate::bundle::function_definition::FunctionKind;
    use crate::NamespacedName;

    #[test]
    fn test_signature_construction() {
        let entry = FunctionEntry {
            name: NamespacedName::new("acme", "my_sum"),
            input_types: vec![DataType::Int64],
            return_type: DataType::Int64,
            runner: Runner::Python,
            logic: "my_module:MySum".to_string(),
            platform: Platform::any(),
            temporary: true,
            kind: FunctionKind::Aggregate,
        };

        let agg = AggregateFunction::new(entry).expect("should create");
        assert_eq!(agg.name(), "acme.my_sum");
        assert_eq!(
            agg.return_type(&[DataType::Int64]).unwrap(),
            DataType::Int64
        );
    }

    #[test]
    fn test_parse_python_logic_valid() {
        let (module, class) = parse_python_logic("my_module:MySum").unwrap();
        assert_eq!(module, "my_module");
        assert_eq!(class, "MySum");
    }

    #[test]
    fn test_parse_python_logic_dotted_module() {
        let (module, class) = parse_python_logic("pkg.subpkg.mod:MyAgg").unwrap();
        assert_eq!(module, "pkg.subpkg.mod");
        assert_eq!(class, "MyAgg");
    }

    #[test]
    fn test_parse_python_logic_invalid() {
        assert!(parse_python_logic("no_colon").is_err());
        assert!(parse_python_logic(":no_module").is_err());
        assert!(parse_python_logic("no_func:").is_err());
    }

    #[test]
    fn test_composite_empty_overloads() {
        let result = AggregateFunction::new_composite(vec![]);
        assert!(result.is_err());
    }

    #[test]
    fn test_composite_single_overload() {
        let entry = FunctionEntry {
            name: NamespacedName::new("acme", "my_sum"),
            input_types: vec![DataType::Int64],
            return_type: DataType::Int64,
            runner: Runner::Python,
            logic: "my_module:MySum".to_string(),
            platform: Platform::any(),
            temporary: true,
            kind: FunctionKind::Aggregate,
        };
        let agg = AggregateFunction::new_composite(vec![entry]).expect("should create");
        assert_eq!(agg.name(), "acme.my_sum");
        assert_eq!(agg.return_type(&[DataType::Int64]).unwrap(), DataType::Int64);
    }

    #[test]
    fn test_composite_multiple_overloads() {
        let int_entry = FunctionEntry {
            name: NamespacedName::new("acme", "my_sum"),
            input_types: vec![DataType::Int64],
            return_type: DataType::Int64,
            runner: Runner::Python,
            logic: "my_module:IntSum".to_string(),
            platform: Platform::any(),
            temporary: true,
            kind: FunctionKind::Aggregate,
        };
        let float_entry = FunctionEntry {
            name: NamespacedName::new("acme", "my_sum"),
            input_types: vec![DataType::Float64],
            return_type: DataType::Float64,
            runner: Runner::Python,
            logic: "my_module:FloatSum".to_string(),
            platform: Platform::any(),
            temporary: true,
            kind: FunctionKind::Aggregate,
        };
        let agg = AggregateFunction::new_composite(vec![int_entry, float_entry]).expect("should create");
        assert_eq!(agg.name(), "acme.my_sum");
        assert_eq!(agg.return_type(&[DataType::Int64]).unwrap(), DataType::Int64);
        assert_eq!(agg.return_type(&[DataType::Float64]).unwrap(), DataType::Float64);
        assert!(agg.return_type(&[DataType::Boolean]).is_err());
    }
}
