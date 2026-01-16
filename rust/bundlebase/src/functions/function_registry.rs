use crate::functions::function_impl::FunctionImpl;
use crate::BundlebaseError;
use arrow_schema::SchemaRef;
use log::debug;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Default)]
pub struct FunctionRegistry {
    functions: HashMap<String, FunctionSignature>,
    impls: HashMap<String, Arc<dyn FunctionImpl>>,
}

impl Clone for FunctionRegistry {
    fn clone(&self) -> Self {
        Self {
            functions: self.functions.clone(),
            impls: self.impls.clone(),
        }
    }
}

impl FunctionRegistry {
    pub fn new() -> Self {
        debug!("Creating FunctionRegistry");
        Self {
            functions: HashMap::new(),
            impls: HashMap::new(),
        }
    }

    pub fn register(&mut self, signature: FunctionSignature) -> Result<(), BundlebaseError> {
        debug!("Registering function: {}", signature.name());
        self.functions
            .insert(signature.name().to_string(), signature);
        Ok(())
    }

    pub fn get_function(&self, name: &str) -> Option<FunctionSignature> {
        debug!(
            "Available functions: {}",
            self.functions
                .keys()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        self.functions.get(name).cloned()
    }

    pub fn get_impl(&self, name: &str) -> Option<Arc<dyn FunctionImpl>> {
        debug!(
            "Available implementations: {}",
            self.impls
                .keys()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        self.impls.get(name).cloned()
    }

    pub fn set_impl(
        &mut self,
        name: &str,
        def: Arc<dyn FunctionImpl>,
    ) -> Result<(), BundlebaseError> {
        debug!("Registering function implementation: {}", name);
        self.impls.insert(name.to_string(), def);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::functions::function_registry::FunctionSignature;
    use crate::DataGenerator;
    use arrow::datatypes::{DataType, Field, Schema};
    use std::sync::Arc;

    // Mock FunctionImpl for testing
    #[derive(Debug)]
    struct MockFunctionImpl;

    impl FunctionImpl for MockFunctionImpl {
        fn execute(
            &self,
            _sig: Arc<FunctionSignature>,
        ) -> Result<Arc<dyn DataGenerator>, BundlebaseError> {
            unimplemented!()
        }

        fn version(&self) -> String {
            "MOCK-v1".to_string()
        }
    }

    #[test]
    fn test_new_registry_is_empty() {
        let registry = FunctionRegistry::new();
        assert!(registry.get_function("any_function").is_none());
        assert!(registry.get_impl("any_function").is_none());
    }

    #[test]
    fn test_register_and_get_function() {
        let mut registry = FunctionRegistry::new();
        let schema = Arc::new(Schema::new(vec![Field::new(
            "test",
            DataType::Int32,
            false,
        )]));

        registry
            .register(FunctionSignature::new("test_func", schema.clone()))
            .unwrap();

        let retrieved = registry.get_function("test_func");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().output().fields().len(), 1);
    }

    #[test]
    fn test_get_nonexistent_function() {
        let registry = FunctionRegistry::new();
        assert!(registry.get_function("nonexistent").is_none());
    }

    #[test]
    fn test_register_duplicate_function() {
        let mut registry = FunctionRegistry::new();
        let schema1 = Arc::new(Schema::new(vec![Field::new(
            "col1",
            DataType::Int32,
            false,
        )]));
        let schema2 = Arc::new(Schema::new(vec![Field::new("col2", DataType::Utf8, true)]));

        registry
            .register(FunctionSignature::new("duplicate", schema1))
            .unwrap();
        registry
            .register(FunctionSignature::new("duplicate", schema2))
            .unwrap();

        // Second registration should overwrite the first
        let retrieved = registry.get_function("duplicate").unwrap();
        let out = retrieved.output();
        let fields = out.fields();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].name(), "col2");
    }

    #[test]
    fn test_set_and_get_impl() {
        let mut registry = FunctionRegistry::new();
        let impl_arc = Arc::new(MockFunctionImpl);

        registry.set_impl("test_impl", impl_arc).unwrap();

        let retrieved = registry.get_impl("test_impl");
        assert!(retrieved.is_some());
    }

    #[test]
    fn test_get_nonexistent_impl() {
        let registry = FunctionRegistry::new();
        assert!(registry.get_impl("nonexistent").is_none());
    }

    #[test]
    fn test_impl_without_signature() {
        // Can set implementation without signature
        let mut registry = FunctionRegistry::new();
        let impl_arc = Arc::new(MockFunctionImpl);

        registry.set_impl("orphan_impl", impl_arc).unwrap();

        assert!(registry.get_impl("orphan_impl").is_some());
        assert!(registry.get_function("orphan_impl").is_none());
    }

    #[test]
    fn test_signature_without_impl() {
        // Can register signature without implementation
        let mut registry = FunctionRegistry::new();
        let schema = Arc::new(Schema::new(vec![Field::new(
            "test",
            DataType::Int32,
            false,
        )]));

        registry
            .register(FunctionSignature::new("orphan_sig", schema))
            .unwrap();

        assert!(registry.get_function("orphan_sig").is_some());
        assert!(registry.get_impl("orphan_sig").is_none());
    }

    #[test]
    fn test_multiple_functions() {
        let mut registry = FunctionRegistry::new();

        for i in 0..5 {
            let schema = Arc::new(Schema::new(vec![Field::new(
                format!("col{}", i),
                DataType::Int32,
                false,
            )]));
            registry
                .register(FunctionSignature::new(&format!("func{}", i), schema))
                .unwrap();
        }

        // All functions should be retrievable
        for i in 0..5 {
            assert!(registry.get_function(&format!("func{}", i)).is_some());
        }
    }

    #[test]
    fn test_overwrite_impl() {
        let mut registry = FunctionRegistry::new();
        let impl1 = Arc::new(MockFunctionImpl);
        let impl2 = Arc::new(MockFunctionImpl);

        registry.set_impl("test", impl1).unwrap();
        registry.set_impl("test", impl2).unwrap();

        // Should have the second implementation
        assert!(registry.get_impl("test").is_some());
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionSignature {
    name: String,
    output: SchemaRef,
}

impl FunctionSignature {
    pub fn new(name: &str, schema: SchemaRef) -> FunctionSignature {
        Self {
            name: name.to_string(),
            output: schema,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn output(&self) -> &SchemaRef {
        &self.output
    }
}
