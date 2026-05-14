//! UdfRuntime enum, parsing, delegation to inner trait impls, serde, and path resolution.

use super::docker::DockerRuntime;
use super::ffi::FfiRuntime;
use super::ipc::IpcRuntime;
use super::java::JavaRuntime;
use super::python::PythonRuntime;

use super::entrypoint::{RuntimeType, UdfEntrypoint};
pub use crate::bridge::manifest::{Manifest, ManifestEntry};
use bundlebase_common::BundlebaseError;
use bundlebase_io::IOReadWriteDir;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

/// The execution environment for connector/function entrypoints.
///
/// Each variant wraps a concrete struct holding parsed fields.
/// Serializes as the FROM string (e.g., `"ffi::./mylib.so:double_val"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UdfRuntime {
    Python(PythonRuntime),
    Ffi(FfiRuntime),
    Java(JavaRuntime),
    Docker(DockerRuntime),
    Ipc(IpcRuntime),
}

impl UdfRuntime {
    /// Parse a FROM string like `"runtime::entrypoint"` into a UdfRuntime.
    ///
    /// The scheme (before `::`) determines the variant, and everything after
    /// `::` is parsed by the variant's constructor.
    ///
    /// # Examples
    /// - `"ipc::./my_func"` → `UdfRuntime::Ipc(IpcRuntime { path: "./my_func" })`
    /// - `"ffi::./mylib.so:double_val"` → `UdfRuntime::Ffi(FfiRuntime { path: "./mylib.so", symbol: Some("double_val") })`
    /// - `"python::mod:func"` → `UdfRuntime::Python(PythonRuntime { module: "mod", function: "func" })`
    pub fn parse_from(from: &str) -> Result<Self, BundlebaseError> {
        let separator = "::";
        let pos = from.find(separator).ok_or_else(|| -> BundlebaseError {
            format!(
                "Invalid FROM '{}'. Expected format: 'runtime::entrypoint' (e.g., 'ipc::./my_func').",
                from
            )
            .into()
        })?;
        let scheme = &from[..pos];
        let entrypoint = &from[pos + separator.len()..];
        if entrypoint.is_empty() {
            return Err(format!(
                "Invalid FROM '{}'. Entrypoint part after '::' cannot be empty.",
                from
            )
            .into());
        }

        match scheme {
            "python" => Ok(UdfRuntime::Python(PythonRuntime::parse(entrypoint)?)),
            "ffi" | "lib" => Ok(UdfRuntime::Ffi(FfiRuntime::parse(entrypoint)?)),
            "java" => Ok(UdfRuntime::Java(JavaRuntime::parse(entrypoint)?)),
            "docker" => Ok(UdfRuntime::Docker(DockerRuntime::parse(entrypoint)?)),
            "ipc" => Ok(UdfRuntime::Ipc(IpcRuntime::parse(entrypoint)?)),
            _ => Err(format!(
                "Invalid runtime '{}'. Must be one of: python, ffi, java, docker, ipc.",
                scheme
            )
            .into()),
        }
    }

    /// Reconstruct the full FROM string: `"runtime::entrypoint"`.
    pub fn to_from_string(&self) -> String {
        format!("{}::{}", self.runtime_name(), self.to_entrypoint_string())
    }

    /// Return a new UdfRuntime with a different file path.
    ///
    /// Only meaningful for runtimes that reference files (Ffi, Ipc, Java, file-backed Python).
    /// Panics for module-backed Python and Docker (which don't have file paths).
    pub fn with_path(self, new_path: String) -> Self {
        match self {
            UdfRuntime::Ffi(r) => UdfRuntime::Ffi(r.with_path(new_path)),
            UdfRuntime::Ipc(r) => UdfRuntime::Ipc(r.with_path(new_path)),
            UdfRuntime::Java(r) => UdfRuntime::Java(r.with_path(new_path)),
            UdfRuntime::Python(r) => UdfRuntime::Python(r.with_path(new_path)),
            UdfRuntime::Docker(_) => {
                panic!(
                    "with_path not supported for {} runtime",
                    self.runtime_name()
                )
            }
        }
    }

    // ---- Delegate all UdfEntrypoint methods ----

    fn inner(&self) -> &dyn UdfEntrypoint {
        match self {
            UdfRuntime::Python(r) => r,
            UdfRuntime::Ffi(r) => r,
            UdfRuntime::Java(r) => r,
            UdfRuntime::Docker(r) => r,
            UdfRuntime::Ipc(r) => r,
        }
    }

    /// Whether this runtime's entrypoint can be persisted in a bundle.
    pub fn can_bundle(&self) -> bool {
        self.inner().can_bundle()
    }

    /// The type of connector registry used by this runtime.
    pub fn runtime_type(&self) -> RuntimeType {
        self.inner().runtime_type()
    }

    /// The runtime name (e.g., "ffi", "python").
    pub fn runtime_name(&self) -> &'static str {
        match self {
            UdfRuntime::Python(_) => "python",
            UdfRuntime::Ffi(_) => "ffi",
            UdfRuntime::Java(_) => "java",
            UdfRuntime::Docker(_) => "docker",
            UdfRuntime::Ipc(_) => "ipc",
        }
    }

    /// Reconstruct the entrypoint portion of the FROM string.
    pub fn to_entrypoint_string(&self) -> String {
        self.inner().to_entrypoint_string()
    }

    /// Return the file path if this runtime references a local file.
    pub fn file_path(&self) -> Option<&str> {
        self.inner().file_path()
    }

    /// Build the prefixed call string for IPC/native dispatch.
    pub fn build_call_string(&self) -> String {
        self.inner().build_call_string()
    }

    /// Validate that the referenced entrypoint (file, module, etc.) is reachable.
    pub fn validate_entrypoint(&self) -> Result<(), BundlebaseError> {
        self.inner().validate_entrypoint()
    }

    /// Verify this runtime's bundled artifact is loadable (e.g., load manifest from shared lib).
    pub fn verify_loadable(&self) -> Result<(), BundlebaseError> {
        self.inner().verify_loadable()
    }

    /// Load the function manifest for wildcard discovery.
    pub fn load_manifest(&self) -> Result<Option<Manifest>, BundlebaseError> {
        self.inner().load_manifest()
    }

    /// Look up a single function's metadata from manifest.
    pub fn lookup_function_in_manifest(
        &self,
        function_name: &str,
    ) -> Result<ManifestEntry, BundlebaseError> {
        self.inner().lookup_function_in_manifest(function_name)
    }

    /// Invoke a scalar function.
    pub fn invoke_scalar(
        &self,
        name: &str,
        function_name: &str,
        args: &datafusion::logical_expr::ScalarFunctionArgs,
        subprocess_cache: &crate::bridge::ipc_bridge::SubprocessCache,
    ) -> datafusion::common::Result<datafusion::logical_expr::ColumnarValue> {
        self.inner()
            .invoke_scalar(name, function_name, args, subprocess_cache)
    }

    /// Create an accumulator for an aggregate function.
    pub fn create_accumulator(
        &self,
        name: &str,
        function_name: &str,
        return_type: &arrow::datatypes::DataType,
        subprocess_cache: &crate::bridge::ipc_bridge::SubprocessCache,
    ) -> datafusion::common::Result<Box<dyn datafusion::logical_expr::Accumulator>> {
        self.inner()
            .create_accumulator(name, function_name, return_type, subprocess_cache)
    }

    /// DataType for aggregate state serialization.
    pub fn aggregate_state_type(
        &self,
        return_type: &arrow::datatypes::DataType,
    ) -> arrow::datatypes::DataType {
        self.inner().aggregate_state_type(return_type)
    }

    /// Copy the file referenced by this UdfRuntime into the bundle's data directory.
    ///
    /// For runtimes that reference local files (Ffi, Ipc, Java, file-backed Python),
    /// reads the file, writes it into the bundle's content-addressed storage via
    /// `write_stream()`, and returns a new UdfRuntime pointing to the bundle-relative location.
    ///
    /// For runtimes that don't reference files (Docker, module-backed Python), returns as-is.
    pub async fn copy_into_bundle(
        &self,
        data_dir: &Arc<dyn IOReadWriteDir>,
    ) -> Result<UdfRuntime, BundlebaseError> {
        match self.inner().copy_into_bundle(data_dir).await? {
            Some(new_path) => Ok(self.clone().with_path(new_path)),
            None => Ok(self.clone()),
        }
    }

    /// Resolve an entrypoint path that may be bundle-relative against the bundle's data directory.
    ///
    /// If the runtime references a file and the path is relative (bundle-internal hash path),
    /// it's resolved against the data_dir's filesystem path.
    /// Absolute paths and non-file runtimes are returned as-is.
    pub fn resolve_path(&self, data_dir: &Arc<dyn IOReadWriteDir>) -> UdfRuntime {
        let file_path = match self.file_path() {
            Some(p) => p,
            None => return self.clone(),
        };

        // Only resolve relative paths (bundle-internal hash paths like "ab/cdef12345678.so")
        if file_path.starts_with('/') || file_path.starts_with("./") || file_path.starts_with("../")
        {
            return self.clone();
        }

        // Extract filesystem path from data_dir URL
        let url = data_dir.url();
        if url.scheme() != "file" {
            return self.clone();
        }

        let dir_path = url.path();
        let resolved = format!("{}/{}", dir_path, file_path);

        self.clone().with_path(resolved)
    }

    /// Verify that a bundled function binary is functional from its new location.
    ///
    /// After `copy_into_bundle()`, this loads the manifest from the bundled copy
    /// to confirm the file is readable and executable. Skipped for runtimes that don't
    /// reference files (Docker, Python).
    pub async fn verify_bundled_function(
        &self,
        data_dir: &Arc<dyn IOReadWriteDir>,
    ) -> Result<(), BundlebaseError> {
        if self.file_path().is_none() {
            return Ok(());
        }

        let resolved = self.resolve_path(data_dir);
        resolved.verify_loadable()
    }

    /// Verify that a bundled connector binary is functional from its new location.
    ///
    /// After `copy_into_bundle()`, spawns the connector from its bundled path,
    /// performs a JSON-RPC handshake, and shuts it down. This confirms the binary is
    /// executable and responds from its new location.
    pub async fn verify_bundled_connector(
        &self,
        data_dir: &Arc<dyn IOReadWriteDir>,
    ) -> Result<(), BundlebaseError> {
        if self.file_path().is_none() {
            return Ok(());
        }

        let resolved = self.resolve_path(data_dir);
        if resolved.runtime_type() == super::entrypoint::RuntimeType::Internal {
            return resolved.validate_entrypoint();
        }
        let call_string = resolved.build_call_string();

        super::ipc_utils::verify_ipc_handshake(&call_string).await
    }
}

impl fmt::Display for UdfRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.runtime_name())
    }
}

/// Custom serialization: serialize as the FROM string.
impl Serialize for UdfRuntime {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_from_string())
    }
}

/// Custom deserialization: deserialize from a FROM string.
impl<'de> Deserialize<'de> for UdfRuntime {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        UdfRuntime::parse_from(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::super::entrypoint::find_in_manifest;
    use super::*;
    use bundlebase_common::{ConfigKey, Scope};
    use bundlebase_io::{writable_dir_from_url, ConfigProvider};
    use std::sync::Arc;
    use tempfile::tempdir;
    use url::Url;

    fn test_lib_path() -> String {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let test_lib_dir = format!("{}/../bundlebase/tests/test_lib_function", manifest_dir);

        let status = std::process::Command::new("cargo")
            .arg("build")
            .current_dir(&test_lib_dir)
            .status()
            .expect("Failed to build test lib");
        assert!(status.success(), "Test lib build failed");

        #[cfg(target_os = "macos")]
        let lib_name = "libtest_lib_function.dylib";
        #[cfg(target_os = "linux")]
        let lib_name = "libtest_lib_function.so";
        #[cfg(target_os = "windows")]
        let lib_name = "test_lib_function.dll";

        format!("{}/target/debug/{}", test_lib_dir, lib_name)
    }

    struct EmptyConfig;

    impl ConfigProvider for EmptyConfig {
        fn get_in_scope(
            &self,
            _scope: &Scope,
            _key: &ConfigKey,
        ) -> Result<Option<String>, BundlebaseError> {
            Ok(None)
        }
    }

    #[test]
    fn test_parse_from_ipc_relative() {
        let rt = UdfRuntime::parse_from("ipc::./my_func").unwrap();
        assert_eq!(rt.runtime_name(), "ipc");
        assert_eq!(rt.to_entrypoint_string(), "./my_func");
    }

    #[test]
    fn test_parse_from_ipc_absolute() {
        let rt = UdfRuntime::parse_from("ipc::/usr/bin/func").unwrap();
        assert_eq!(rt.runtime_name(), "ipc");
        assert_eq!(rt.to_entrypoint_string(), "/usr/bin/func");
    }

    #[test]
    fn test_parse_from_ffi() {
        let rt = UdfRuntime::parse_from("ffi::./mylib.so").unwrap();
        assert_eq!(rt.runtime_name(), "ffi");
        assert_eq!(rt.to_entrypoint_string(), "./mylib.so");
    }

    #[test]
    fn test_parse_from_lib_compat() {
        let rt = UdfRuntime::parse_from("lib::./mylib.so").unwrap();
        assert_eq!(rt.runtime_name(), "ffi");
        assert_eq!(rt.to_entrypoint_string(), "./mylib.so");
    }

    #[test]
    fn test_parse_from_python() {
        let rt = UdfRuntime::parse_from("python::mod:func").unwrap();
        assert_eq!(rt.runtime_name(), "python");
        assert_eq!(rt.to_entrypoint_string(), "mod:func");
    }

    #[test]
    fn test_parse_from_docker() {
        let rt = UdfRuntime::parse_from("docker::my-image").unwrap();
        assert_eq!(rt.runtime_name(), "docker");
        assert_eq!(rt.to_entrypoint_string(), "my-image");
    }

    #[test]
    fn test_parse_from_java() {
        let rt = UdfRuntime::parse_from("java::com.example.MyClass").unwrap();
        assert_eq!(rt.runtime_name(), "java");
        assert_eq!(rt.to_entrypoint_string(), "com.example.MyClass");
    }

    #[test]
    fn test_parse_from_invalid_no_separator() {
        assert!(UdfRuntime::parse_from("ipc:./my_func").is_err());
    }

    #[test]
    fn test_parse_from_invalid_empty_entrypoint() {
        assert!(UdfRuntime::parse_from("ipc::").is_err());
    }

    #[test]
    fn test_parse_from_invalid_runtime() {
        assert!(UdfRuntime::parse_from("unknown::./func").is_err());
    }

    #[test]
    fn test_to_from_string() {
        let rt = UdfRuntime::parse_from("ipc::./my_func").unwrap();
        assert_eq!(rt.to_from_string(), "ipc::./my_func");

        let rt = UdfRuntime::parse_from("ffi::./mylib.so").unwrap();
        assert_eq!(rt.to_from_string(), "ffi::./mylib.so");

        let rt = UdfRuntime::parse_from("python::mod:func").unwrap();
        assert_eq!(rt.to_from_string(), "python::mod:func");

        let rt = UdfRuntime::parse_from("ipc::/usr/bin/func").unwrap();
        assert_eq!(rt.to_from_string(), "ipc::/usr/bin/func");
    }

    #[test]
    fn test_from_roundtrip() {
        let from = "ipc::./my_func";
        let rt = UdfRuntime::parse_from(from).unwrap();
        assert_eq!(rt.to_from_string(), from);
    }

    #[test]
    fn test_find_in_manifest_not_found_lists_available() {
        let manifest = Manifest {
            functions: vec![
                ManifestEntry {
                    name: "add".to_string(),
                    symbol: None,
                    input_types: vec!["Int64".to_string()],
                    return_type: "Int64".to_string(),
                    kind: "scalar".to_string(),
                },
                ManifestEntry {
                    name: "multiply".to_string(),
                    symbol: None,
                    input_types: vec!["Int64".to_string()],
                    return_type: "Int64".to_string(),
                    kind: "scalar".to_string(),
                },
            ],
        };

        let err = find_in_manifest(manifest, "nonexistent", "ipc::./my_func")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("nonexistent"),
            "Error should mention the requested function: {}",
            err
        );
        assert!(
            err.contains("add") && err.contains("multiply"),
            "Error should list available functions: {}",
            err
        );
    }

    #[test]
    fn test_find_in_manifest_empty_manifest() {
        let manifest = Manifest { functions: vec![] };

        let err = find_in_manifest(manifest, "nonexistent", "ipc::./my_func")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("no functions"),
            "Error should indicate empty manifest: {}",
            err
        );
    }

    #[test]
    fn test_udf_runtime_can_bundle() {
        // Module-backed Python cannot be bundled
        assert!(!UdfRuntime::parse_from("python::mod:func")
            .unwrap()
            .can_bundle());
        // File-backed Python CAN be bundled
        assert!(UdfRuntime::parse_from("python::./script.py:func")
            .unwrap()
            .can_bundle());
        assert!(UdfRuntime::parse_from("python::path/to/script.py:func")
            .unwrap()
            .can_bundle());

        assert!(UdfRuntime::parse_from("ffi::./lib.so")
            .unwrap()
            .can_bundle());
        assert!(UdfRuntime::parse_from("java::./my.jar")
            .unwrap()
            .can_bundle());
        assert!(UdfRuntime::parse_from("docker::my-image")
            .unwrap()
            .can_bundle());
        assert!(UdfRuntime::parse_from("ipc::./func").unwrap().can_bundle());
    }

    #[test]
    fn test_udf_runtime_serde_roundtrip() {
        let rt = UdfRuntime::parse_from("ffi::./mylib.so:double_val").unwrap();
        let yaml = serde_yaml_ng::to_string(&rt).unwrap();
        assert!(yaml.contains("ffi::./mylib.so:double_val"));
        let deser: UdfRuntime = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(deser, rt);
    }

    #[test]
    fn test_udf_runtime_serde_roundtrip_ipc() {
        let rt = UdfRuntime::parse_from("ipc::./my_func").unwrap();
        let yaml = serde_yaml_ng::to_string(&rt).unwrap();
        let deser: UdfRuntime = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(deser, rt);
    }

    #[test]
    fn test_udf_runtime_serde_roundtrip_python() {
        let rt = UdfRuntime::parse_from("python::mod:func").unwrap();
        let yaml = serde_yaml_ng::to_string(&rt).unwrap();
        let deser: UdfRuntime = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(deser, rt);
    }

    #[test]
    fn test_udf_runtime_serde_roundtrip_python_file_backed() {
        let rt = UdfRuntime::parse_from("python::./script.py:func").unwrap();
        let yaml = serde_yaml_ng::to_string(&rt).unwrap();
        assert!(yaml.contains("python::./script.py:func"));
        let deser: UdfRuntime = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(deser, rt);
    }

    #[test]
    fn test_python_file_backed_detection() {
        // File-backed: contains / or ends with .py
        let rt = UdfRuntime::parse_from("python::./script.py:func").unwrap();
        assert!(rt.can_bundle());
        assert_eq!(rt.runtime_type(), RuntimeType::External);
        assert_eq!(rt.file_path(), Some("./script.py"));

        let rt = UdfRuntime::parse_from("python::path/to/module:func").unwrap();
        assert!(rt.can_bundle());
        assert_eq!(rt.file_path(), Some("path/to/module"));

        // Module-backed: no / and no .py
        let rt = UdfRuntime::parse_from("python::mymodule:func").unwrap();
        assert!(!rt.can_bundle());
        assert_eq!(rt.runtime_type(), RuntimeType::Internal);
        assert_eq!(rt.file_path(), None);
    }

    #[test]
    fn test_validate_entrypoint_python_file_backed_nonexistent() {
        let rt = UdfRuntime::parse_from("python::./nonexistent_script_xyz.py:func").unwrap();
        let err = rt.validate_entrypoint().unwrap_err().to_string();
        assert!(
            err.contains("Python script"),
            "Expected 'Python script' in error: {}",
            err
        );
        assert!(
            err.contains("not found"),
            "Expected 'not found' in error: {}",
            err
        );
    }

    #[test]
    fn test_validate_entrypoint_ipc_nonexistent() {
        let rt = UdfRuntime::parse_from("ipc::./nonexistent_binary_xyz").unwrap();
        let err = rt.validate_entrypoint().unwrap_err().to_string();
        assert!(
            err.contains("IPC executable"),
            "Expected 'IPC executable' in error: {}",
            err
        );
        assert!(
            err.contains("not found"),
            "Expected 'not found' in error: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_verify_bundled_connector_ffi_does_not_use_ipc_spawn() {
        let rt = UdfRuntime::parse_from(&format!("ffi::{}", test_lib_path())).unwrap();
        let temp = tempdir().unwrap();
        let url = Url::from_directory_path(temp.path()).unwrap();
        let data_dir = writable_dir_from_url(&url, Arc::new(EmptyConfig))
            .await
            .expect("create temp data dir");

        rt.verify_bundled_connector(&data_dir)
            .await
            .expect("ffi bundled connector should verify without IPC spawn");
    }

    #[test]
    fn test_validate_entrypoint_ffi_nonexistent() {
        let rt = UdfRuntime::parse_from("ffi::./nonexistent_lib_xyz.so").unwrap();
        let err = rt.validate_entrypoint().unwrap_err().to_string();
        assert!(
            err.contains("Shared library"),
            "Expected 'Shared library' in error: {}",
            err
        );
        assert!(
            err.contains("not found"),
            "Expected 'not found' in error: {}",
            err
        );
    }

    #[test]
    fn test_validate_entrypoint_docker_always_ok() {
        let rt = UdfRuntime::parse_from("docker::any-image:latest").unwrap();
        assert!(rt.validate_entrypoint().is_ok());
    }

    #[test]
    fn test_validate_entrypoint_java_nonexistent() {
        let rt = UdfRuntime::parse_from("java::./nonexistent_xyz.jar").unwrap();
        let err = rt.validate_entrypoint().unwrap_err().to_string();
        assert!(
            err.contains("JAR file"),
            "Expected 'JAR file' in error: {}",
            err
        );
        assert!(
            err.contains("not found"),
            "Expected 'not found' in error: {}",
            err
        );
    }
}
