//! SFTP IO backend - read-only file and directory operations via SSH/SFTP.
//!
//! Note: FTP and SFTP backends share similar patterns (connect → operate → close)
//! but use different underlying libraries (suppaftp vs russh_sftp) with incompatible
//! types. Extracting a common abstraction would add complexity without clear benefit.

use crate::io::registry::IOFactory;
use crate::io::{FileInfo, IOReadDir, IOReadFile, IOReadWriteFile};
use crate::BundleConfig;
use crate::BundlebaseError;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use russh::client::{self, Handle};
use russh_keys::key::PublicKey;
use russh_keys::load_secret_key;
use russh_sftp::client::SftpSession;
use std::fmt::Debug;
use std::path::Path;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use url::Url;

/// SSH client handler for russh.
struct SshHandler;

#[async_trait::async_trait]
impl client::Handler for SshHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        // Accept all host keys for now
        // TODO: Add known_hosts verification support
        Ok(true)
    }
}

/// Parse an SFTP URL into its components.
///
/// # URL Format
/// `sftp://[user@]host[:port]/path`
///
/// # Returns
/// Tuple of (user, host, port, path)
pub fn parse_sftp_url(url: &Url) -> Result<(String, String, u16, String), BundlebaseError> {
    let scheme = url.scheme();
    if scheme != "sftp" {
        return Err(format!("Expected 'sftp' URL scheme, got '{}'", scheme).into());
    }

    let host = url
        .host_str()
        .ok_or_else(|| BundlebaseError::from("SFTP URL must include a host"))?;

    let port = url.port().unwrap_or(22);

    let user = if url.username().is_empty() {
        std::env::var("USER").unwrap_or_else(|_| "root".to_string())
    } else {
        url.username().to_string()
    };

    let path = url.path().to_string();
    if path.is_empty() || path == "/" {
        return Err("SFTP URL must include a path".into());
    }

    Ok((user, host.to_string(), port, path))
}

/// Build an SFTP URL from components.
///
/// Constructs a URL in the format: `sftp://user@host:port/path`
fn build_sftp_url(user: &str, host: &str, port: u16, path: &str) -> Result<Url, BundlebaseError> {
    Url::parse(&format!(
        "sftp://{}@{}:{}{}",
        user, host, port, path
    )).map_err(|e| format!("Failed to build SFTP URL: {}", e).into())
}

/// SFTP client for SSH file operations.
pub struct SftpClient {
    session: Handle<SshHandler>,
    sftp: SftpSession,
}

impl SftpClient {
    /// Connect to an SSH host with key authentication.
    pub async fn connect(
        host: &str,
        port: u16,
        user: &str,
        key_path: &Path,
    ) -> Result<Self, BundlebaseError> {
        let key = load_secret_key(key_path, None).map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to load SSH key from '{}': {}",
                key_path.display(),
                e
            ))
        })?;

        let config = Arc::new(client::Config::default());

        let mut session = client::connect(config, (host, port), SshHandler)
            .await
            .map_err(|e| {
                BundlebaseError::from(format!(
                    "Failed to connect to SSH server {}:{}: {}",
                    host, port, e
                ))
            })?;

        let auth_success = session
            .authenticate_publickey(user, Arc::new(key))
            .await
            .map_err(|e| {
                BundlebaseError::from(format!(
                    "SSH authentication failed for user '{}': {}",
                    user, e
                ))
            })?;

        if !auth_success {
            return Err(BundlebaseError::from(format!(
                "SSH authentication failed for user '{}': public key not accepted",
                user
            )));
        }

        let channel = session.channel_open_session().await.map_err(|e| {
            BundlebaseError::from(format!("Failed to open SSH channel: {}", e))
        })?;

        channel.request_subsystem(true, "sftp").await.map_err(|e| {
            BundlebaseError::from(format!("Failed to request SFTP subsystem: {}", e))
        })?;

        let sftp = SftpSession::new(channel.into_stream()).await.map_err(|e| {
            BundlebaseError::from(format!("Failed to initialize SFTP session: {}", e))
        })?;

        Ok(Self { session, sftp })
    }

    /// List all files in a remote directory recursively.
    pub async fn list_files_recursive(
        &self,
        path: &str,
        user: &str,
        host: &str,
        port: u16,
    ) -> Result<Vec<FileInfo>, BundlebaseError> {
        let mut all_files = Vec::new();
        self.list_dir_recursive_inner(path, user, host, port, &mut all_files).await?;
        Ok(all_files)
    }

    async fn list_dir_recursive_inner(
        &self,
        path: &str,
        user: &str,
        host: &str,
        port: u16,
        files: &mut Vec<FileInfo>,
    ) -> Result<(), BundlebaseError> {
        let entries = self.sftp.read_dir(path).await.map_err(|e| {
            BundlebaseError::from(format!("Failed to list directory '{}': {}", path, e))
        })?;

        for entry in entries {
            let file_name = entry.file_name();

            if file_name == "." || file_name == ".." {
                continue;
            }

            let full_path = if path.ends_with('/') {
                format!("{}{}", path, file_name)
            } else {
                format!("{}/{}", path, file_name)
            };

            let file_type = entry.file_type();
            let is_dir = file_type.is_dir();
            let size = entry.metadata().size.unwrap_or(0);

            if is_dir {
                Box::pin(self.list_dir_recursive_inner(&full_path, user, host, port, files)).await?;
            } else {
                let url = build_sftp_url(user, host, port, &full_path)?;
                files.push(FileInfo::new(url).with_size(size));
            }
        }

        Ok(())
    }

    /// Read file contents from the remote server.
    pub async fn read_file(&self, path: &str) -> Result<Bytes, BundlebaseError> {
        let mut file = self.sftp.open(path).await.map_err(|e| {
            BundlebaseError::from(format!("Failed to open remote file '{}': {}", path, e))
        })?;

        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).await.map_err(|e| {
            BundlebaseError::from(format!("Failed to read from '{}': {}", path, e))
        })?;

        Ok(Bytes::from(buffer))
    }

    /// Close the SFTP session and SSH connection.
    pub async fn close(self) -> Result<(), BundlebaseError> {
        self.sftp.close().await.map_err(|e| {
            BundlebaseError::from(format!("Failed to close SFTP session: {}", e))
        })?;

        self.session
            .disconnect(russh::Disconnect::ByApplication, "", "en")
            .await
            .map_err(|e| {
                BundlebaseError::from(format!("Failed to disconnect SSH session: {}", e))
            })?;

        Ok(())
    }
}

/// SFTP file reader - read-only access to a single SFTP file.
#[derive(Clone)]
pub struct SftpFile {
    url: Url,
    host: String,
    port: u16,
    user: String,
    key_path: String,
    path: String,
}

impl Debug for SftpFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SftpFile")
            .field("url", &self.url)
            .field("path", &self.path)
            .finish()
    }
}

impl SftpFile {
    /// Create an SftpFile from a URL and configuration.
    pub fn from_url(url: &Url, config: Arc<BundleConfig>) -> Result<Self, BundlebaseError> {
        let (user, host, port, path) = parse_sftp_url(url)?;

        // Get key_path from config, env var, or default to ~/.ssh/id_rsa
        let key_path = config
            .get_config_for_url(url)
            .get("key_path")
            .cloned()
            .or_else(|| std::env::var("SSH_KEY_PATH").ok())
            .unwrap_or_else(|| "~/.ssh/id_rsa".to_string());

        Ok(Self {
            url: url.clone(),
            host,
            port,
            user,
            key_path,
            path,
        })
    }

    async fn connect(&self) -> Result<SftpClient, BundlebaseError> {
        let key_path_expanded = shellexpand::tilde(&self.key_path).to_string();
        SftpClient::connect(
            &self.host,
            self.port,
            &self.user,
            Path::new(&key_path_expanded),
        )
        .await
    }
}

#[async_trait]
impl IOReadFile for SftpFile {
    fn url(&self) -> &Url {
        &self.url
    }

    async fn exists(&self) -> Result<bool, BundlebaseError> {
        let client = self.connect().await?;
        let result = client.sftp.metadata(&self.path).await;
        client.close().await?;
        Ok(result.is_ok())
    }

    async fn read_bytes(&self) -> Result<Option<Bytes>, BundlebaseError> {
        let client = self.connect().await?;
        match client.read_file(&self.path).await {
            Ok(data) => {
                client.close().await?;
                Ok(Some(data))
            }
            Err(e) => {
                client.close().await?;
                // Check if it's a file not found error
                // SFTP uses SSH_FX_NO_SUCH_FILE (code 2) for missing files
                // We check common error message patterns as a fallback
                let err_str = e.to_string().to_lowercase();
                if err_str.contains("no such file") || err_str.contains("not found") || err_str.contains("error code: 2") {
                    Ok(None)
                } else {
                    Err(e)
                }
            }
        }
    }

    async fn read_stream(
        &self,
    ) -> Result<Option<BoxStream<'static, Result<Bytes, BundlebaseError>>>, BundlebaseError> {
        match self.read_bytes().await? {
            Some(bytes) => {
                let stream = futures::stream::once(async move { Ok(bytes) });
                Ok(Some(Box::pin(stream)))
            }
            None => Ok(None),
        }
    }

    async fn metadata(&self) -> Result<Option<FileInfo>, BundlebaseError> {
        let client = self.connect().await?;
        match client.sftp.metadata(&self.path).await {
            Ok(meta) => {
                client.close().await?;
                Ok(Some(
                    FileInfo::new(self.url.clone()).with_size(meta.size.unwrap_or(0)),
                ))
            }
            Err(_) => {
                client.close().await?;
                Ok(None)
            }
        }
    }

    async fn version(&self) -> Result<String, BundlebaseError> {
        let client = self.connect().await?;
        match client.sftp.metadata(&self.path).await {
            Ok(meta) => {
                client.close().await?;
                Ok(format!("size-{}", meta.size.unwrap_or(0)))
            }
            Err(e) => {
                client.close().await?;
                Err(format!("Failed to get SFTP file version: {}", e).into())
            }
        }
    }
}

/// SFTP directory lister - read-only access to list SFTP directories.
#[derive(Clone)]
pub struct SftpDir {
    url: Url,
    host: String,
    port: u16,
    user: String,
    key_path: String,
    path: String,
}

impl Debug for SftpDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SftpDir")
            .field("url", &self.url)
            .field("path", &self.path)
            .finish()
    }
}

impl SftpDir {
    /// Create an SftpDir from a URL and configuration.
    pub fn from_url(url: &Url, config: Arc<BundleConfig>) -> Result<Self, BundlebaseError> {
        let (user, host, port, path) = parse_sftp_url(url)?;

        // Get key_path from config, env var, or default to ~/.ssh/id_rsa
        let key_path = config
            .get_config_for_url(url)
            .get("key_path")
            .cloned()
            .or_else(|| std::env::var("SSH_KEY_PATH").ok())
            .unwrap_or_else(|| "~/.ssh/id_rsa".to_string());

        Ok(Self {
            url: url.clone(),
            host,
            port,
            user,
            key_path,
            path,
        })
    }

    async fn connect(&self) -> Result<SftpClient, BundlebaseError> {
        let key_path_expanded = shellexpand::tilde(&self.key_path).to_string();
        SftpClient::connect(
            &self.host,
            self.port,
            &self.user,
            Path::new(&key_path_expanded),
        )
        .await
    }
}

#[async_trait]
impl IOReadDir for SftpDir {
    fn url(&self) -> &Url {
        &self.url
    }

    async fn list_files(&self) -> Result<Vec<FileInfo>, BundlebaseError> {
        let client = self.connect().await?;
        let files = client.list_files_recursive(&self.path, &self.user, &self.host, self.port).await?;
        client.close().await?;
        Ok(files)
    }

    fn subdir(&self, name: &str) -> Result<Box<dyn IOReadDir>, BundlebaseError> {
        let new_path = if self.path.ends_with('/') {
            format!("{}{}", self.path, name.trim_start_matches('/'))
        } else {
            format!("{}/{}", self.path, name.trim_start_matches('/'))
        };

        let new_url = build_sftp_url(&self.user, &self.host, self.port, &new_path)?;

        Ok(Box::new(SftpDir {
            url: new_url,
            host: self.host.clone(),
            port: self.port,
            user: self.user.clone(),
            key_path: self.key_path.clone(),
            path: new_path,
        }))
    }

    fn file(&self, name: &str) -> Result<Box<dyn IOReadFile>, BundlebaseError> {
        let new_path = if self.path.ends_with('/') {
            format!("{}{}", self.path, name.trim_start_matches('/'))
        } else {
            format!("{}/{}", self.path, name.trim_start_matches('/'))
        };

        let new_url = build_sftp_url(&self.user, &self.host, self.port, &new_path)?;

        Ok(Box::new(SftpFile {
            url: new_url,
            host: self.host.clone(),
            port: self.port,
            user: self.user.clone(),
            key_path: self.key_path.clone(),
            path: new_path,
        }))
    }
}

/// Factory for SFTP IO backends.
pub struct SftpIOFactory;

#[async_trait]
impl IOFactory for SftpIOFactory {
    fn schemes(&self) -> &[&str] {
        &["sftp"]
    }

    fn supports_write(&self, _url: &Url) -> bool {
        false // SFTP is read-only in this implementation
    }

    fn supports_streaming_read(&self) -> bool {
        // SFTP reads entire file into memory before returning a stream
        false
    }

    fn supports_versioning(&self) -> bool {
        // SFTP uses file size as a synthetic version, not native versioning
        false
    }

    async fn create_reader(
        &self,
        url: &Url,
        config: Arc<BundleConfig>,
    ) -> Result<Box<dyn IOReadFile>, BundlebaseError> {
        Ok(Box::new(SftpFile::from_url(url, config)?))
    }

    async fn create_lister(
        &self,
        url: &Url,
        config: Arc<BundleConfig>,
    ) -> Result<Box<dyn IOReadDir>, BundlebaseError> {
        Ok(Box::new(SftpDir::from_url(url, config)?))
    }

    async fn create_writer(
        &self,
        _url: &Url,
        _config: Arc<BundleConfig>,
    ) -> Result<Option<Box<dyn IOReadWriteFile>>, BundlebaseError> {
        Ok(None) // Read-only
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sftp_url_full() {
        let url = Url::parse("sftp://testuser@example.com:2222/home/data").unwrap();
        let (user, host, port, path) = parse_sftp_url(&url).unwrap();
        assert_eq!(user, "testuser");
        assert_eq!(host, "example.com");
        assert_eq!(port, 2222);
        assert_eq!(path, "/home/data");
    }

    #[test]
    fn test_parse_sftp_url() {
        let url = Url::parse("sftp://testuser@example.com/data/files").unwrap();
        let (user, host, port, path) = parse_sftp_url(&url).unwrap();
        assert_eq!(user, "testuser");
        assert_eq!(host, "example.com");
        assert_eq!(port, 22);
        assert_eq!(path, "/data/files");
    }

    #[test]
    fn test_parse_sftp_url_wrong_scheme() {
        let url = Url::parse("http://example.com/data").unwrap();
        let result = parse_sftp_url(&url);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Expected 'sftp'"));
    }

    #[test]
    fn test_parse_sftp_url_missing_path() {
        let url = Url::parse("sftp://user@example.com").unwrap();
        let result = parse_sftp_url(&url);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must include a path"));
    }

    #[test]
    fn test_parse_sftp_url_defaults_user() {
        // When no username is provided, it defaults to $USER or "root"
        let url = Url::parse("sftp://example.com/data").unwrap();
        let result = parse_sftp_url(&url);
        assert!(result.is_ok());
        let (user, host, _, _) = result.unwrap();
        assert_eq!(host, "example.com");
        // User defaults to either $USER env var or "root"
        assert!(!user.is_empty());
    }

    #[test]
    fn test_build_sftp_url() {
        let url = build_sftp_url("user", "example.com", 22, "/data").unwrap();
        assert_eq!(url.scheme(), "sftp");
        assert_eq!(url.username(), "user");
        assert_eq!(url.host_str(), Some("example.com"));
        // Note: port() returns None for default ports (22 for SFTP)
        assert_eq!(url.port_or_known_default(), Some(22));
        assert_eq!(url.path(), "/data");
    }

    #[test]
    fn test_build_sftp_url_custom_port() {
        let url = build_sftp_url("user", "example.com", 2222, "/data").unwrap();
        assert_eq!(url.port(), Some(2222));
    }

    #[test]
    fn test_sftp_factory_schemes() {
        let factory = SftpIOFactory;
        let schemes = factory.schemes();
        assert_eq!(schemes, &["sftp"]);
    }

    #[test]
    fn test_sftp_factory_supports_write_returns_false() {
        let factory = SftpIOFactory;
        let url = Url::parse("sftp://user@example.com/data").unwrap();
        assert!(!factory.supports_write(&url));
    }

    #[test]
    fn test_sftp_file_from_url() {
        let url = Url::parse("sftp://testuser@example.com:2222/home/data/file.txt").unwrap();
        let sftp_file = SftpFile::from_url(&url, BundleConfig::default().into()).unwrap();
        assert_eq!(sftp_file.host, "example.com");
        assert_eq!(sftp_file.port, 2222);
        assert_eq!(sftp_file.user, "testuser");
        assert_eq!(sftp_file.path, "/home/data/file.txt");
    }

    #[test]
    fn test_sftp_dir_from_url() {
        let url = Url::parse("sftp://testuser@example.com/data/").unwrap();
        let sftp_dir = SftpDir::from_url(&url, BundleConfig::default().into()).unwrap();
        assert_eq!(sftp_dir.host, "example.com");
        assert_eq!(sftp_dir.port, 22);
        assert_eq!(sftp_dir.user, "testuser");
        assert_eq!(sftp_dir.path, "/data/");
    }
}
