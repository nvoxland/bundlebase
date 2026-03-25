//! ImportFunction command implementation (persistent).
//!
//! Handles both single function loading and wildcard/bulk discovery
//! (e.g., `IMPORT FUNCTION acme.* FROM 'lib://./mylib.so'`).

use crate::parser::{escape_string, extract_string_content};
use crate::parser::extract_identifier;
use crate::{CommandParsing, Rule};
use bundlebase_common::Platform;
use bundlebase_udf::runtime::UdfRuntime;
use bundlebase_common::arrow_types::parse_arrow_type_name;
use bundlebase_udf::{parse_function_name, FunctionKind};
use bundlebase::bundle::operation::ImportFunctionOp;
use bundlebase_common::BundlebaseError;
use std::collections::HashMap;
use crate::BundleBuilderCommand;
use bundlebase::BundleBuilder;

/// Command to define a named function.
///
/// Supports two modes:
/// - **Single**: `IMPORT FUNCTION acme.double_val FROM 'ipc::./my_func'`
/// - **Wildcard**: `IMPORT FUNCTION acme.* FROM 'ffi::./mylib.so'` (bulk discovery)
#[derive(Debug, Clone)]
pub struct ImportFunctionCommand {
    /// Full dotted function name, or `namespace.*` for wildcard mode
    pub name: String,
    /// The from string (e.g., "ipc::./my_func", "ffi::./mylib.so")
    pub from: String,
    /// Platform
    pub platform: Platform,
}

impl ImportFunctionCommand {
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

    /// Returns true if this is a wildcard/bulk discovery command (name ends with `.*`).
    pub fn is_wildcard(&self) -> bool {
        self.name.ends_with(".*")
    }

    /// Returns the namespace for wildcard mode (strips `.*` suffix).
    fn wildcard_namespace(&self) -> &str {
        &self.name[..self.name.len() - 2]
    }
}

impl CommandParsing for ImportFunctionCommand {
    fn rule() -> Rule {
        Rule::import_function_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;
        let mut from = None;
        let mut args: HashMap<String, String> = HashMap::new();

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
                                        key = Some(extract_identifier(&part));
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
            "IMPORT FUNCTION missing function name".into()
        })?;

        let from = from.ok_or_else(|| -> BundlebaseError {
            "IMPORT FUNCTION missing FROM clause".into()
        })?;

        let platform: Platform = match args.remove("platform") {
            Some(s) => s.parse()?,
            None => Platform::any(),
        };

        Ok(ImportFunctionCommand::new(name, from, platform))
    }

    fn to_statement(&self) -> String {
        let mut with_parts = Vec::new();
        if self.platform != Platform::any() {
            with_parts.push(format!("platform = {}", escape_string(&self.platform.to_string())));
        }

        if with_parts.is_empty() {
            format!(
                "IMPORT FUNCTION {} FROM {}",
                self.name,
                escape_string(&self.from)
            )
        } else {
            format!(
                "IMPORT FUNCTION {} FROM {} WITH ({})",
                self.name,
                escape_string(&self.from),
                with_parts.join(", ")
            )
        }
    }
}

impl BundleBuilderCommand for ImportFunctionCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let from = UdfRuntime::parse_from(&self.from)?;

        // Persistent functions cannot use runtimes that can't be bundled
        if !from.can_bundle() {
            return Err(format!("'{}' runtime cannot be bundled — use import_temp_function instead", from.runtime_name()).into());
        }

        // Copy the referenced file into the bundle ONCE (before manifest loading).
        // The manifest must be loaded from the original path to discover functions,
        // but operations use the bundled path.
        let bundled_from = from.copy_into_bundle(&builder.bundle().data_dir()).await?;

        // Verify the bundled copy works from its new location
        bundled_from.verify_bundled_function(&builder.bundle().data_dir()).await?;

        if self.is_wildcard() {
            // Wildcard/bulk mode — discover all functions from manifest
            let namespace = self.wildcard_namespace();

            let manifest = from.load_manifest()?
                .ok_or_else(|| -> BundlebaseError {
                    format!(
                        "Wildcard function discovery not supported for '{}' runtime",
                        from.runtime_name()
                    )
                    .into()
                })?;

            let mut count = 0;
            for entry in &manifest.functions {
                let input_types = entry
                    .input_types
                    .iter()
                    .map(|s| parse_arrow_type_name(s))
                    .collect::<Result<Vec<_>, _>>()?;
                let return_type = parse_arrow_type_name(&entry.return_type)?;
                let kind: FunctionKind = entry.kind.parse()?;

                let symbol = entry.symbol.as_deref().unwrap_or(&entry.name);
                // bundled_from.to_entrypoint_string() is just the hash path (no symbol) for wildcard mode
                let entrypoint = format!("{}:{}", bundled_from.to_entrypoint_string(), symbol);
                let func_from = UdfRuntime::parse_from(&format!("{}::{}", bundled_from.runtime_name(), entrypoint))?;
                let name = format!("{}.{}", namespace, entry.name);

                let op = ImportFunctionOp::new(
                    name,
                    input_types,
                    return_type,
                    func_from,
                    self.platform.clone(),
                    kind,
                );
                builder.apply_operation(op.into()).await?;
                count += 1;
            }

            Ok(format!(
                "Loaded {} function(s) from '{}'",
                count, from.to_entrypoint_string()
            ))
        } else {
            let namespaced = parse_function_name(&self.name)?;
            let func_name = &namespaced.name;
            let entry = from.lookup_function_in_manifest(func_name)?;
            let kind: FunctionKind = entry.kind.parse()?;

            let input_types = entry.input_types.iter()
                .map(|s| parse_arrow_type_name(s))
                .collect::<Result<Vec<_>, _>>()?;
            let return_type = parse_arrow_type_name(&entry.return_type)?;

            let op = ImportFunctionOp::new(
                self.name.clone(),
                input_types,
                return_type,
                bundled_from,
                self.platform.clone(),
                kind,
            );
            builder.apply_operation(op.into()).await?;

            Ok(format!("Loaded function: {}", self.name))
        }
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::CommandParsing;
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_import_function() {
        let input = "IMPORT FUNCTION acme.double_val FROM 'ipc::./my_func'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert_eq!(c.from, "ipc::./my_func");
                assert_eq!(c.platform, Platform::any());
            }
            _ => panic!("Expected ImportFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_function_with_platform() {
        let input = "IMPORT FUNCTION acme.double_val FROM 'ipc::./my_func' WITH (platform = 'linux/amd64')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert_eq!(c.from, "ipc::./my_func");
                assert_eq!(c.platform.os, "linux");
                assert_eq!(c.platform.arch, "amd64");
            }
            _ => panic!("Expected ImportFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_function_wildcard() {
        let input = "IMPORT FUNCTION acme.* FROM 'ffi::./mylib.so'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportFunction(c) => {
                assert_eq!(c.name, "acme.*");
                assert!(c.is_wildcard());
                assert_eq!(c.from, "ffi::./mylib.so");
            }
            _ => panic!("Expected ImportFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_function_wildcard_with_args() {
        let input = "IMPORT FUNCTION tools.* FROM 'ipc::./my_func' WITH (platform = 'linux/amd64')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportFunction(c) => {
                assert_eq!(c.name, "tools.*");
                assert!(c.is_wildcard());
                assert_eq!(c.from, "ipc::./my_func");
                assert_eq!(c.platform.os, "linux");
            }
            _ => panic!("Expected ImportFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_function_absolute_path() {
        let input = "IMPORT FUNCTION acme.double_val FROM 'ipc::/usr/bin/my_func'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportFunction(c) => {
                assert_eq!(c.from, "ipc::/usr/bin/my_func");
            }
            _ => panic!("Expected ImportFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_function_python() {
        let input = "IMPORT FUNCTION acme.double_val FROM 'python::mod:func'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportFunction(c) => {
                assert_eq!(c.from, "python::mod:func");
            }
            _ => panic!("Expected ImportFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_function_roundtrip() {
        let cmd = ImportFunctionCommand::new(
            "acme.double_val",
            "ipc::./my_func",
            Platform::any(),
        );
        let statement = cmd.to_statement();
        assert_eq!(statement, "IMPORT FUNCTION acme.double_val FROM 'ipc::./my_func'");
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::ImportFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert_eq!(c.from, "ipc::./my_func");
                assert_eq!(c.platform, Platform::any());
            }
            _ => panic!("Expected ImportFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_function_roundtrip_with_platform() {
        let cmd = ImportFunctionCommand::new(
            "acme.double_val",
            "ipc::./my_func",
            "linux/amd64".parse().unwrap(),
        );
        let statement = cmd.to_statement();
        assert!(statement.contains("WITH (platform = 'linux/amd64')"));
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::ImportFunction(c) => {
                assert_eq!(c.platform.os, "linux");
                assert_eq!(c.platform.arch, "amd64");
            }
            _ => panic!("Expected ImportFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_function_wildcard_roundtrip() {
        let cmd = ImportFunctionCommand::new(
            "acme.*",
            "ffi::./mylib.so",
            Platform::any(),
        );
        let statement = cmd.to_statement();
        assert_eq!(statement, "IMPORT FUNCTION acme.* FROM 'ffi::./mylib.so'");
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::ImportFunction(c) => {
                assert_eq!(c.name, "acme.*");
                assert!(c.is_wildcard());
                assert_eq!(c.from, "ffi::./mylib.so");
            }
            _ => panic!("Expected ImportFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_function_case_insensitive() {
        let input = "load function acme.double_val from 'ipc::./test'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert_eq!(c.from, "ipc::./test");
            }
            _ => panic!("Expected ImportFunction variant"),
        }
    }

    #[test]
    fn test_parse_import_function_lib_with_symbol() {
        let input = "IMPORT FUNCTION acme.double_val FROM 'ffi::./mylib.so:double_val'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportFunction(c) => {
                assert_eq!(c.from, "ffi::./mylib.so:double_val");
            }
            _ => panic!("Expected ImportFunction variant"),
        }
    }
}
