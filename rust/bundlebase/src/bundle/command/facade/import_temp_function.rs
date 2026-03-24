//! ImportTempFunction command implementation (session-only).
//!
//! Loads a function at runtime only, without persisting an operation.
//! Works on both `Bundle` and `BundleBuilder` via `BundleFacade`.
//!
//! Supports both single and wildcard/bulk discovery modes.

use crate::bundle::command::parser::{escape_string, extract_string_content};
use crate::bundle::command::response::OutputShape;
use crate::bundle::command::{BundleFacadeCommand, CommandParsing, Rule};
use crate::platform::Platform;
use crate::udf::{UdfRuntime, RuntimeType};
use crate::bundle::facade::BundleFacade;
use crate::arrow_types::parse_arrow_type_name;
use crate::bundle::function_entry::{FunctionEntry, FunctionKind};
use crate::NamespacedName;
use crate::BundlebaseError;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use std::collections::HashMap;
use std::sync::Arc;

/// Command to load a function at runtime only (not persisted).
///
/// Types and kind are always auto-detected from the function's manifest.
#[derive(Debug, Clone)]
pub struct ImportTempFunctionCommand {
    /// Full dotted function name, or `namespace.*` for wildcard mode
    pub name: String,
    /// From string (e.g. "python::mod:func")
    pub from: String,
    /// Platform
    pub platform: Platform,
}

impl ImportTempFunctionCommand {
    pub fn new(
        name: impl Into<String>,
        from: impl Into<String>,
        platform: Platform,
    ) -> Self {
        Self {
            name: name.into(),
            from: from.into(),
            platform,
        }
    }

    /// Validates a function entry and registers it with the facade.
    ///
    /// Performs IPC entrypoint string validation and kind consistency checks,
    /// then adds to the registry and re-registers UDFs.
    async fn validate_and_register(
        facade: &dyn BundleFacade,
        entry: FunctionEntry,
    ) -> Result<(), BundlebaseError> {
        // Validate IPC entrypoint string at import time (fail early)
        if entry.from.runtime_type() == RuntimeType::External {
            crate::function::ipc_bridge::parse_call(&entry.from.build_call_string())?;
        }

        // Validate kind consistency before adding
        let name = entry.name.to_string();
        {
            let existing = facade.function_registry().read().resolve_all(&name);
            if !existing.is_empty() {
                let existing_kind = existing[0].kind;
                if entry.kind != existing_kind {
                    return Err(format!(
                        "Function '{}' has overloads with mixed kinds (scalar and aggregate). \
                         All overloads of a function must be the same kind.",
                        name
                    ).into());
                }
            }
        }

        // Add to registry then re-register all overloads for this name
        facade.function_registry().write().add(entry);
        facade.function_registry().read().register_functions_for_name(&name)?;
        facade.function_registry().read().refresh_version_udf(facade.version());
        Ok(())
    }

    /// Returns true if this is a wildcard/bulk discovery command.
    pub fn is_wildcard(&self) -> bool {
        self.name.ends_with(".*")
    }

    /// Returns the namespace for wildcard mode.
    fn wildcard_namespace(&self) -> &str {
        &self.name[..self.name.len() - 2]
    }

    /// Returns the Arrow schema for this command's output.
    pub fn output_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![Field::new(
            "message",
            DataType::Utf8,
            false,
        )]))
    }

    /// Returns the expected output shape.
    pub fn output_shape() -> OutputShape {
        OutputShape::SingleValue
    }
}

impl CommandParsing for ImportTempFunctionCommand {
    fn rule() -> Rule {
        Rule::import_temp_function_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;
        let mut from = None;
        let mut args = HashMap::new();

        for inner_pair in pair.into_inner() {
            match inner_pair.as_rule() {
                Rule::function_name => {
                    name = Some(inner_pair.as_str().to_string());
                }
                Rule::quoted_string => {
                    from = Some(extract_string_content(inner_pair.as_str())?);
                }
                Rule::source_args => {
                    for arg_pair in inner_pair.into_inner() {
                        if arg_pair.as_rule() == Rule::source_arg_pair {
                            let mut key = None;
                            let mut value = None;
                            for part in arg_pair.into_inner() {
                                match part.as_rule() {
                                    Rule::identifier => {
                                        key = Some(part.as_str().to_string());
                                    }
                                    Rule::quoted_string => {
                                        value = Some(extract_string_content(part.as_str())?);
                                    }
                                    _ => {}
                                }
                            }
                            if let (Some(k), Some(v)) = (key, value) {
                                args.insert(k, v);
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        let name = name.ok_or_else(|| -> BundlebaseError {
            "IMPORT TEMP FUNCTION missing function name".into()
        })?;

        let from = from.ok_or_else(|| -> BundlebaseError {
            "IMPORT TEMP FUNCTION missing FROM clause".into()
        })?;

        let platform: Platform = match args.remove("platform") {
            Some(s) => s.parse()?,
            None => Platform::any(),
        };

        Ok(ImportTempFunctionCommand::new(name, from, platform))
    }

    fn to_statement(&self) -> String {
        let mut with_parts = Vec::new();
        if self.platform != Platform::any() {
            with_parts.push(format!("platform = {}", escape_string(&self.platform.to_string())));
        }

        if with_parts.is_empty() {
            format!(
                "IMPORT TEMP FUNCTION {} FROM {}",
                self.name,
                escape_string(&self.from)
            )
        } else {
            format!(
                "IMPORT TEMP FUNCTION {} FROM {} WITH ({})",
                self.name,
                escape_string(&self.from),
                with_parts.join(", ")
            )
        }
    }
}

impl BundleFacadeCommand for ImportTempFunctionCommand {
    type Output = String;

    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<String, BundlebaseError> {
        let from = UdfRuntime::parse_from(&self.from)?;
        from.validate_entrypoint()?;

        if self.is_wildcard() {
            // Wildcard/bulk mode
            let namespace = self.wildcard_namespace().to_string();

            let manifest = from.load_manifest()?
                .ok_or_else(|| -> BundlebaseError {
                    format!(
                        "Wildcard function discovery not supported for '{}' runtime",
                        from.runtime_name()
                    )
                    .into()
                })?;

            let mut count = 0;
            for manifest_entry in &manifest.functions {
                let input_types = manifest_entry
                    .input_types
                    .iter()
                    .map(|s| parse_arrow_type_name(s))
                    .collect::<Result<Vec<_>, _>>()?;
                let return_type = parse_arrow_type_name(&manifest_entry.return_type)?;
                let kind: FunctionKind = manifest_entry.kind.parse()?;

                let symbol = manifest_entry.symbol.as_deref().unwrap_or(&manifest_entry.name);
                let entrypoint = format!("{}:{}", from.to_entrypoint_string(), symbol);
                let func_from = UdfRuntime::parse_from(&format!("{}::{}", from.runtime_name(), entrypoint))?;
                let name = format!("{}.{}", namespace, manifest_entry.name);
                let namespaced: NamespacedName = name.parse()?;

                let entry = FunctionEntry {
                    id: crate::data::ObjectId::generate(),
                    name: namespaced,
                    input_types,
                    return_type,
                    from: func_from,
                    platform: self.platform.clone(),
                    temporary: true,
                    kind,
                };
                Self::validate_and_register(facade, entry).await?;
                count += 1;
            }

            Ok(format!(
                "Loaded {} temporary function(s) from '{}'",
                count, from.to_entrypoint_string()
            ))
        } else {
            // Single function mode — auto-detect types from manifest
            let namespaced: NamespacedName = self.name.parse()
                .map_err(|e: crate::BundlebaseError| e)?;

            // Use the symbol from the from string (e.g., "MySum" from "module:MySum")
            // rather than the SQL name, since the SQL name may differ from the manifest entry.
            let entrypoint_str = from.to_entrypoint_string();
            let symbol = entrypoint_str.rsplit(':').next().unwrap_or(&entrypoint_str);
            let manifest_entry = from.lookup_function_in_manifest(symbol)?;
            let kind: FunctionKind = match manifest_entry.kind.as_str() {
                "aggregate" => FunctionKind::Aggregate,
                _ => FunctionKind::Scalar,
            };

            let input_types = manifest_entry.input_types.iter()
                .map(|s| parse_arrow_type_name(s))
                .collect::<Result<Vec<_>, _>>()?;
            let return_type = parse_arrow_type_name(&manifest_entry.return_type)?;
            let entry = FunctionEntry {
                id: crate::data::ObjectId::generate(),
                name: namespaced,
                input_types,
                return_type,
                from: from.clone(),
                platform: self.platform.clone(),
                temporary: true,
                kind,
            };
            Self::validate_and_register(facade, entry).await?;
            Ok(format!("Loaded temporary function: {}", self.name))
        }
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_import_temp_function() {
        let input = "IMPORT TEMP FUNCTION acme.double_val FROM 'python::mod:func'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportTempFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert_eq!(c.from, "python::mod:func");
                assert_eq!(c.platform, Platform::any());
            }
            _ => panic!("Expected ImportTempFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_temp_function_wildcard() {
        let input = "IMPORT TEMP FUNCTION acme.* FROM 'ffi::./mylib.so'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportTempFunction(c) => {
                assert_eq!(c.name, "acme.*");
                assert!(c.is_wildcard());
                assert_eq!(c.from, "ffi::./mylib.so");
            }
            _ => panic!("Expected ImportTempFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_temp_function_roundtrip() {
        let cmd = ImportTempFunctionCommand::new(
            "acme.double_val",
            "python::mod:func",
            Platform::any(),
        );
        let statement = cmd.to_statement();
        assert_eq!(statement, "IMPORT TEMP FUNCTION acme.double_val FROM 'python::mod:func'");
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::ImportTempFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert_eq!(c.from, "python::mod:func");
                assert_eq!(c.platform, Platform::any());
            }
            _ => panic!("Expected ImportTempFunction variant"),
        }
    }

    #[test]
    fn test_parse_default_platform() {
        let input = "IMPORT TEMP FUNCTION acme.double_val FROM 'python::mod:func'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportTempFunction(c) => {
                assert_eq!(c.platform, Platform::any());
            }
            _ => panic!("Expected ImportTempFunction variant"),
        }
    }
}
