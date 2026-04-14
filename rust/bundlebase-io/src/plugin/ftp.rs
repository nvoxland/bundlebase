//! FTP IO backend - read-only file and directory operations via FTP.
//!
//! Note: FTP and SFTP backends share similar patterns (connect → operate → close)
//! but use different underlying libraries (suppaftp vs russh_sftp) with incompatible
//! types. Extracting a common abstraction would add complexity without clear benefit.

use crate::registry::IOFactory;
use crate::{BundlebaseError, ConfigProvider};
use crate::{FileInfo, IOReadDir, IOReadFile, IOReadWriteFile};
use async_trait::async_trait;
use bundlebase_common::{config_keys, config_scopes, ConfigKey, ConfigScope};
use bytes::Bytes;
use futures::stream::BoxStream;
use log::debug;
use std::fmt::Debug;
use std::sync::Arc;
use suppaftp::tokio::AsyncFtpStream;
use suppaftp::types::FileType;
use tokio::io::AsyncReadExt;
use url::Url;

config_scopes!(ftp_scopes, {
    pub const FTP_SCOPE: ConfigScope = ConfigScope::new("ftp");
});

config_keys!(ftp_keys, {
    pub const FTP_USERNAME_CFG: ConfigKey = FTP_SCOPE.define("username");
    pub const FTP_PASSWORD_CFG: ConfigKey = FTP_SCOPE.define_secure("password");
});

/// Parse an FTP URL into its components.
///
/// # URL Format
/// `ftp://host[:port]/path`
///
/// Credentials are resolved from `BundleConfig`, not from the URL.
///
/// Examples:
/// - `ftp://ftp.example.com/pub/data`
/// - `ftp://ftp.example.com:2121/data`
///
/// # Returns
/// Tuple of (host, port, path)
pub fn parse_ftp_url(url: &Url) -> Result<(String, u16, String), BundlebaseError> {
    if url.scheme() != "ftp" {
        return Err(format!("Expected 'ftp' URL scheme, got '{}'", url.scheme()).into());
    }

    let host = url
        .host_str()
        .ok_or_else(|| BundlebaseError::from("FTP URL must include a host"))?;

    let port = url.port().unwrap_or(21);

    let path = url.path().to_string();
    if path.is_empty() || path == "/" {
        return Err("FTP URL must include a path".into());
    }

    Ok((host.to_string(), port, path))
}

/// Build an FTP URL from components
///
/// Constructs a URL in the format: `ftp://host:port/path`
fn build_ftp_url(host: &str, port: u16, path: &str) -> Result<Url, BundlebaseError> {
    Url::parse(&format!("ftp://{}:{}{}", host, port, path))
        .map_err(|e| format!("Failed to build FTP URL: {}", e).into())
}

/// FTP file reader - read-only access to a single FTP file.
#[derive(Clone)]
pub struct FtpFile {
    url: Url,
    host: String,
    port: u16,
    user: String,
    password: String,
    path: String,
}

impl Debug for FtpFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FtpFile")
            .field("url", &self.url)
            .field("path", &self.path)
            .finish()
    }
}

impl FtpFile {
    /// Download the file to a temp file by streaming from FTP, returning the temp file.
    pub async fn download_to_temp_file(
        &self,
    ) -> Result<Option<tempfile::NamedTempFile>, BundlebaseError> {
        let mut stream = self.connect().await?;

        let result = stream.retr_as_stream(&self.path).await;

        match result {
            Ok(mut data_stream) => {
                let temp = tempfile::NamedTempFile::new().map_err(|e| {
                    BundlebaseError::from(format!(
                        "Failed to create temp file for FTP download: {}",
                        e
                    ))
                })?;
                let mut async_temp = tokio::fs::File::from_std(temp.reopen().map_err(|e| {
                    BundlebaseError::from(format!(
                        "Failed to reopen temp file for FTP download: {}",
                        e
                    ))
                })?);
                tokio::io::copy(&mut data_stream, &mut async_temp)
                    .await
                    .map_err(|e| {
                        BundlebaseError::from(format!(
                            "Failed to download FTP file '{}': {}",
                            self.path, e
                        ))
                    })?;

                stream
                    .finalize_retr_stream(data_stream)
                    .await
                    .map_err(|e| {
                        BundlebaseError::from(format!(
                            "Failed to finalize FTP download for '{}': {}",
                            self.path, e
                        ))
                    })?;
                if let Err(e) = stream.quit().await {
                    debug!("Error closing FTP connection: {}", e);
                }
                Ok(Some(temp))
            }
            Err(e) => {
                if let Err(e) = stream.quit().await {
                    debug!("Error closing FTP connection: {}", e);
                }
                let err_str = e.to_string().to_lowercase();
                if err_str.contains("550")
                    || err_str.contains("not found")
                    || err_str.contains("no such file")
                {
                    Ok(None)
                } else {
                    Err(format!("Failed to download FTP file '{}': {}", self.path, e).into())
                }
            }
        }
    }

    /// Create an FtpFile from a URL, resolving credentials from config.
    pub fn from_url(url: &Url, config: Arc<dyn ConfigProvider>) -> Result<Self, BundlebaseError> {
        let (host, port, path) = parse_ftp_url(url)?;
        let scope = bundlebase_common::Scope::try_from(url)?;
        let user = config
            .get(&scope, &FTP_USERNAME_CFG)?
            .unwrap_or_else(|| "anonymous".to_string());
        let password = config.get(&scope, &FTP_PASSWORD_CFG)?.unwrap_or_default();
        Ok(Self {
            url: url.clone(),
            host,
            port,
            user,
            password,
            path,
        })
    }

    async fn connect(&self) -> Result<AsyncFtpStream, BundlebaseError> {
        let mut stream = AsyncFtpStream::connect(format!("{}:{}", self.host, self.port))
            .await
            .map_err(|e| {
                BundlebaseError::from(format!(
                    "Failed to connect to FTP server {}:{}: {}",
                    self.host, self.port, e
                ))
            })?;

        stream
            .login(&self.user, &self.password)
            .await
            .map_err(|e| {
                BundlebaseError::from(format!(
                    "FTP authentication failed for user '{}': {}",
                    self.user, e
                ))
            })?;

        stream
            .transfer_type(FileType::Binary)
            .await
            .map_err(|e| BundlebaseError::from(format!("Failed to set FTP binary mode: {}", e)))?;

        Ok(stream)
    }
}

#[async_trait]
impl IOReadFile for FtpFile {
    fn url(&self) -> &Url {
        &self.url
    }

    async fn exists(&self) -> Result<bool, BundlebaseError> {
        let mut stream = self.connect().await?;
        let result = stream.size(&self.path).await;
        if let Err(e) = stream.quit().await {
            debug!("Error closing FTP connection: {}", e);
        }
        Ok(result.is_ok())
    }

    async fn open_stream(
        &self,
    ) -> Result<Option<BoxStream<'static, Result<Bytes, BundlebaseError>>>, BundlebaseError> {
        match self.download_to_temp_file().await? {
            Some(temp) => {
                let stream = crate::util::stream_from_temp_file(temp);
                Ok(Some(Box::pin(futures::StreamExt::map(stream, |r| {
                    r.map_err(|e| BundlebaseError::from(format!("Failed to read temp file: {}", e)))
                }))))
            }
            None => Ok(None),
        }
    }

    async fn metadata(&self) -> Result<Option<FileInfo>, BundlebaseError> {
        let mut stream = self.connect().await?;
        match stream.size(&self.path).await {
            Ok(size) => {
                if let Err(e) = stream.quit().await {
                    debug!("Error closing FTP connection: {}", e);
                }
                Ok(Some(FileInfo::new(self.url.clone()).with_size(size as u64)))
            }
            Err(_) => {
                if let Err(e) = stream.quit().await {
                    debug!("Error closing FTP connection: {}", e);
                }
                Ok(None)
            }
        }
    }

    async fn version(&self) -> Result<String, BundlebaseError> {
        // FTP doesn't have native versioning, use size as a simple version
        let mut stream = self.connect().await?;
        match stream.size(&self.path).await {
            Ok(size) => {
                if let Err(e) = stream.quit().await {
                    debug!("Error closing FTP connection: {}", e);
                }
                Ok(format!("size-{}", size))
            }
            Err(e) => {
                if let Err(e) = stream.quit().await {
                    debug!("Error closing FTP connection: {}", e);
                }
                Err(format!("Failed to get FTP file version: {}", e).into())
            }
        }
    }
}

/// FTP directory lister - read-only access to list FTP directories.
#[derive(Clone)]
pub struct FtpDir {
    url: Url,
    host: String,
    port: u16,
    user: String,
    password: String,
    path: String,
}

impl Debug for FtpDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FtpDir")
            .field("url", &self.url)
            .field("path", &self.path)
            .finish()
    }
}

impl FtpDir {
    /// Create an FtpDir from a URL, resolving credentials from config.
    pub fn from_url(url: &Url, config: Arc<dyn ConfigProvider>) -> Result<Self, BundlebaseError> {
        let (host, port, path) = parse_ftp_url(url)?;
        let scope = bundlebase_common::Scope::try_from(url)?;
        let user = config
            .get(&scope, &FTP_USERNAME_CFG)?
            .unwrap_or_else(|| "anonymous".to_string());
        let password = config.get(&scope, &FTP_PASSWORD_CFG)?.unwrap_or_default();
        Ok(Self {
            url: url.clone(),
            host,
            port,
            user,
            password,
            path,
        })
    }

    async fn connect(&self) -> Result<AsyncFtpStream, BundlebaseError> {
        let mut stream = AsyncFtpStream::connect(format!("{}:{}", self.host, self.port))
            .await
            .map_err(|e| {
                BundlebaseError::from(format!(
                    "Failed to connect to FTP server {}:{}: {}",
                    self.host, self.port, e
                ))
            })?;

        stream
            .login(&self.user, &self.password)
            .await
            .map_err(|e| {
                BundlebaseError::from(format!(
                    "FTP authentication failed for user '{}': {}",
                    self.user, e
                ))
            })?;

        stream
            .transfer_type(FileType::Binary)
            .await
            .map_err(|e| BundlebaseError::from(format!("Failed to set FTP binary mode: {}", e)))?;

        Ok(stream)
    }

    async fn list_files_recursive_internal(
        &self,
        stream: &mut AsyncFtpStream,
        path: &str,
        files: &mut Vec<FileInfo>,
    ) -> Result<(), BundlebaseError> {
        let entries: Vec<String> = stream.nlst(Some(path)).await.map_err(|e| {
            BundlebaseError::from(format!("Failed to list FTP directory '{}': {}", path, e))
        })?;

        for entry_name in entries {
            let filename: &str = entry_name.rsplit('/').next().unwrap_or(&entry_name);
            if filename == "." || filename == ".." || filename.is_empty() {
                continue;
            }

            let full_path = if path.ends_with('/') {
                format!("{}{}", path, filename)
            } else {
                format!("{}/{}", path, filename)
            };

            // Try to get the size to determine if it's a file
            match stream.size(&full_path).await {
                Ok(size) => {
                    let url = build_ftp_url(&self.host, self.port, &full_path)?;
                    files.push(FileInfo::new(url).with_size(size as u64));
                }
                Err(_) => {
                    // Might be a directory, try to list it
                    let sub_result: Result<Vec<String>, _> = stream.nlst(Some(&full_path)).await;
                    if let Ok(sub_entries) = sub_result {
                        if !sub_entries.is_empty() {
                            Box::pin(self.list_files_recursive_internal(stream, &full_path, files))
                                .await?;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

#[async_trait]
impl IOReadDir for FtpDir {
    fn url(&self) -> &Url {
        &self.url
    }

    async fn list_files(&self) -> Result<Vec<FileInfo>, BundlebaseError> {
        let mut stream = self.connect().await?;
        let mut files = Vec::new();
        self.list_files_recursive_internal(&mut stream, &self.path, &mut files)
            .await?;
        if let Err(e) = stream.quit().await {
            debug!("Error closing FTP connection: {}", e);
        }
        Ok(files)
    }

    fn subdir(&self, name: &str) -> Result<Box<dyn IOReadDir>, BundlebaseError> {
        let new_path = if self.path.ends_with('/') {
            format!("{}{}", self.path, name.trim_start_matches('/'))
        } else {
            format!("{}/{}", self.path, name.trim_start_matches('/'))
        };

        let new_url = build_ftp_url(&self.host, self.port, &new_path)?;

        Ok(Box::new(FtpDir {
            url: new_url,
            host: self.host.clone(),
            port: self.port,
            user: self.user.clone(),
            password: self.password.clone(),
            path: new_path,
        }))
    }

    fn file(&self, name: &str) -> Result<Box<dyn IOReadFile>, BundlebaseError> {
        let new_path = if self.path.ends_with('/') {
            format!("{}{}", self.path, name.trim_start_matches('/'))
        } else {
            format!("{}/{}", self.path, name.trim_start_matches('/'))
        };

        let new_url = build_ftp_url(&self.host, self.port, &new_path)?;

        Ok(Box::new(FtpFile {
            url: new_url,
            host: self.host.clone(),
            port: self.port,
            user: self.user.clone(),
            password: self.password.clone(),
            path: new_path,
        }))
    }
}

/// Factory for FTP IO backends.
pub struct FtpIOFactory;

#[async_trait]
impl IOFactory for FtpIOFactory {
    fn schemes(&self) -> &[&str] {
        &["ftp"]
    }

    fn supports_write(&self, _url: &Url) -> bool {
        false // FTP is read-only in this implementation
    }

    fn supports_streaming_read(&self) -> bool {
        true
    }

    fn supports_versioning(&self) -> bool {
        // FTP uses file size as a synthetic version, not native versioning
        false
    }

    async fn create_reader(
        &self,
        url: &Url,
        config: Arc<dyn ConfigProvider>,
    ) -> Result<Box<dyn IOReadFile>, BundlebaseError> {
        Ok(Box::new(FtpFile::from_url(url, config)?))
    }

    async fn create_lister(
        &self,
        url: &Url,
        config: Arc<dyn ConfigProvider>,
    ) -> Result<Box<dyn IOReadDir>, BundlebaseError> {
        Ok(Box::new(FtpDir::from_url(url, config)?))
    }

    async fn create_writer(
        &self,
        _url: &Url,
        _config: Arc<dyn ConfigProvider>,
    ) -> Result<Option<Box<dyn IOReadWriteFile>>, BundlebaseError> {
        Ok(None) // Read-only
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ftp_url_basic() {
        let url = Url::parse("ftp://ftp.example.com:2121/data/files").unwrap();
        let (host, port, path) = parse_ftp_url(&url).unwrap();
        assert_eq!(host, "ftp.example.com");
        assert_eq!(port, 2121);
        assert_eq!(path, "/data/files");
    }

    #[test]
    fn test_parse_ftp_url_default_port() {
        let url = Url::parse("ftp://ftp.example.com/pub/data").unwrap();
        let (host, port, path) = parse_ftp_url(&url).unwrap();
        assert_eq!(host, "ftp.example.com");
        assert_eq!(port, 21);
        assert_eq!(path, "/pub/data");
    }

    #[test]
    fn test_parse_ftp_url_wrong_scheme() {
        let url = Url::parse("http://example.com/data").unwrap();
        let result = parse_ftp_url(&url);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Expected 'ftp'"));
    }

    #[test]
    fn test_parse_ftp_url_missing_path() {
        let url = Url::parse("ftp://ftp.example.com").unwrap();
        let result = parse_ftp_url(&url);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must include a path"));
    }

    #[test]
    fn test_build_ftp_url() {
        let url = build_ftp_url("example.com", 21, "/data").unwrap();
        assert_eq!(url.scheme(), "ftp");
        assert_eq!(url.host_str(), Some("example.com"));
        assert_eq!(url.port_or_known_default(), Some(21));
        assert_eq!(url.path(), "/data");
        // Credentials should not be in the URL
        assert!(url.username().is_empty());
        assert_eq!(url.password(), None);
    }

    #[test]
    fn test_build_ftp_url_custom_port() {
        let url = build_ftp_url("example.com", 2121, "/data").unwrap();
        assert_eq!(url.port(), Some(2121));
    }

    #[test]
    fn test_ftp_factory_schemes() {
        let factory = FtpIOFactory;
        let schemes = factory.schemes();
        assert_eq!(schemes, &["ftp"]);
    }

    #[test]
    fn test_ftp_factory_supports_write_returns_false() {
        let factory = FtpIOFactory;
        let url = Url::parse("ftp://ftp.example.com/data").unwrap();
        assert!(!factory.supports_write(&url));
    }

    #[test]
    fn test_ftp_file_from_url_with_config() {
        let config = crate::test_utils::test_config_with_values(&[
            ("ftp", FTP_USERNAME_CFG.key, "testuser"),
            ("ftp", FTP_PASSWORD_CFG.key, "testpass"),
        ]);

        let url = Url::parse("ftp://ftp.example.com:2121/data/file.txt").unwrap();
        let ftp_file = FtpFile::from_url(&url, config).unwrap();
        assert_eq!(ftp_file.host, "ftp.example.com");
        assert_eq!(ftp_file.port, 2121);
        assert_eq!(ftp_file.user, "testuser");
        assert_eq!(ftp_file.password, "testpass");
        assert_eq!(ftp_file.path, "/data/file.txt");
    }

    #[test]
    fn test_ftp_file_from_url_anonymous_default() {
        let config = crate::test_utils::test_config();
        let url = Url::parse("ftp://ftp.example.com/pub/data/file.txt").unwrap();
        let ftp_file = FtpFile::from_url(&url, config).unwrap();
        assert_eq!(ftp_file.user, "anonymous");
        assert_eq!(ftp_file.password, "");
    }

    #[test]
    fn test_ftp_dir_from_url_with_config() {
        let config = crate::test_utils::test_config_with_values(&[
            ("ftp", FTP_USERNAME_CFG.key, "myuser"),
            ("ftp", FTP_PASSWORD_CFG.key, "mypass"),
        ]);

        let url = Url::parse("ftp://ftp.example.com/pub/data/").unwrap();
        let ftp_dir = FtpDir::from_url(&url, config).unwrap();
        assert_eq!(ftp_dir.host, "ftp.example.com");
        assert_eq!(ftp_dir.port, 21);
        assert_eq!(ftp_dir.user, "myuser");
        assert_eq!(ftp_dir.password, "mypass");
        assert_eq!(ftp_dir.path, "/pub/data/");
    }

    #[test]
    fn test_ftp_dir_from_url_anonymous_default() {
        let config = crate::test_utils::test_config();
        let url = Url::parse("ftp://ftp.example.com/pub/data/").unwrap();
        let ftp_dir = FtpDir::from_url(&url, config).unwrap();
        assert_eq!(ftp_dir.user, "anonymous");
        assert_eq!(ftp_dir.password, "");
    }
}
