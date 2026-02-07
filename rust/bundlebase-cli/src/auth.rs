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

    /// Returns true if no custom credentials have been configured.
    pub fn is_using_defaults(&self) -> bool {
        self.username.is_none() && self.password.is_none()
    }

    /// Validate username and password.
    pub fn validate(&self, username: &str, password: &str) -> bool {
        let expected_username = self.username.as_deref().unwrap_or(USERNAME);
        let expected_password = self.password.as_deref().unwrap_or(PASSWORD);

        username == expected_username && password == expected_password
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_authenticator_default_invalid_username() {
        let auth = BundlebaseAuthenticator::default();
        assert!(!auth.validate("user", "password"));
    }

    #[test]
    fn test_authenticator_default_invalid_both() {
        let auth = BundlebaseAuthenticator::default();
        assert!(!auth.validate("user", "wrong"));
    }

    #[test]
    fn test_uses_default_credentials() {
        let auth = BundlebaseAuthenticator::new();
        assert!(auth.is_using_defaults());
    }

    #[test]
    fn test_uses_custom_credentials() {
        let auth = BundlebaseAuthenticator::with_credentials("custom", "secret");
        assert!(!auth.is_using_defaults());
    }
}
