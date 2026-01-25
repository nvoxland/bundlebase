//! Authentication for bundlebase CLI services.
//!
//! This module provides authentication support for Flight and other services.

/// Basic authentication credentials.
/// For now, hardcoded username/password.
pub const USERNAME: &str = "admin";
pub const PASSWORD: &str = "password";

/// Authenticator for bundlebase services.
///
/// Provides credential validation for Flight handshake and other authentication needs.
#[derive(Debug, Clone, Default)]
pub struct BundlebaseAuthenticator {
    /// Username for authentication (uses default if None)
    pub username: Option<String>,
    /// Password for authentication (uses default if None)
    pub password: Option<String>,
}

impl BundlebaseAuthenticator {
    /// Create a new authenticator with default credentials.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new authenticator with custom credentials.
    pub fn with_credentials(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: Some(username.into()),
            password: Some(password.into()),
        }
    }

    /// Validate username and password.
    pub fn validate(&self, username: &str, password: &str) -> bool {
        let expected_username = self.username.as_deref().unwrap_or(USERNAME);
        let expected_password = self.password.as_deref().unwrap_or(PASSWORD);

        username == expected_username && password == expected_password
    }
}

/// Validate username and password using default credentials.
pub fn validate_credentials(username: &str, password: &str) -> bool {
    username == USERNAME && password == PASSWORD
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_credentials() {
        assert!(validate_credentials("admin", "password"));
    }

    #[test]
    fn test_invalid_username() {
        assert!(!validate_credentials("user", "password"));
    }

    #[test]
    fn test_invalid_password() {
        assert!(!validate_credentials("admin", "wrong"));
    }

    #[test]
    fn test_invalid_both() {
        assert!(!validate_credentials("user", "wrong"));
    }

    #[test]
    fn test_authenticator_default() {
        let auth = BundlebaseAuthenticator::default();
        assert!(auth.validate("admin", "password"));
        assert!(!auth.validate("admin", "wrong"));
    }

    #[test]
    fn test_authenticator_custom() {
        let auth = BundlebaseAuthenticator::with_credentials("custom", "secret");
        assert!(auth.validate("custom", "secret"));
        assert!(!auth.validate("admin", "password"));
    }
}
