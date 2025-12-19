/// Basic authentication credentials
/// For now, hardcoded username/password
pub const USERNAME: &str = "admin";
pub const PASSWORD: &str = "password";

/// Validate username and password
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
}
