use arrow::array::ArrayRef;
use arrow::error::ArrowError;

/// Trait for scalar user-defined functions.
pub trait ScalarFunction: Send + Sync {
    /// Apply the function to the given input arrays, returning an output array.
    fn invoke(&self, args: &[ArrayRef]) -> Result<ArrayRef, ArrowError>;
}

/// Trait for aggregate user-defined functions.
pub trait AggregateFunction: Send + Sync {
    /// The type of accumulator state. Stored opaquely by the framework.
    type State: Send + 'static;

    /// Create a new accumulator state.
    fn create_state(&self) -> Result<Self::State, ArrowError>;

    /// Add data from input arrays into the accumulator state.
    fn accumulate(&self, state: &mut Self::State, args: &[ArrayRef]) -> Result<(), ArrowError>;

    /// Merge state_b into state_a.
    fn merge(&self, state_a: &mut Self::State, state_b: Self::State) -> Result<(), ArrowError>;

    /// Produce a final result from the accumulator state.
    fn evaluate(&self, state: &Self::State) -> Result<ArrayRef, ArrowError>;
}

/// Metadata for a single function, used for auto-detection.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FunctionMeta {
    pub name: String,
    pub input_types: Vec<String>,
    pub return_type: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
}

/// Manifest describing all functions in a provider.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FunctionManifest {
    pub functions: Vec<FunctionMeta>,
}

/// Trait for function providers that group functions with metadata.
pub trait FunctionProvider: Send + Sync {
    /// Look up a function by name, returning it as a dynamic dispatch enum.
    fn get_function(&self, name: &str) -> Option<FunctionRef<'_>>;

    /// Return the function metadata for auto-discovery.
    fn metadata(&self) -> FunctionManifest;
}

/// Dynamic reference to either a scalar or aggregate function.
pub enum FunctionRef<'a> {
    Scalar(&'a dyn ScalarFunction),
    Aggregate(&'a dyn DynAggregateFunction),
}

/// Type-erased aggregate function trait (for dynamic dispatch with Any state).
pub trait DynAggregateFunction: Send + Sync {
    fn create_state_dyn(&self) -> Result<Box<dyn std::any::Any + Send>, ArrowError>;
    fn accumulate_dyn(
        &self,
        state: &mut Box<dyn std::any::Any + Send>,
        args: &[ArrayRef],
    ) -> Result<(), ArrowError>;
    fn merge_dyn(
        &self,
        state_a: &mut Box<dyn std::any::Any + Send>,
        state_b: Box<dyn std::any::Any + Send>,
    ) -> Result<(), ArrowError>;
    fn evaluate_dyn(
        &self,
        state: &Box<dyn std::any::Any + Send>,
    ) -> Result<ArrayRef, ArrowError>;
}

/// Blanket impl to bridge typed AggregateFunction to DynAggregateFunction.
impl<T: AggregateFunction> DynAggregateFunction for T {
    fn create_state_dyn(&self) -> Result<Box<dyn std::any::Any + Send>, ArrowError> {
        Ok(Box::new(self.create_state()?))
    }

    fn accumulate_dyn(
        &self,
        state: &mut Box<dyn std::any::Any + Send>,
        args: &[ArrayRef],
    ) -> Result<(), ArrowError> {
        let s = state
            .downcast_mut::<T::State>()
            .ok_or_else(|| ArrowError::InvalidArgumentError("State type mismatch".to_string()))?;
        self.accumulate(s, args)
    }

    fn merge_dyn(
        &self,
        state_a: &mut Box<dyn std::any::Any + Send>,
        state_b: Box<dyn std::any::Any + Send>,
    ) -> Result<(), ArrowError> {
        let a = state_a
            .downcast_mut::<T::State>()
            .ok_or_else(|| ArrowError::InvalidArgumentError("State type mismatch".to_string()))?;
        let b = *state_b
            .downcast::<T::State>()
            .map_err(|_| ArrowError::InvalidArgumentError("State type mismatch".to_string()))?;
        self.merge(a, b)
    }

    fn evaluate_dyn(
        &self,
        state: &Box<dyn std::any::Any + Send>,
    ) -> Result<ArrayRef, ArrowError> {
        let s = state
            .downcast_ref::<T::State>()
            .ok_or_else(|| ArrowError::InvalidArgumentError("State type mismatch".to_string()))?;
        self.evaluate(s)
    }
}
