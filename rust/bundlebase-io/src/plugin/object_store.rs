//! ObjectStore IO backend - file and directory operations via the object_store crate.
//!
//! Supports: file://, s3://, gs://, azure://, az://, memory://, empty://

use crate::registry::IOFactory;
use crate::util::{join_path, join_url};
use crate::BundlebaseError;
use crate::ConfigProvider;
use crate::{get_memory_store, get_null_store, EMPTY_SCHEME, EMPTY_URL};
use crate::{FileInfo, IOReadDir, IOReadFile, IOReadWriteDir, IOReadWriteFile};
use async_trait::async_trait;
use bundlebase_common::{config_keys, config_scopes, ConfigKey, ConfigScope};
use bytes::Bytes;
use datafusion::datasource::object_store::ObjectStoreUrl;
use futures::stream::{BoxStream, StreamExt, TryStreamExt};
use object_store::path::Path as ObjectPath;
use object_store::{ObjectMeta, ObjectStore, ObjectStoreExt};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt::{Debug, Display};
use std::env::current_dir;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use url::Url;

// ── Custom scheme registry ─────────────────────────────────────────

type ObjectStoreFactoryFn = Box<
    dyn Fn(&Url, &dyn ConfigProvider) -> Result<(Arc<dyn ObjectStore>, ObjectPath), BundlebaseError>
        + Send
        + Sync,
>;

static CUSTOM_SCHEMES: OnceLock<parking_lot::RwLock<HashMap<String, ObjectStoreFactoryFn>>> =
    OnceLock::new();

/// Register a custom ObjectStore factory for a URL scheme.
///
/// Once registered, `parse_url()` will delegate to the factory for any URL
/// whose scheme matches. This is intended for benchmarks and tests that need
/// to inject a wrapped store (e.g., `ThrottledStore`) that survives the
/// commit-reopen cycle.
pub fn register_object_store_scheme(
    scheme: &str,
    factory: impl Fn(&Url, &dyn ConfigProvider) -> Result<(Arc<dyn ObjectStore>, ObjectPath), BundlebaseError>
        + Send
        + Sync
        + 'static,
) {
    let map = CUSTOM_SCHEMES.get_or_init(|| parking_lot::RwLock::new(HashMap::new()));
    map.write().insert(scheme.to_string(), Box::new(factory));
}

// ── Scope constants ─────────────────────────────────────────────────

config_scopes!(object_store_scopes, {
    pub const S3_SCOPE: ConfigScope = ConfigScope::new("s3");
    pub const GCS_SCOPE: ConfigScope = ConfigScope::new("gs");
    pub const AZURE_SCOPE: ConfigScope = ConfigScope::new("azure");
});

// ── S3 configuration keys ───────────────────────────────────────────

config_keys!(s3_keys, {
    pub const S3_REGION_CFG: ConfigKey = S3_SCOPE.define("region");
    pub const S3_ACCESS_KEY_ID_CFG: ConfigKey = S3_SCOPE.define("access_key_id");
    pub const S3_ENDPOINT_CFG: ConfigKey = S3_SCOPE.define("endpoint");
    pub const S3_BUCKET_CFG: ConfigKey = S3_SCOPE.define("bucket");
    pub const S3_ALLOW_HTTP_CFG: ConfigKey = S3_SCOPE.define("allow_http");
    pub const S3_SKIP_SIGNATURE_CFG: ConfigKey = S3_SCOPE.define("skip_signature");
    pub const S3_VIRTUAL_HOSTED_STYLE_REQUEST_CFG: ConfigKey =
        S3_SCOPE.define("virtual_hosted_style_request");
    pub const S3_IMDSV1_FALLBACK_CFG: ConfigKey = S3_SCOPE.define("imdsv1_fallback");
    pub const S3_METADATA_ENDPOINT_CFG: ConfigKey = S3_SCOPE.define("metadata_endpoint");
    pub const S3_CONTAINER_CREDENTIALS_RELATIVE_URI_CFG: ConfigKey =
        S3_SCOPE.define("container_credentials_relative_uri");
    pub const S3_UNSIGNED_PAYLOAD_CFG: ConfigKey = S3_SCOPE.define("unsigned_payload");
    pub const S3_CHECKSUM_ALGORITHM_CFG: ConfigKey = S3_SCOPE.define("checksum_algorithm");
    pub const S3_COPY_IF_NOT_EXISTS_CFG: ConfigKey = S3_SCOPE.define("copy_if_not_exists");
    pub const S3_CONDITIONAL_PUT_CFG: ConfigKey = S3_SCOPE.define("conditional_put");
    pub const S3_SECRET_ACCESS_KEY_CFG: ConfigKey =
        S3_SCOPE.define("secret_access_key").runtime_only().secure();
    pub const S3_SESSION_TOKEN_CFG: ConfigKey =
        S3_SCOPE.define("session_token").runtime_only().secure();
    pub const S3_TOKEN_CFG: ConfigKey = S3_SCOPE.define("token").runtime_only().secure();
});

// ── GCS configuration keys ──────────────────────────────────────────

config_keys!(gcs_keys, {
    pub const GCS_BUCKET_CFG: ConfigKey = GCS_SCOPE.define("bucket");
    pub const GCS_SERVICE_ACCOUNT_PATH_CFG: ConfigKey = GCS_SCOPE.define("service_account_path");
    pub const GCS_APPLICATION_CREDENTIALS_CFG: ConfigKey =
        GCS_SCOPE.define("application_credentials");
    pub const GCS_SERVICE_ACCOUNT_KEY_CFG: ConfigKey = GCS_SCOPE
        .define("service_account_key")
        .runtime_only()
        .secure();
});

// ── Azure configuration keys ────────────────────────────────────────

config_keys!(azure_keys, {
    pub const AZURE_ACCOUNT_CFG: ConfigKey = AZURE_SCOPE.define("account");
    pub const AZURE_CONTAINER_CFG: ConfigKey = AZURE_SCOPE.define("container");
    pub const AZURE_CLIENT_ID_CFG: ConfigKey = AZURE_SCOPE.define("client_id");
    pub const AZURE_TENANT_ID_CFG: ConfigKey = AZURE_SCOPE.define("tenant_id");
    pub const AZURE_AUTHORITY_HOST_CFG: ConfigKey = AZURE_SCOPE.define("authority_host");
    pub const AZURE_USE_EMULATOR_CFG: ConfigKey = AZURE_SCOPE.define("use_emulator");
    pub const AZURE_ACCESS_KEY_CFG: ConfigKey =
        AZURE_SCOPE.define("access_key").runtime_only().secure();
    pub const AZURE_SAS_TOKEN_CFG: ConfigKey =
        AZURE_SCOPE.define("sas_token").runtime_only().secure();
    pub const AZURE_BEARER_TOKEN_CFG: ConfigKey =
        AZURE_SCOPE.define("bearer_token").runtime_only().secure();
    pub const AZURE_CLIENT_SECRET_CFG: ConfigKey =
        AZURE_SCOPE.define("client_secret").runtime_only().secure();
});

// ============================================================================
// URL and ObjectStore utilities
// ============================================================================

pub(crate) fn compute_store_url(url: &Url) -> ObjectStoreUrl {
    ObjectStoreUrl::parse(format!("{}://{}", url.scheme(), url.authority()))
        .expect("BUG: URL scheme://authority should be valid")
}

/// Parse a URL and return an ObjectStore and Path
///
/// # Arguments
/// * `url` - The URL to parse
/// * `config` - Optional configuration to apply to the ObjectStore
pub(crate) fn parse_url(
    url: &Url,
    config: &dyn ConfigProvider,
) -> Result<(Arc<dyn ObjectStore>, ObjectPath), BundlebaseError> {
    // Check custom scheme registry first (e.g., throttle:// for benchmarks)
    if let Some(map) = CUSTOM_SCHEMES.get() {
        if let Some(factory) = map.read().get(url.scheme()) {
            return factory(url, config);
        }
    }

    if url.scheme() == EMPTY_SCHEME {
        let store: Arc<dyn ObjectStore> = get_null_store();

        if !url.authority().is_empty() {
            return Err("Empty URL must be empty:///<path>.".into());
        }
        Ok((store, ObjectPath::from(url.path())))
    } else if url.scheme() == "memory" {
        if !url.authority().is_empty() {
            return Err("Memory URL must be memory:///<path>".into());
        }
        Ok((get_memory_store(), url.path().into()))
    } else {
        // Try building with config; the builder will use config.get() per key
        let url_str = url.as_str();
        let store = build_object_store(url, url_str, config)?;
        let path = ObjectPath::from(url.path());
        Ok((Arc::new(store), path))
    }
}

/// Build an ObjectStore with configuration.
///
/// Starts with Builder::from_env() to pick up environment variables,
/// then applies config values on top via `config.get(key, &scope)`.
/// TODO: use config first, then fallback to defaults
pub(crate) fn build_object_store(
    url: &Url,
    url_str: &str,
    config: &dyn ConfigProvider,
) -> Result<Box<dyn ObjectStore>, BundlebaseError> {
    use bundlebase_common::Scope;
    use object_store::aws::AmazonS3Builder;
    use object_store::azure::MicrosoftAzureBuilder;
    use object_store::gcp::GoogleCloudStorageBuilder;

    let scope = Scope::try_from(url_str)?;

    match url.scheme() {
        "s3" => {
            let mut builder = AmazonS3Builder::from_env().with_url(url.as_str());

            for spec in s3_keys() {
                if let Some(value) = config.get_in_scope(&scope, spec)? {
                    builder = builder.with_config(spec.key.parse()?, value);
                }
            }

            Ok(Box::new(builder.build()?))
        }
        "gs" => {
            let mut builder = GoogleCloudStorageBuilder::from_env().with_url(url.as_str());

            for spec in gcs_keys() {
                if let Some(value) = config.get_in_scope(&scope, spec)? {
                    builder = builder.with_config(spec.key.parse()?, value);
                }
            }

            Ok(Box::new(builder.build()?))
        }
        "azure" | "az" => {
            let mut builder = MicrosoftAzureBuilder::from_env().with_url(url.as_str());

            for spec in azure_keys() {
                if let Some(value) = config.get_in_scope(&scope, spec)? {
                    builder = builder.with_config(spec.key.parse()?, value);
                }
            }

            Ok(Box::new(builder.build()?))
        }
        scheme => {
            // For unknown schemes, fall back to object_store::parse_url
            let (store, _) = object_store::parse_url(url)
                .map_err(|e| format!("Unsupported URL scheme '{}': {}", scheme, e))?;
            Ok(Box::new(store))
        }
    }
}

// ============================================================================
// IOFile - File abstraction for reading and writing files via object_store
// ============================================================================

/// File abstraction for reading and writing files via object_store.
#[derive(Clone)]
pub struct ObjectStoreFile {
    url: Url,
    store: Arc<dyn ObjectStore>,
    path: ObjectPath,
    /// Held so we can consult config in async hot paths like `version()`
    /// without threading a `&dyn ConfigProvider` through `IOReadFile`.
    config: Arc<dyn ConfigProvider>,
}

impl Debug for ObjectStoreFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IOFile")
            .field("url", &self.url)
            .field("path", &self.path)
            .finish()
    }
}

impl ObjectStoreFile {
    /// Create an IOFile from a URL.
    pub fn from_url(
        url: &Url,
        config: Arc<dyn ConfigProvider>,
    ) -> Result<ObjectStoreFile, BundlebaseError> {
        let (store, path) = parse_url(url, &config)?;
        Self::new(url, store, &path, config)
    }

    /// Creates a file from the passed string.
    /// The string can be either a URL or a path relative to the passed base_dir.
    pub fn from_str(
        path: &str,
        base: &dyn IOReadDir,
        config: Arc<dyn ConfigProvider>,
    ) -> Result<ObjectStoreFile, BundlebaseError> {
        if path.contains(":") {
            // Absolute URL - use provided config
            Self::from_url(&Url::parse(path)?, config)
        } else {
            // Relative path - join with base URL and create from that
            let base_url = base.url();
            let file_url = join_url(base_url, path)?;
            Self::from_url(&file_url, config)
        }
    }

    /// Create an IOFile directly with all components.
    pub fn new(
        url: &Url,
        store: Arc<dyn ObjectStore>,
        path: &ObjectPath,
        config: Arc<dyn ConfigProvider>,
    ) -> Result<Self, BundlebaseError> {
        Ok(Self {
            url: url.clone(),
            store,
            path: path.clone(),
            config,
        })
    }

    /// Get the underlying ObjectStore.
    pub fn store(&self) -> Arc<dyn ObjectStore> {
        self.store.clone()
    }

    /// Get the ObjectStore URL for DataFusion registration.
    pub fn store_url(&self) -> ObjectStoreUrl {
        compute_store_url(&self.url)
    }

    /// Get the path within the object store.
    pub fn store_path(&self) -> &ObjectPath {
        &self.path
    }

    /// Read file contents as a stream, returning an error if the file doesn't exist.
    pub async fn read_existing(
        &self,
    ) -> Result<BoxStream<'static, Result<Bytes, BundlebaseError>>, BundlebaseError> {
        match self.read_stream().await? {
            Some(stream) => Ok(stream),
            None => Err(format!("File not found: {}", self.url).into()),
        }
    }

    /// Get full ObjectMeta from object store.
    pub async fn object_meta(&self) -> Result<Option<ObjectMeta>, BundlebaseError> {
        match self.store.head(&self.path).await {
            Ok(meta) => Ok(Some(meta)),
            Err(e) => {
                if matches!(e, object_store::Error::NotFound { .. }) {
                    Ok(None)
                } else {
                    Err(Box::new(e))
                }
            }
        }
    }
}

#[async_trait]
impl IOReadFile for ObjectStoreFile {
    fn url(&self) -> &Url {
        &self.url
    }

    async fn exists(&self) -> Result<bool, BundlebaseError> {
        match self.store.head(&self.path).await {
            Ok(_) => Ok(true),
            Err(e) => {
                if matches!(e, object_store::Error::NotFound { .. }) {
                    Ok(false)
                } else {
                    Err(Box::new(e))
                }
            }
        }
    }

    async fn open_stream(
        &self,
    ) -> Result<Option<BoxStream<'static, Result<Bytes, BundlebaseError>>>, BundlebaseError> {
        match self.store.get(&self.path).await {
            Ok(result) => {
                let stream = result
                    .into_stream()
                    .map_err(|e| Box::new(e) as BundlebaseError);
                Ok(Some(Box::pin(stream)))
            }
            Err(e) => {
                if matches!(e, object_store::Error::NotFound { .. }) {
                    Ok(None)
                } else {
                    Err(Box::new(e))
                }
            }
        }
    }

    async fn metadata(&self) -> Result<Option<FileInfo>, BundlebaseError> {
        match self.store.head(&self.path).await {
            Ok(meta) => Ok(Some(
                FileInfo::new(self.url.clone())
                    .with_size(meta.size)
                    .with_modified(meta.last_modified),
            )),
            Err(e) => {
                if matches!(e, object_store::Error::NotFound { .. }) {
                    Ok(None)
                } else {
                    Err(Box::new(e))
                }
            }
        }
    }

    async fn version(&self) -> Result<String, BundlebaseError> {
        let meta = self.store.head(&self.path).await?;

        if self.url.scheme() == "file" && is_git_versioning_enabled(&*self.config)? {
            return match git_oid_for_url(&self.url).await {
                Some(oid) => Ok(oid),
                None => Err(format!(
                    "system.git_versioning is enabled but git could not produce an OID for {}. \
                     Verify the file is inside a git working tree and that `git` is on PATH, \
                     or unset system.git_versioning to fall back to mtime-based versions.",
                    self.url
                )
                .into()),
            };
        }
        // Priority: Version (S3 style) → ETag (HTTP standard) → LastModified (hashed timestamp)
        let version = if meta
            .version
            .as_ref()
            .is_some_and(|x| !x.is_empty() && x != "0")
        {
            meta.version
        } else if meta
            .e_tag
            .as_ref()
            .is_some_and(|x| !x.is_empty() && x != "0")
        {
            meta.e_tag
        } else {
            let timestamp = meta.last_modified.to_rfc3339();
            let mut hasher = Sha256::new();
            hasher.update(timestamp.as_bytes());
            let hash = hasher.finalize();
            Some(hex::encode(&hash[..8]))
        };
        Ok(version.unwrap_or_else(|| "UNKNOWN".to_string()))
    }
}

#[async_trait]
impl IOReadWriteFile for ObjectStoreFile {
    async fn write(&self, data: Bytes) -> Result<(), BundlebaseError> {
        if self.url.scheme() == EMPTY_SCHEME {
            return Err(format!("Cannot write to {}:// URL: {}", EMPTY_SCHEME, self.url).into());
        }

        let put_result = object_store::PutPayload::from_bytes(data);
        self.store.put(&self.path, put_result).await?;
        Ok(())
    }

    async fn write_stream(
        &self,
        mut source: BoxStream<'static, Result<Bytes, std::io::Error>>,
    ) -> Result<(), BundlebaseError> {
        if self.url.scheme() == EMPTY_SCHEME {
            return Err(format!("Cannot write to {}:// URL: {}", EMPTY_SCHEME, self.url).into());
        }

        // TODO: actually stream it
        // Collect stream into a single buffer
        let mut buffer = Vec::new();
        while let Some(chunk_result) = source.next().await {
            let chunk = chunk_result?;
            buffer.extend_from_slice(&chunk);
        }

        let put_result = object_store::PutPayload::from_bytes(Bytes::from(buffer));
        self.store.put(&self.path, put_result).await?;
        Ok(())
    }

    async fn delete(&self) -> Result<(), BundlebaseError> {
        match self.store.delete(&self.path).await {
            Ok(_) => Ok(()),
            Err(e) => {
                if matches!(e, object_store::Error::NotFound { .. }) {
                    Ok(())
                } else {
                    Err(Box::new(e))
                }
            }
        }
    }
}

impl Display for ObjectStoreFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "IOFile({})", self.url)
    }
}

// ============================================================================
// IODir - Directory abstraction for listing files and navigating subdirectories
// ============================================================================

/// Directory abstraction for listing files and navigating subdirectories.
#[derive(Clone)]
pub struct ObjectStoreDir {
    url: Url,
    store: Arc<dyn ObjectStore>,
    path: ObjectPath,
    config: Arc<dyn ConfigProvider>,
}

impl Debug for ObjectStoreDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IODir")
            .field("url", &self.url)
            .field("path", &self.path)
            .finish()
    }
}

impl ObjectStoreDir {
    /// Create an IODir from a URL.
    pub fn from_url(
        url: &Url,
        config: Arc<dyn ConfigProvider>,
    ) -> Result<ObjectStoreDir, BundlebaseError> {
        if url.scheme() == "memory" && !url.authority().is_empty() {
            return Err("Memory URL must be memory:///<path>".into());
        }
        if url.scheme() == EMPTY_SCHEME && !url.authority().is_empty() {
            return Err(format!("Empty URL must be {}<path>", EMPTY_URL).into());
        }

        let (store, path) = parse_url(url, &config)?;

        ObjectStoreDir::new(url, store, &path, config)
    }

    /// Creates a directory from the passed string.
    /// The string can be either a URL or a filesystem path (relative or absolute).
    pub fn from_str(
        path: &str,
        config: Arc<dyn ConfigProvider>,
    ) -> Result<ObjectStoreDir, BundlebaseError> {
        let url = str_to_url(path)?;
        Self::from_url(&url, config)
    }

    /// Create an IODir directly with all components.
    pub fn new(
        url: &Url,
        store: Arc<dyn ObjectStore>,
        path: &ObjectPath,
        config: Arc<dyn ConfigProvider>,
    ) -> Result<Self, BundlebaseError> {
        Ok(Self {
            url: url.clone(),
            store,
            path: path.clone(),
            config,
        })
    }

    /// Get an IOFile for a path within this directory.
    pub fn io_file(&self, path: &str) -> Result<ObjectStoreFile, BundlebaseError> {
        let file_url = join_url(&self.url, path)?;
        let object_path = join_path(&self.path, path)?;

        // Reuse the existing store instead of creating a new one
        // This is important for stores like TarObjectStore where the URL might not
        // indicate the store type
        ObjectStoreFile::new(
            &file_url,
            self.store.clone(),
            &object_path,
            self.config.clone(),
        )
    }

    /// Get an IODir for a subdirectory within this directory.
    pub fn io_subdir(&self, subdir: &str) -> Result<ObjectStoreDir, BundlebaseError> {
        Ok(ObjectStoreDir {
            url: join_url(&self.url, subdir)?,
            store: self.store.clone(),
            path: join_path(&self.path, subdir)?,
            config: self.config.clone(),
        })
    }
}

#[async_trait]
impl IOReadDir for ObjectStoreDir {
    fn url(&self) -> &Url {
        &self.url
    }

    async fn list_files(&self) -> Result<Vec<FileInfo>, BundlebaseError> {
        let mut files = Vec::new();
        let mut list_iter = self.store.list(Some(&self.path));

        while let Some(meta_result) = list_iter.next().await {
            let meta = meta_result?;
            let location = meta.location;
            // Get the relative path from self.path to location by stripping the prefix
            let location_str = location.as_ref();
            let prefix_str = self.path.as_ref();
            let relative_path = if let Some(stripped) = location_str.strip_prefix(prefix_str) {
                stripped.trim_start_matches('/')
            } else {
                location_str
            };

            let file_url = join_url(&self.url, relative_path)?;
            files.push(
                FileInfo::new(file_url)
                    .with_size(meta.size)
                    .with_modified(meta.last_modified),
            );
        }
        Ok(files)
    }

    fn subdir(&self, name: &str) -> Result<Box<dyn IOReadDir>, BundlebaseError> {
        Ok(Box::new(self.io_subdir(name)?))
    }

    fn file(&self, name: &str) -> Result<Box<dyn IOReadFile>, BundlebaseError> {
        Ok(Box::new(self.io_file(name)?))
    }
}

#[async_trait]
impl IOReadWriteDir for ObjectStoreDir {
    fn writable_subdir(&self, name: &str) -> Result<Box<dyn IOReadWriteDir>, BundlebaseError> {
        Ok(Box::new(self.io_subdir(name)?))
    }

    fn writable_file(&self, name: &str) -> Result<Box<dyn IOReadWriteFile>, BundlebaseError> {
        Ok(Box::new(self.io_file(name)?))
    }

    async fn rename(&self, from: &str, to: &str) -> Result<(), BundlebaseError> {
        let from_path = join_path(&self.path, from)?;
        let to_path = join_path(&self.path, to)?;

        // Use native rename - atomic on local filesystem, efficient on cloud
        self.store.rename(&from_path, &to_path).await?;
        Ok(())
    }
}

impl Display for ObjectStoreDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.url)
    }
}

pub(crate) fn str_to_url(path: &str) -> Result<Url, BundlebaseError> {
    if path.contains(":") {
        Ok(Url::parse(path)?)
    } else {
        file_url(path)
    }
}

/// Returns a URL for a file path.
/// If the path is relative, returns an absolute file URL relative to the current working directory.
/// `..` and `.` segments are resolved lexically so the resulting URL does not contain
/// path-traversal segments that downstream parsers (e.g. object_store) reject.
fn file_url(path: &str) -> Result<Url, BundlebaseError> {
    let path_buf = PathBuf::from(path);
    let absolute_path = if path_buf.is_absolute() {
        path_buf
    } else {
        current_dir()
            .map_err(|e| BundlebaseError::from(format!("Failed to get current directory: {}", e)))?
            .join(path_buf)
    };

    let normalized = normalize_path(&absolute_path);

    Url::from_file_path(normalized.as_path())
        .map_err(|_| BundlebaseError::from(format!("Invalid file path: {}", path)))
}

/// Returns the git blob OID of a `file://` URL's target if and only if the
/// file is a clean tracked file in a git working tree. Returns `None` for
/// any other scheme, or when git can't produce a confident answer.
///
/// Used by `version()` so that local files inside a git checkout get a
/// stable, content-addressed change-detection token instead of a
/// last-modified-time hash. The xxh3 content hash recorded in attach ops is
/// untouched — only the `version` change-detection field is affected.
/// Returns `true` if the `system.git_versioning` config is set to `"true"`.
fn is_git_versioning_enabled(config: &dyn ConfigProvider) -> Result<bool, BundlebaseError> {
    use bundlebase_common::system_config::GIT_VERSIONING_CFG;
    let value = config.get(&GIT_VERSIONING_CFG)?;
    Ok(value.as_deref() == Some("true"))
}

async fn git_oid_for_url(url: &Url) -> Option<String> {
    if url.scheme() != "file" {
        return None;
    }
    let path = url.to_file_path().ok()?;
    crate::plugin::git_version::working_tree_oid(&path).await
}

/// Resolve `.` and `..` components in `path` lexically (without touching the
/// filesystem). The path may not exist yet, so `std::fs::canonicalize` isn't
/// usable.
fn normalize_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

// ============================================================================
// ObjectStoreIOFactory - Factory for creating ObjectStore-backed IO instances
// ============================================================================

/// Factory for object_store-backed URLs (file://, s3://, gs://, azure://, memory://, empty://).
pub struct ObjectStoreIOFactory;

#[async_trait]
impl IOFactory for ObjectStoreIOFactory {
    fn schemes(&self) -> &[&str] {
        &["file", "s3", "gs", "azure", "az", "memory", "empty"]
    }

    fn supports_write(&self, url: &Url) -> bool {
        // empty:// is read-only
        url.scheme() != "empty"
    }

    async fn create_reader(
        &self,
        url: &Url,
        config: Arc<dyn ConfigProvider>,
    ) -> Result<Box<dyn IOReadFile>, BundlebaseError> {
        Ok(Box::new(ObjectStoreFile::from_url(url, config)?))
    }

    async fn create_lister(
        &self,
        url: &Url,
        config: Arc<dyn ConfigProvider>,
    ) -> Result<Box<dyn IOReadDir>, BundlebaseError> {
        Ok(Box::new(ObjectStoreDir::from_url(url, config)?))
    }

    async fn create_writable_lister(
        &self,
        url: &Url,
        config: Arc<dyn ConfigProvider>,
    ) -> Result<Option<Box<dyn IOReadWriteDir>>, BundlebaseError> {
        Ok(Some(Box::new(ObjectStoreDir::from_url(url, config)?)))
    }

    async fn create_writer(
        &self,
        url: &Url,
        config: Arc<dyn ConfigProvider>,
    ) -> Result<Option<Box<dyn IOReadWriteFile>>, BundlebaseError> {
        // empty:// is read-only
        if url.scheme() == "empty" {
            return Ok(None);
        }
        Ok(Some(Box::new(ObjectStoreFile::from_url(url, config)?)))
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::random_memory_file;
    use rstest::rstest;

    #[tokio::test]
    async fn test_read_write() {
        let file = random_memory_file("test.json");
        // Convert to IOFile
        let io_file =
            ObjectStoreFile::from_url(file.url(), crate::test_utils::test_config()).unwrap();

        assert!(!io_file.exists().await.unwrap());

        io_file.write(Bytes::from("hello world")).await.unwrap();
        assert_eq!(
            Some(Bytes::from("hello world")),
            io_file.read_bytes().await.unwrap()
        );
    }

    #[tokio::test]
    async fn test_null() {
        let file = ObjectStoreFile::from_url(
            &Url::parse("empty:///test.json").unwrap(),
            crate::test_utils::test_config(),
        )
        .unwrap();
        assert!(!file.exists().await.unwrap());
        assert!(file.write(Bytes::from("hello world")).await.is_err());
    }

    // IODir tests

    #[rstest]
    #[case("memory:///test", "test")]
    #[case("memory:///test/", "test")]
    #[case("memory:///test/sub/dir", "test/sub/dir")]
    #[case("memory:///path//with///more/", "path/with/more")]
    #[case("file:///test", "test")]
    #[case("file:///test/sub/dir", "test/sub/dir")]
    #[case("s3://test", "")]
    #[case("s3://test/path/here", "path/here")]
    fn test_from_str(#[case] input: &str, #[case] expected_path: &str) {
        let dir = ObjectStoreDir::from_str(input, crate::test_utils::test_config()).unwrap();
        assert_eq!(dir.url.to_string(), input);
        assert_eq!(dir.path.to_string(), expected_path);
    }

    #[test]
    fn test_from_string_complex() {
        assert!(
            ObjectStoreDir::from_str("memory://bucket/test", crate::test_utils::test_config())
                .is_err(),
            "Memory must start with :///"
        );

        let dir =
            ObjectStoreDir::from_str("memory:///test/../test2", crate::test_utils::test_config())
                .unwrap();
        assert_eq!(dir.path.to_string(), "test2");
        assert_eq!(dir.url.to_string(), "memory:///test2");

        let dir =
            ObjectStoreDir::from_str("relative/path", crate::test_utils::test_config()).unwrap();
        assert_eq!(
            dir.url.to_string(),
            file_url("relative/path").unwrap().to_string()
        );
    }

    #[rstest]
    #[case("memory:///test", "subdir", "memory:///test/subdir", "test/subdir")]
    #[case("memory:///test", "/subdir", "memory:///test/subdir", "test/subdir")]
    #[case("memory:///test/", "subdir", "memory:///test/subdir", "test/subdir")]
    #[case("memory:///test/", "/subdir", "memory:///test/subdir", "test/subdir")]
    #[case(
        "memory:///test",
        "/nested/subdir/here",
        "memory:///test/nested/subdir/here",
        "test/nested/subdir/here"
    )]
    fn test_subdir(
        #[case] base: Url,
        #[case] subdir: &str,
        #[case] expected_url: Url,
        #[case] expected_path: &str,
    ) {
        let dir = ObjectStoreDir::from_url(&base, crate::test_utils::test_config()).unwrap();
        let subdir = dir.io_subdir(subdir).unwrap();
        assert_eq!(subdir.url, expected_url);
        assert_eq!(subdir.path.to_string(), expected_path);
    }

    #[test]
    fn test_file() {
        let dir =
            ObjectStoreDir::from_str("memory:///test", crate::test_utils::test_config()).unwrap();
        let file = dir.io_file("other").unwrap();
        assert_eq!(file.url().to_string(), "memory:///test/other");

        let subdir = dir.io_subdir("this/file.txt").unwrap();
        assert_eq!(subdir.url().to_string(), "memory:///test/this/file.txt");
    }

    #[tokio::test]
    async fn test_list_files() {
        let dir =
            ObjectStoreDir::from_str("memory:///test", crate::test_utils::test_config()).unwrap();
        assert_eq!(0, dir.list_files().await.unwrap().len())
    }

    #[tokio::test]
    async fn test_null_url() {
        let dir = ObjectStoreDir::from_str(EMPTY_URL, crate::test_utils::test_config()).unwrap();
        assert_eq!(0, dir.list_files().await.unwrap().len());
    }

    // Utility tests

    #[rstest]
    #[case("s3://bucket/path/to/dir", "s3://bucket/")]
    #[case("s3://bucket/path/to/dir", "s3://bucket/")]
    #[case("memory:///path/to/dir", "memory:///")]
    #[case("memory:///path/to/dir", "memory:///")]
    fn test_compute_store_url(#[case] url: &str, #[case] expected: &str) {
        let url = Url::parse(url).unwrap();
        assert_eq!(expected, compute_store_url(&url).as_str());
    }

    fn config_with_git_versioning() -> Arc<dyn ConfigProvider> {
        let cfg = crate::test_utils::TestConfigProvider::new();
        cfg.set("system", "git_versioning", "true");
        Arc::new(cfg)
    }

    #[tokio::test]
    async fn version_uses_git_oid_when_enabled_in_repo() {
        // With system.git_versioning=true and a file in a working tree,
        // version() returns the git blob OID (40-char sha1 hex).
        use std::process::Command as StdCommand;
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        StdCommand::new("git")
            .arg("-C")
            .arg(repo)
            .args(["init", "--quiet"])
            .status()
            .unwrap();
        let path = repo.join("data.csv");
        std::fs::write(&path, b"a,b\n1,2\n").unwrap();

        let url = Url::from_file_path(&path).unwrap();
        let file = ObjectStoreFile::from_url(&url, config_with_git_versioning()).unwrap();
        let version = file.version().await.unwrap();
        assert_eq!(
            version.len(),
            40,
            "expected 40-char sha1 hex, got {:?}",
            version
        );
        assert!(
            version.chars().all(|c| c.is_ascii_hexdigit()),
            "expected hex OID, got {:?}",
            version
        );

        // And it should match what `git hash-object` produces directly.
        let expected = StdCommand::new("git")
            .arg("-C")
            .arg(repo)
            .args(["hash-object", path.to_str().unwrap()])
            .output()
            .unwrap();
        let expected_oid = String::from_utf8(expected.stdout).unwrap().trim().to_string();
        assert_eq!(version, expected_oid);
    }

    #[tokio::test]
    async fn version_does_not_use_git_when_disabled() {
        // Default config does not enable git_versioning. Even if the file
        // is in a git working tree, the version must not be the git OID —
        // we verify by comparing against `git hash-object` and asserting
        // they differ (the fallback is an mtime-derived ETag/hash).
        use std::process::Command as StdCommand;
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        StdCommand::new("git")
            .arg("-C")
            .arg(repo)
            .args(["init", "--quiet"])
            .status()
            .unwrap();
        let path = repo.join("data.csv");
        std::fs::write(&path, b"a,b\n1,2\n").unwrap();

        let url = Url::from_file_path(&path).unwrap();
        let file = ObjectStoreFile::from_url(&url, crate::test_utils::test_config()).unwrap();
        let version = file.version().await.unwrap();

        let oid_output = StdCommand::new("git")
            .arg("-C")
            .arg(repo)
            .args(["hash-object", path.to_str().unwrap()])
            .output()
            .unwrap();
        let oid = String::from_utf8(oid_output.stdout).unwrap().trim().to_string();
        assert_ne!(
            version, oid,
            "git lookup must not happen when system.git_versioning is off"
        );
    }

    #[tokio::test]
    async fn version_errors_when_enabled_but_outside_repo() {
        // Enabled, but the file is outside any working tree. The user
        // explicitly opted into git versioning, so a missing git context is
        // a fatal error rather than a silent fallback.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.csv");
        std::fs::write(&path, b"a,b\n1,2\n").unwrap();

        let url = Url::from_file_path(&path).unwrap();
        let file = ObjectStoreFile::from_url(&url, config_with_git_versioning()).unwrap();
        let err = file.version().await.expect_err("expected error");
        let msg = err.to_string();
        assert!(
            msg.contains("system.git_versioning"),
            "expected error to mention the config key, got {:?}",
            msg
        );
    }

    #[test]
    fn test_custom_scheme_registration() {
        use crate::get_memory_store;

        register_object_store_scheme("testscheme", |url, _config| {
            Ok((get_memory_store(), ObjectPath::from(url.path())))
        });

        let url = Url::parse("testscheme:///some/path").unwrap();
        let config = crate::test_utils::TestConfigProvider::new();
        let (store, path) = parse_url(&url, &config).unwrap();

        assert_eq!(path.as_ref(), "some/path");
        // Verify it returned the memory store (put + get round-trip)
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let test_path = ObjectPath::from("/custom_scheme_test");
            store
                .put(&test_path, object_store::PutPayload::from_static(b"hello"))
                .await
                .unwrap();
            let result = store.get(&test_path).await.unwrap();
            assert_eq!(result.bytes().await.unwrap().as_ref(), b"hello");
        });
    }
}
