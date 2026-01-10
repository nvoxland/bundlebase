//! SFTP client for SSH file operations.
//!
//! Provides an async SFTP client wrapper around russh for listing and downloading files
//! from remote directories via SSH.

use crate::BundlebaseError;
use bytes::Bytes;
use russh::client::{self, Handle};
use russh_keys::key::PublicKey;
use russh_keys::load_secret_key;
use russh_sftp::client::SftpSession;
use std::path::Path;
use std::sync::Arc;
use tokio::io::AsyncReadExt;

/// Information about a remote file from SFTP listing.
#[derive(Debug, Clone)]
pub struct SftpFileInfo {
    /// Full path on the remote system
    pub path: String,
    /// File size in bytes
    pub size: u64,
    /// Whether this entry is a directory
    pub is_dir: bool,
}

/// SSH client handler for russh.
struct SshHandler;

#[async_trait::async_trait]
impl client::Handler for SshHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        // For now, accept all host keys. In production, this should verify against known_hosts.
        // TODO: Add known_hosts verification support
        Ok(true)
    }
}

/// SFTP client for SSH file operations.
///
/// Provides methods to connect to SSH servers, list remote directories,
/// and download file contents via SFTP.
pub struct SftpClient {
    session: Handle<SshHandler>,
    sftp: SftpSession,
}

impl SftpClient {
    /// Connect to an SSH host with key authentication.
    ///
    /// # Arguments
    /// * `host` - Hostname or IP address
    /// * `port` - SSH port (typically 22)
    /// * `user` - Username for authentication
    /// * `key_path` - Path to SSH private key file
    ///
    /// # Returns
    /// Connected SFTP client or error if connection fails.
    pub async fn connect(
        host: &str,
        port: u16,
        user: &str,
        key_path: &Path,
    ) -> Result<Self, BundlebaseError> {
        // Load the SSH private key
        let key = load_secret_key(key_path, None).map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to load SSH key from '{}': {}",
                key_path.display(),
                e
            ))
        })?;

        // Create SSH client configuration
        let config = Arc::new(client::Config::default());

        // Connect to the SSH server
        let mut session = client::connect(config, (host, port), SshHandler)
            .await
            .map_err(|e| {
                BundlebaseError::from(format!(
                    "Failed to connect to SSH server {}:{}: {}",
                    host, port, e
                ))
            })?;

        // Authenticate with the key
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

        // Open SFTP channel
        let channel = session.channel_open_session().await.map_err(|e| {
            BundlebaseError::from(format!("Failed to open SSH channel: {}", e))
        })?;

        channel.request_subsystem(true, "sftp").await.map_err(|e| {
            BundlebaseError::from(format!("Failed to request SFTP subsystem: {}", e))
        })?;

        // Create SFTP session
        let sftp = SftpSession::new(channel.into_stream()).await.map_err(|e| {
            BundlebaseError::from(format!("Failed to initialize SFTP session: {}", e))
        })?;

        Ok(Self { session, sftp })
    }

    /// List all files in a remote directory recursively.
    ///
    /// # Arguments
    /// * `path` - Remote directory path to list
    ///
    /// # Returns
    /// Vector of file information for all files (not directories) in the tree.
    pub async fn list_files_recursive(&self, path: &str) -> Result<Vec<SftpFileInfo>, BundlebaseError> {
        let mut all_files = Vec::new();
        self.list_dir_recursive_inner(path, &mut all_files).await?;
        Ok(all_files)
    }

    /// Internal recursive directory listing helper.
    async fn list_dir_recursive_inner(
        &self,
        path: &str,
        files: &mut Vec<SftpFileInfo>,
    ) -> Result<(), BundlebaseError> {
        let entries = self.sftp.read_dir(path).await.map_err(|e| {
            BundlebaseError::from(format!("Failed to list directory '{}': {}", path, e))
        })?;

        for entry in entries {
            let file_name = entry.file_name();

            // Skip . and ..
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
                // Recurse into subdirectory
                Box::pin(self.list_dir_recursive_inner(&full_path, files)).await?;
            } else {
                files.push(SftpFileInfo {
                    path: full_path,
                    size,
                    is_dir: false,
                });
            }
        }

        Ok(())
    }

    /// Read file contents from the remote server.
    ///
    /// # Arguments
    /// * `path` - Remote file path to read
    ///
    /// # Returns
    /// File contents as bytes.
    pub async fn read_file(&self, path: &str) -> Result<Bytes, BundlebaseError> {
        let mut file = self.sftp.open(path).await.map_err(|e| {
            BundlebaseError::from(format!("Failed to open remote file '{}': {}", path, e))
        })?;

        // Read entire file contents using AsyncReadExt
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).await.map_err(|e| {
            BundlebaseError::from(format!("Failed to read from '{}': {}", path, e))
        })?;

        Ok(Bytes::from(buffer))
    }

    /// Close the SFTP session and underlying SSH connection.
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

/// Parse an SCP/SFTP URL into its components.
///
/// # URL Format
/// `scp://[user@]host[:port]/path` or `sftp://[user@]host[:port]/path`
///
/// Both `scp://` and `sftp://` schemes are accepted and treated identically.
///
/// Examples:
/// - `scp://user@example.com/data/files`
/// - `sftp://user@example.com/data/files`
/// - `scp://user@example.com:22/home/user/data`
/// - `sftp://192.168.1.100/var/data`
///
/// # Returns
/// Tuple of (user, host, port, path)
pub fn parse_scp_url(url: &url::Url) -> Result<(String, String, u16, String), BundlebaseError> {
    let scheme = url.scheme();
    if scheme != "scp" && scheme != "sftp" {
        return Err(format!("Expected 'scp' or 'sftp' URL scheme, got '{}'", scheme).into());
    }

    let host = url.host_str().ok_or_else(|| {
        BundlebaseError::from("SCP/SFTP URL must include a host")
    })?;

    let port = url.port().unwrap_or(22);

    let user = if url.username().is_empty() {
        // Default to current user
        std::env::var("USER").unwrap_or_else(|_| "root".to_string())
    } else {
        url.username().to_string()
    };

    let path = url.path().to_string();
    if path.is_empty() || path == "/" {
        return Err("SCP/SFTP URL must include a path".into());
    }

    Ok((user, host.to_string(), port, path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    #[test]
    fn test_parse_scp_url_full() {
        let url = Url::parse("scp://testuser@example.com:2222/home/data").unwrap();
        let (user, host, port, path) = parse_scp_url(&url).unwrap();
        assert_eq!(user, "testuser");
        assert_eq!(host, "example.com");
        assert_eq!(port, 2222);
        assert_eq!(path, "/home/data");
    }

    #[test]
    fn test_parse_scp_url_default_port() {
        let url = Url::parse("scp://testuser@example.com/data/files").unwrap();
        let (user, host, port, path) = parse_scp_url(&url).unwrap();
        assert_eq!(user, "testuser");
        assert_eq!(host, "example.com");
        assert_eq!(port, 22);
        assert_eq!(path, "/data/files");
    }

    #[test]
    fn test_parse_scp_url_no_user() {
        let url = Url::parse("scp://192.168.1.100/var/data").unwrap();
        let (user, host, port, path) = parse_scp_url(&url).unwrap();
        // User should default to $USER env var or "root"
        assert!(!user.is_empty());
        assert_eq!(host, "192.168.1.100");
        assert_eq!(port, 22);
        assert_eq!(path, "/var/data");
    }

    #[test]
    fn test_parse_scp_url_wrong_scheme() {
        let url = Url::parse("http://example.com/data").unwrap();
        let result = parse_scp_url(&url);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Expected 'scp' or 'sftp'"));
    }

    #[test]
    fn test_parse_scp_url_no_host() {
        let url = Url::parse("scp:///data").unwrap();
        let result = parse_scp_url(&url);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must include a host"));
    }

    #[test]
    fn test_parse_scp_url_no_path() {
        let url = Url::parse("scp://user@example.com").unwrap();
        let result = parse_scp_url(&url);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must include a path"));
    }

    #[test]
    fn test_parse_scp_url_root_only_path() {
        let url = Url::parse("scp://user@example.com/").unwrap();
        let result = parse_scp_url(&url);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must include a path"));
    }

    // Tests for sftp:// scheme
    #[test]
    fn test_parse_sftp_url_full() {
        let url = Url::parse("sftp://testuser@example.com:2222/home/data").unwrap();
        let (user, host, port, path) = parse_scp_url(&url).unwrap();
        assert_eq!(user, "testuser");
        assert_eq!(host, "example.com");
        assert_eq!(port, 2222);
        assert_eq!(path, "/home/data");
    }

    #[test]
    fn test_parse_sftp_url_default_port() {
        let url = Url::parse("sftp://testuser@example.com/data/files").unwrap();
        let (user, host, port, path) = parse_scp_url(&url).unwrap();
        assert_eq!(user, "testuser");
        assert_eq!(host, "example.com");
        assert_eq!(port, 22);
        assert_eq!(path, "/data/files");
    }

    #[test]
    fn test_parse_sftp_url_no_user() {
        let url = Url::parse("sftp://192.168.1.100/var/data").unwrap();
        let (user, host, port, path) = parse_scp_url(&url).unwrap();
        // User should default to $USER env var or "root"
        assert!(!user.is_empty());
        assert_eq!(host, "192.168.1.100");
        assert_eq!(port, 22);
        assert_eq!(path, "/var/data");
    }
}
