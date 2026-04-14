//! Configuration scope — a normalized, boundary-aware scope identifier.

use crate::config::ConfigScope;
use crate::BundlebaseError;
use serde::Serialize;
use std::fmt;

/// A normalized, boundary-aware configuration scope.
///
/// Scopes identify which path prefix a config value applies to. They are always
/// stored in a canonical form:
/// - No leading `/`
/// - No `://` sequences (URLs are converted to name form)
/// - No trailing `/`
///
/// # Matching
///
/// Scopes match on complete `/` boundaries:
/// - `s3/abc` matches `s3/abc` (exact)
/// - `s3/abc` matches `s3/abc/def` (boundary prefix: next char is `/`)
/// - `s3/abc` does NOT match `s3/abcd` (next char is `d`, not `/`)
///
/// # Construction
///
/// - Use `Scope::from_name()` when you already know the scope name (e.g., from a ConfigScope constant)
/// - Use `Scope::from_url()` to convert a URL using a specific ConfigScope's rules
/// - For user-facing input with full validation, use `BundleConfig::validate_scope()` in the core crate
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Scope(String);

/// Custom deserializer that validates scope strings.
impl<'de> serde::Deserialize<'de> for Scope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Scope::from_name(s).map_err(|e| serde::de::Error::custom(e.to_string()))
    }
}

impl Scope {
    /// Create a scope from a known name string.
    ///
    /// Performs basic syntactic validation (no empty, no leading slash, no `://`).
    /// Does NOT validate against registered scopes — use this when you know the
    /// scope name is valid (e.g., from a ConfigScope constant or a URL conversion).
    pub fn from_name(name: impl Into<String>) -> Result<Self, BundlebaseError> {
        let name = name.into();
        if name.is_empty() {
            return Err(BundlebaseError::from(
                "Scope cannot be empty. Use a named scope like 's3' or 'kaggle'.",
            ));
        }
        if name.starts_with('/') {
            return Err(BundlebaseError::from(format!(
                "Scope must not start with '/': got '{}'",
                name
            )));
        }
        if name.contains("://") {
            return Err(BundlebaseError::from(format!(
                "Scope must not contain '://': got '{}'. Use Scope::from_url() for URLs.",
                name
            )));
        }
        Ok(Self(name))
    }

    /// Create a scope from a URL using a specific ConfigScope's URL-to-name conversion.
    ///
    /// This is the preferred way to create a scope from a URL when you know
    /// which ConfigScope handles the URL scheme.
    ///
    /// # Example
    /// ```rust,ignore
    /// let scope = Scope::from_url("sftp://host/path", &SFTP_SCOPE)?;
    /// assert_eq!(scope.as_str(), "sftp/host/path");
    /// ```
    pub fn from_url(url: &str, config_scope: &ConfigScope) -> Result<Self, BundlebaseError> {
        let name = config_scope.url_to_name(url).ok_or_else(|| {
            BundlebaseError::from(format!(
                "URL '{}' does not match scope '{}'",
                url, config_scope.name
            ))
        })?;
        Self::from_name(name)
    }

    /// Create a scope from a URL, trying all provided ConfigScopes.
    ///
    /// Returns the first successful match, or an error if no scope matches.
    pub fn from_url_with_scopes(
        url: &str,
        scopes: &[ConfigScope],
    ) -> Result<Self, BundlebaseError> {
        for scope in scopes {
            if let Some(name) = scope.url_to_name(url) {
                return Self::from_name(name);
            }
        }
        Err(BundlebaseError::from(format!(
            "Unknown scope for URL '{}'",
            url
        )))
    }

    /// Create a validated scope from user input.
    ///
    /// Handles both URLs (e.g., `s3://bucket/path`) and names (e.g., `s3/bucket`).
    /// Validates against the provided list of known scopes.
    pub fn new(input: &str, known_scopes: &[ConfigScope]) -> Result<Self, BundlebaseError> {
        if input.is_empty() {
            return Err(BundlebaseError::from(
                "Scope cannot be empty. Use a named scope like 's3' or 'kaggle'.",
            ));
        }

        if input.starts_with('/') {
            return Err(BundlebaseError::from(format!(
                "Scope must not start with '/': got '{}'",
                input
            )));
        }

        if input.contains("://") {
            return Self::from_url_with_scopes(input, known_scopes);
        }

        // Name-based: validate first path component against known scopes
        let first = input.split('/').next().unwrap_or(input);
        for config_scope in known_scopes {
            if config_scope.name == first {
                return Ok(Self(input.to_string()));
            }
        }
        Err(BundlebaseError::from(format!("Unknown scope: {}", input)))
    }

    /// Check whether this scope matches a query scope.
    pub fn matches(&self, query: &Scope) -> bool {
        if self.0 == query.0 {
            return true;
        }
        if query.0.len() > self.0.len()
            && query.0.starts_with(&self.0)
            && query.0.as_bytes()[self.0.len()] == b'/'
        {
            return true;
        }
        false
    }

    /// Returns the scope as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<&str> for Scope {
    type Error = BundlebaseError;

    /// Create a scope from a string. Performs basic syntactic validation only.
    /// For full registry validation, use `Scope::new()` with known scopes
    /// or `validated_scope()` from the core crate.
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        // If it contains "://", it's a URL — convert using default scheme-based logic
        if s.contains("://") {
            // Apply the default URL-to-name conversion directly
            let scheme_end = s.find("://").unwrap_or(0);
            let scheme = &s[..scheme_end];
            let rest = s[scheme_end + 3..].trim_end_matches('/');
            let name = if rest.is_empty() {
                scheme.to_string()
            } else {
                format!("{}/{}", scheme, rest)
            };
            return Self::from_name(name);
        }
        Self::from_name(s)
    }
}

impl TryFrom<&url::Url> for Scope {
    type Error = BundlebaseError;

    fn try_from(url: &url::Url) -> Result<Self, Self::Error> {
        Self::try_from(url.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_name_valid() {
        assert_eq!(Scope::from_name("s3").unwrap().as_str(), "s3");
        assert_eq!(Scope::from_name("s3/bucket").unwrap().as_str(), "s3/bucket");
    }

    #[test]
    fn test_from_name_rejects_empty() {
        assert!(Scope::from_name("").is_err());
    }

    #[test]
    fn test_from_name_rejects_leading_slash() {
        assert!(Scope::from_name("/s3").is_err());
    }

    #[test]
    fn test_from_name_rejects_url() {
        assert!(Scope::from_name("s3://bucket").is_err());
    }

    #[test]
    fn test_from_url() {
        let scope = ConfigScope::new("s3");
        assert_eq!(
            Scope::from_url("s3://bucket/path", &scope)
                .unwrap()
                .as_str(),
            "s3/bucket/path"
        );
    }

    #[test]
    fn test_from_url_no_path() {
        let scope = ConfigScope::new("s3");
        assert_eq!(Scope::from_url("s3://", &scope).unwrap().as_str(), "s3");
    }

    #[test]
    fn test_from_url_wrong_scheme() {
        let scope = ConfigScope::new("s3");
        assert!(Scope::from_url("gs://bucket", &scope).is_err());
    }

    #[test]
    fn test_matches_exact() {
        let s = Scope::from_name("s3/abc").unwrap();
        assert!(s.matches(&Scope::from_name("s3/abc").unwrap()));
    }

    #[test]
    fn test_matches_boundary_prefix() {
        let s = Scope::from_name("s3/abc").unwrap();
        assert!(s.matches(&Scope::from_name("s3/abc/def").unwrap()));
    }

    #[test]
    fn test_matches_rejects_non_boundary() {
        let s = Scope::from_name("s3/abc").unwrap();
        assert!(!s.matches(&Scope::from_name("s3/abcd").unwrap()));
    }

    #[test]
    fn test_matches_rejects_shorter() {
        let s = Scope::from_name("s3/abc").unwrap();
        assert!(!s.matches(&Scope::from_name("s3/ab").unwrap()));
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", Scope::from_name("s3/abc").unwrap()), "s3/abc");
    }

    #[test]
    fn test_new_with_known_scopes() {
        let scopes = [ConfigScope::new("s3"), ConfigScope::new("gs")];
        assert_eq!(Scope::new("s3", &scopes).unwrap().as_str(), "s3");
        assert_eq!(
            Scope::new("s3/bucket", &scopes).unwrap().as_str(),
            "s3/bucket"
        );
        assert!(Scope::new("unknown", &scopes).is_err());
    }

    #[test]
    fn test_new_with_url() {
        let scopes = [ConfigScope::new("s3"), ConfigScope::new("gs")];
        assert_eq!(
            Scope::new("s3://bucket/path", &scopes).unwrap().as_str(),
            "s3/bucket/path"
        );
    }

    #[test]
    fn test_serde_roundtrip() {
        let s = Scope::from_name("s3/abc/def").unwrap();
        let json = serde_json::to_string(&s).expect("serialize");
        assert_eq!(json, r#""s3/abc/def""#);
        let deserialized: Scope = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized, s);
    }
}
