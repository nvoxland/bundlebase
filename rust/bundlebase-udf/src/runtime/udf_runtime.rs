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

    /// Whether this runtime executes external code that must be gated behind
    /// `system.allow_external_code`. ffi/ipc/docker/java load or spawn external
    /// code. File-backed python (`python::./x.py:f`) is bundleable and runs the
    /// shipped `.py` in a subprocess, so it is external too. Only module-backed
    /// python (in-process, importable module, never bundled) is not external.
    pub fn is_external(&self) -> bool {
        match self {
            UdfRuntime::Python(r) => r.is_file_backed(),
            _ => true,
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

    /// Resolve an entrypoint path that may be bundle-relative to a local filesystem path.
    ///
    /// Bundle-relative paths (content-addressed hash paths like `ab/cdef12345678.so`)
    /// resolve against the data directory for `file://` bundles. For every other
    /// backend (memory, tar, object stores) they resolve to a local cache location,
    /// which [`resolve_local`](Self::resolve_local) fills. Absolute paths, `./` paths
    /// and runtimes without a file are returned as-is.
    pub fn resolve_path(&self, data_dir: &Arc<dyn IOReadWriteDir>) -> UdfRuntime {
        let Some(file_path) = self.file_path() else {
            return self.clone();
        };
        if !is_bundle_relative(file_path) {
            return self.clone();
        }

        let url = data_dir.url();
        let resolved = if url.scheme() == "file" {
            format!("{}/{}", url.path().trim_end_matches('/'), file_path)
        } else {
            match local_cache_path(url, file_path) {
                Some(p) => p.to_string_lossy().to_string(),
                None => return self.clone(),
            }
        };

        self.clone().with_path(resolved)
    }

    /// Like [`resolve_path`](Self::resolve_path), but also makes sure the file exists
    /// locally. For bundles not on local disk, the bundled file is downloaded into the
    /// local cache (verifying its content hash) so a subprocess or `dlopen` can use it.
    pub async fn resolve_local(
        &self,
        data_dir: &Arc<dyn IOReadWriteDir>,
    ) -> Result<UdfRuntime, BundlebaseError> {
        let resolved = self.resolve_path(data_dir);
        if let Some(rel) = self.file_path() {
            if is_bundle_relative(rel) && data_dir.url().scheme() != "file" {
                let dest = local_cache_path(data_dir.url(), rel).ok_or_else(|| {
                    BundlebaseError::from(format!(
                        "Bundled entrypoint path '{}' is not a valid bundle-relative path",
                        rel
                    ))
                })?;
                materialize_bundled_file(data_dir, rel, &dest).await?;
            }
        }
        Ok(resolved)
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

        let resolved = self.resolve_local(data_dir).await?;
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

        let resolved = self.resolve_local(data_dir).await?;
        if resolved.runtime_type() == super::entrypoint::RuntimeType::Internal {
            return resolved.validate_entrypoint();
        }
        let call_string = resolved.build_call_string();

        super::ipc_utils::verify_ipc_handshake(&call_string).await
    }
}

/// Whether an entrypoint path points inside the bundle (a content-addressed hash
/// path) rather than at a file on the local machine.
fn is_bundle_relative(path: &str) -> bool {
    !(path.starts_with('/') || path.starts_with("./") || path.starts_with("../"))
}

/// Where a bundled file from a non-`file://` bundle is materialized locally.
///
/// Keyed by the data directory URL so one bundle's files can never stand in for
/// another's. Returns `None` if `rel` could escape the cache directory.
fn local_cache_path(data_dir_url: &url::Url, rel: &str) -> Option<std::path::PathBuf> {
    use std::path::{Component, Path};
    let rel_path = Path::new(rel);
    if !rel_path
        .components()
        .all(|c| matches!(c, Component::Normal(_)))
    {
        return None;
    }
    let root = dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("bundlebase")
        .join("udf");
    let key = format!(
        "{:016x}",
        xxhash_rust::xxh3::xxh3_64(data_dir_url.as_str().as_bytes())
    );
    Some(root.join(key).join(rel_path))
}

/// Split a content-addressed path `ab/cdef0123456789.ext` into the hash prefix it
/// claims (`abcdef0123456789`).
fn claimed_hash_prefix(rel: &str) -> Option<String> {
    let (dir, file) = rel.split_once('/')?;
    let stem = file.split('.').next()?;
    if dir.len() != 2 || stem.len() != 14 || file.contains('/') {
        return None;
    }
    Some(format!("{}{}", dir, stem))
}

/// Stream a bundled file into `dest`, verifying that its content matches the hash
/// in its name. Reuses `dest` if it already holds matching content.
async fn materialize_bundled_file(
    data_dir: &Arc<dyn IOReadWriteDir>,
    rel: &str,
    dest: &std::path::Path,
) -> Result<(), BundlebaseError> {
    use futures::StreamExt;
    use tokio::io::AsyncWriteExt;

    let expected = claimed_hash_prefix(rel).ok_or_else(|| {
        BundlebaseError::from(format!(
            "Bundled entrypoint path '{}' is not a content-addressed path",
            rel
        ))
    })?;
    let hash_prefix = |hasher: &xxhash_rust::xxh3::Xxh3| -> String {
        format!("{:032x}", hasher.digest128())[..16].to_string()
    };

    if let Ok(existing) = tokio::fs::read(dest).await {
        let mut hasher = xxhash_rust::xxh3::Xxh3::new();
        hasher.update(&existing);
        if hash_prefix(&hasher) == expected {
            return Ok(());
        }
    }

    let mut stream = data_dir.file(rel)?.read_stream().await?.ok_or_else(|| {
        BundlebaseError::from(format!(
            "Bundled entrypoint file '{}' not found in {}",
            rel,
            data_dir.url()
        ))
    })?;

    let parent = dest
        .parent()
        .ok_or_else(|| BundlebaseError::from(format!("Invalid cache path '{}'", dest.display())))?;
    tokio::fs::create_dir_all(parent).await.map_err(|e| {
        BundlebaseError::from(format!(
            "Failed to create cache directory '{}': {}",
            parent.display(),
            e
        ))
    })?;

    // Write to a unique temp name, then rename, so concurrent materializations
    // never expose a partially written file.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = parent.join(format!(".tmp-{}-{}", std::process::id(), nanos));
    let io_err = |e: std::io::Error| {
        BundlebaseError::from(format!(
            "Failed to write bundled file to '{}': {}",
            dest.display(),
            e
        ))
    };

    let mut hasher = xxhash_rust::xxh3::Xxh3::new();
    let write_result: Result<(), BundlebaseError> = async {
        let mut out = tokio::fs::File::create(&tmp).await.map_err(io_err)?;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            hasher.update(&chunk);
            out.write_all(&chunk).await.map_err(io_err)?;
        }
        out.flush().await.map_err(io_err)?;
        Ok(())
    }
    .await;
    if let Err(e) = write_result {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e);
    }

    let actual = hash_prefix(&hasher);
    if actual != expected {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(format!(
            "Bundled entrypoint file '{}' does not match its content hash (got {}); \
             the bundle may be corrupt",
            rel, actual
        )
        .into());
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
            .await
            .map_err(io_err)?;
    }

    tokio::fs::rename(&tmp, dest).await.map_err(io_err)?;
    Ok(())
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

    /// Bundle a small script into a non-file data dir (memory://) and return
    /// the bundled runtime plus the data dir.
    async fn bundle_script_into_memory(
        content: &str,
    ) -> (UdfRuntime, Arc<dyn IOReadWriteDir>, tempfile::TempDir) {
        let src = tempdir().unwrap();
        let script = src.path().join("my_func.sh");
        std::fs::write(&script, content).unwrap();
        let rt = UdfRuntime::parse_from(&format!("ipc::{}", script.display())).unwrap();

        // The source tempdir's name is unique, so reuse it for the bundle URL.
        let unique = src
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let url = Url::parse(&format!("memory:///resolve_local_{}", unique)).unwrap();
        let data_dir = writable_dir_from_url(&url, Arc::new(EmptyConfig))
            .await
            .expect("create memory data dir");
        let bundled = rt.copy_into_bundle(&data_dir).await.expect("bundle");
        (bundled, data_dir, src)
    }

    #[tokio::test]
    async fn test_resolve_local_materializes_from_non_file_bundle() {
        let content = "#!/bin/sh\necho hi\n";
        let (bundled, data_dir, _src) = bundle_script_into_memory(content).await;

        let resolved = bundled.resolve_local(&data_dir).await.expect("resolve");
        let path = resolved.file_path().expect("ipc has a path").to_string();
        assert!(
            std::path::Path::new(&path).is_absolute(),
            "resolved path should be a local absolute path: {}",
            path
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert!(mode & 0o100 != 0, "materialized file should be executable");
        }

        // Synchronous resolve_path agrees with the materialized location.
        assert_eq!(
            bundled.resolve_path(&data_dir).file_path(),
            Some(path.as_str())
        );
    }

    #[tokio::test]
    async fn test_resolve_local_rejects_content_not_matching_its_hash() {
        let (bundled, data_dir, _src) = bundle_script_into_memory("#!/bin/sh\n").await;
        let rel = bundled.file_path().unwrap().to_string();

        // Overwrite the bundled file in place with different bytes.
        let tampered = data_dir.writable_file(&rel).unwrap();
        let stream = futures::stream::once(async {
            Ok::<bytes::Bytes, std::io::Error>(bytes::Bytes::from_static(b"evil"))
        });
        tampered.write_stream(Box::pin(stream)).await.unwrap();

        let err = bundled
            .resolve_local(&data_dir)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("hash"),
            "expected a hash mismatch error: {}",
            err
        );
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
