//! IO Plugin system - Protocol-specific storage backend implementations.
//!
//! This module contains implementations for specific storage backends:
//! - `object_store`: file://, s3://, gs://, azure://, memory://, empty://
//! - `ftp`: ftp://
//! - `sftp`: sftp://, scp://
//! - `tar`: tar://

pub mod ftp;
pub mod object_store;
pub mod sftp;
pub mod tar;

use crate::io::registry::IORegistry;
use std::sync::Arc;

// Re-export commonly used types for convenience
pub use ftp::{FtpDir, FtpFile, FtpFileInfo, FtpIOFactory};
pub use object_store::{ObjectStoreDir, ObjectStoreFile, ObjectStoreIOFactory};
pub use sftp::{SftpClient, SftpDir, SftpFile, SftpFileInfo, SftpIOFactory};
pub use tar::{TarDir, TarFile, TarIOFactory, TarObjectStore};

/// Register all built-in IO factories with the registry.
///
/// This is called by the global `io_registry()` singleton to set up
/// all protocol handlers.
pub fn register_builtin_factories(registry: &mut IORegistry) {
    registry.register(Arc::new(ObjectStoreIOFactory));
    registry.register(Arc::new(FtpIOFactory));
    registry.register(Arc::new(SftpIOFactory));
    registry.register(Arc::new(TarIOFactory));
}
