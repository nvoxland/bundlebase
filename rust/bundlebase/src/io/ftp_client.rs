//! FTP client for file operations.
//!
//! Provides an async FTP client wrapper around suppaftp for listing and downloading files
//! from remote FTP servers.

use crate::BundlebaseError;
use bytes::Bytes;
use suppaftp::tokio::AsyncFtpStream;
use suppaftp::types::FileType;
use tokio::io::AsyncReadExt;

/// Information about a remote file from FTP listing.
#[derive(Debug, Clone)]
pub struct FtpFileInfo {
    /// Full path on the remote system
    pub path: String,
    /// File size in bytes (if available)
    pub size: Option<u64>,
    /// Whether this entry is a directory
    pub is_dir: bool,
}

/// FTP client for file operations.
///
/// Provides methods to connect to FTP servers, list remote directories,
/// and download file contents.
pub struct FtpClient {
    stream: AsyncFtpStream,
}

impl FtpClient {
    /// Connect to an FTP server with optional authentication.
    ///
    /// # Arguments
    /// * `host` - Hostname or IP address
    /// * `port` - FTP port (typically 21)
    /// * `user` - Username for authentication (use "anonymous" for anonymous FTP)
    /// * `password` - Password for authentication (use "" or email for anonymous)
    ///
    /// # Returns
    /// Connected FTP client or error if connection fails.
    pub async fn connect(
        host: &str,
        port: u16,
        user: &str,
        password: &str,
    ) -> Result<Self, BundlebaseError> {
        // Connect to the FTP server
        let mut stream = AsyncFtpStream::connect(format!("{}:{}", host, port))
            .await
            .map_err(|e| {
                BundlebaseError::from(format!(
                    "Failed to connect to FTP server {}:{}: {}",
                    host, port, e
                ))
            })?;

        // Login
        stream.login(user, password).await.map_err(|e| {
            BundlebaseError::from(format!(
                "FTP authentication failed for user '{}': {}",
                user, e
            ))
        })?;

        // Set binary transfer mode
        stream.transfer_type(FileType::Binary).await.map_err(|e| {
            BundlebaseError::from(format!("Failed to set FTP binary mode: {}", e))
        })?;

        Ok(Self { stream })
    }

    /// List all files in a remote directory recursively.
    ///
    /// # Arguments
    /// * `path` - Remote directory path to list
    ///
    /// # Returns
    /// Vector of file information for all files (not directories) in the tree.
    pub async fn list_files_recursive(&mut self, path: &str) -> Result<Vec<FtpFileInfo>, BundlebaseError> {
        let mut all_files = Vec::new();
        self.list_dir_recursive_inner(path, &mut all_files).await?;
        Ok(all_files)
    }

    /// Internal recursive directory listing helper.
    async fn list_dir_recursive_inner(
        &mut self,
        path: &str,
        files: &mut Vec<FtpFileInfo>,
    ) -> Result<(), BundlebaseError> {
        // Get detailed listing using NLST command
        let entries: Vec<String> = self.stream.nlst(Some(path)).await.map_err(|e| {
            BundlebaseError::from(format!("Failed to list FTP directory '{}': {}", path, e))
        })?;

        for entry_name in entries {
            // Skip . and ..
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
            // If we can get the size, it's a file; if it fails, it might be a directory
            match self.stream.size(&full_path).await {
                Ok(size) => {
                    // It's a file
                    files.push(FtpFileInfo {
                        path: full_path,
                        size: Some(size as u64),
                        is_dir: false,
                    });
                }
                Err(_) => {
                    // Might be a directory, try to list it
                    let sub_result: Result<Vec<String>, _> = self.stream.nlst(Some(&full_path)).await;
                    if let Ok(sub_entries) = sub_result {
                        // If we can list it, it's a directory - recurse
                        if !sub_entries.is_empty() {
                            Box::pin(self.list_dir_recursive_inner(&full_path, files)).await?;
                        }
                    }
                    // If both fail, skip this entry
                }
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
    pub async fn read_file(&mut self, path: &str) -> Result<Bytes, BundlebaseError> {
        // Use retr to get a data stream and read from it
        let mut buffer = Vec::new();
        let mut data_stream = self.stream.retr_as_stream(path).await.map_err(|e| {
            BundlebaseError::from(format!("Failed to start FTP download for '{}': {}", path, e))
        })?;

        data_stream.read_to_end(&mut buffer).await.map_err(|e| {
            BundlebaseError::from(format!("Failed to read FTP file '{}': {}", path, e))
        })?;

        // Finalize the transfer
        self.stream.finalize_retr_stream(data_stream).await.map_err(|e| {
            BundlebaseError::from(format!("Failed to finalize FTP download for '{}': {}", path, e))
        })?;

        Ok(Bytes::from(buffer))
    }

    /// Close the FTP connection.
    pub async fn close(mut self) -> Result<(), BundlebaseError> {
        let _: () = self.stream.quit().await.map_err(|e| {
            BundlebaseError::from(format!("Failed to close FTP connection: {}", e))
        })?;
        Ok(())
    }
}

/// Parse an FTP URL into its components.
///
/// # URL Format
/// `ftp://[user[:password]@]host[:port]/path`
///
/// Examples:
/// - `ftp://ftp.example.com/pub/data`
/// - `ftp://user:pass@ftp.example.com/data`
/// - `ftp://ftp.example.com:2121/data`
/// - `ftp://anonymous@ftp.example.com/pub` (anonymous FTP)
///
/// # Returns
/// Tuple of (user, password, host, port, path)
pub fn parse_ftp_url(url: &url::Url) -> Result<(String, String, String, u16, String), BundlebaseError> {
    if url.scheme() != "ftp" {
        return Err(format!("Expected 'ftp' URL scheme, got '{}'", url.scheme()).into());
    }

    let host = url.host_str().ok_or_else(|| {
        BundlebaseError::from("FTP URL must include a host")
    })?;

    let port = url.port().unwrap_or(21);

    // Default to anonymous if no user specified
    let user = if url.username().is_empty() {
        "anonymous".to_string()
    } else {
        url.username().to_string()
    };

    // Default password for anonymous is empty or email
    let password = url.password().unwrap_or("").to_string();

    let path = url.path().to_string();
    if path.is_empty() || path == "/" {
        return Err("FTP URL must include a path".into());
    }

    Ok((user, password, host.to_string(), port, path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    #[test]
    fn test_parse_ftp_url_full() {
        let url = Url::parse("ftp://testuser:testpass@ftp.example.com:2121/data/files").unwrap();
        let (user, password, host, port, path) = parse_ftp_url(&url).unwrap();
        assert_eq!(user, "testuser");
        assert_eq!(password, "testpass");
        assert_eq!(host, "ftp.example.com");
        assert_eq!(port, 2121);
        assert_eq!(path, "/data/files");
    }

    #[test]
    fn test_parse_ftp_url_default_port() {
        let url = Url::parse("ftp://user:pass@ftp.example.com/data").unwrap();
        let (user, password, host, port, path) = parse_ftp_url(&url).unwrap();
        assert_eq!(user, "user");
        assert_eq!(password, "pass");
        assert_eq!(host, "ftp.example.com");
        assert_eq!(port, 21);
        assert_eq!(path, "/data");
    }

    #[test]
    fn test_parse_ftp_url_anonymous() {
        let url = Url::parse("ftp://ftp.example.com/pub/data").unwrap();
        let (user, password, host, port, path) = parse_ftp_url(&url).unwrap();
        assert_eq!(user, "anonymous");
        assert_eq!(password, "");
        assert_eq!(host, "ftp.example.com");
        assert_eq!(port, 21);
        assert_eq!(path, "/pub/data");
    }

    #[test]
    fn test_parse_ftp_url_user_no_password() {
        let url = Url::parse("ftp://anonymous@ftp.example.com/pub").unwrap();
        let (user, password, host, port, path) = parse_ftp_url(&url).unwrap();
        assert_eq!(user, "anonymous");
        assert_eq!(password, "");
        assert_eq!(host, "ftp.example.com");
        assert_eq!(port, 21);
        assert_eq!(path, "/pub");
    }

    #[test]
    fn test_parse_ftp_url_wrong_scheme() {
        let url = Url::parse("http://example.com/data").unwrap();
        let result = parse_ftp_url(&url);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Expected 'ftp'"));
    }

    #[test]
    fn test_parse_ftp_url_no_host() {
        let url = Url::parse("ftp:///data").unwrap();
        let result = parse_ftp_url(&url);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("must include a host") || err_msg.contains("must include a path"),
            "Unexpected error message: {}",
            err_msg
        );
    }

    #[test]
    fn test_parse_ftp_url_no_path() {
        let url = Url::parse("ftp://ftp.example.com").unwrap();
        let result = parse_ftp_url(&url);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must include a path"));
    }

    #[test]
    fn test_parse_ftp_url_root_only_path() {
        let url = Url::parse("ftp://ftp.example.com/").unwrap();
        let result = parse_ftp_url(&url);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must include a path"));
    }
}
