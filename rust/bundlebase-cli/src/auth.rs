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

    /// Build an authenticator from environment variables so operators can set
    /// real credentials without the hardcoded default. Each of
    /// `BUNDLEBASE_FLIGHT_USERNAME` / `BUNDLEBASE_FLIGHT_PASSWORD` that is set
    /// overrides the corresponding default.
    pub fn from_env() -> Self {
        Self {
            username: std::env::var("BUNDLEBASE_FLIGHT_USERNAME").ok(),
            password: std::env::var("BUNDLEBASE_FLIGHT_PASSWORD").ok(),
        }
    }

    /// Returns true if either credential is still the insecure built-in
    /// default — setting only the username silently keeps the default
    /// password accepted, which deserves the startup warning just as much.
    pub fn is_using_defaults(&self) -> bool {
        self.username.is_none() || self.password.is_none()
    }

    /// Validate username and password using a constant-time comparison so an
    /// attacker can't recover the credentials from response timing.
    pub fn validate(&self, username: &str, password: &str) -> bool {
        let expected_username = self.username.as_deref().unwrap_or(USERNAME);
        let expected_password = self.password.as_deref().unwrap_or(PASSWORD);

        // `&` (not `&&`) so both comparisons always run — no short-circuit.
        constant_time_eq(username.as_bytes(), expected_username.as_bytes())
            & constant_time_eq(password.as_bytes(), expected_password.as_bytes())
    }
}

/// Constant-time byte-slice equality. Returns false immediately on a length
/// mismatch (length is not secret here), otherwise compares every byte so the
/// time taken does not depend on where the first difference is.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
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

    #[test]
    fn test_constant_time_eq() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreX"));
        assert!(!constant_time_eq(b"secret", b"secre")); // length mismatch
        assert!(!constant_time_eq(b"secret", b"secret!"));
        assert!(constant_time_eq(b"", b""));
    }
}
