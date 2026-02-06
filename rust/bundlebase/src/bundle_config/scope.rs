use crate::BundlebaseError;
use serde::{Deserialize, Serialize};
use std::fmt;

/// A normalized, boundary-aware configuration scope.
///
/// Scopes identify which path prefix a config value applies to. They are always
/// stored in a canonical form:
/// - No leading `/` (paths are normalized: `s3://bucket` → `s3/bucket`)
/// - No `://` sequences
/// - No trailing `/`
/// - The global scope is `"/"` and matches everything
///
/// # Matching
///
/// Scopes match on complete `/` boundaries:
/// - `s3/abc` matches `s3/abc` (exact)
/// - `s3/abc` matches `s3/abc/def` (boundary prefix: next char is `/`)
/// - `s3/abc` does NOT match `s3/abcd` (next char is `d`, not `/`)
/// - `"/"` (global) matches everything
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Scope(String);

impl Scope {
    /// Create a scope from a correctly formatted string.
    ///
    /// The input must already be in canonical scope form:
    /// - Must not start with `/`
    /// - Must not contain `://`
    /// - `"/"` represents the global scope (empty string is invalid)
    ///
    /// # Panics
    /// Panics if the input is not in valid scope form.
    ///
    /// # Examples
    /// ```
    /// use bundlebase::bundle_config::Scope;
    /// let s = Scope::new("s3/bucket/path");
    /// assert_eq!(s.as_str(), "s3/bucket/path");
    ///
    /// let global = Scope::new("/");
    /// assert!(global.is_global());
    /// ```
    pub fn new(s: &str) -> Self {
        if s.is_empty() {
            panic!("Scope cannot be empty. Use \"/\" for global scope.");
        }
        // "/" is the global scope - allowed
        if s != "/" {
            assert!(!s.starts_with('/'), "Scope must not start with '/': got '{}'", s);
        }
        assert!(!s.contains("://"), "Scope must not contain '://': got '{}'", s);
        Self(s.to_string())
    }

    /// Resolve a path string to a Scope by trying all registered config scopes.
    ///
    /// Each registered `ConfigScope` gets a chance to claim the path. If a scope
    /// matches, its normalized `Scope` is returned. If no scope matches, an error
    /// is returned.
    ///
    /// # Examples
    /// ```
    /// use bundlebase::bundle_config::Scope;
    /// let scope = Scope::from_path("s3://bucket/path").unwrap();
    /// assert_eq!(scope.as_str(), "s3/bucket/path");
    /// ```
    pub fn from_path(path: &str) -> Result<Self, BundlebaseError> {
        use super::BundleConfig;

        if path.is_empty() || path == "/" {
            return Ok(Self::global());
        }

        for config_scope in BundleConfig::all_scopes() {
            if let Some(s) = config_scope.from_path(path) {
                return Ok(s);
            }
        }
        Err(BundlebaseError::from(format!(
            "Unknown scope for path '{}'",
            path
        )))
    }

    /// Look up a scope by registered name. Accepts `kaggle` or `/kaggle` (legacy).
    ///
    /// Unlike [`from_path`], this does not run URL-based matching logic.
    /// It simply matches against the `name` field of each registered
    /// [`ConfigScope`](super::ConfigScope).
    ///
    /// # Examples
    /// ```
    /// use bundlebase::bundle_config::Scope;
    /// let scope = Scope::from_name("kaggle").unwrap();
    /// assert_eq!(scope.as_str(), "kaggle");
    ///
    /// let scope = Scope::from_name("s3").unwrap();
    /// assert_eq!(scope.as_str(), "s3");
    /// ```
    pub fn from_name(name: &str) -> Result<Self, BundlebaseError> {
        use super::BundleConfig;

        let trimmed = name.trim_start_matches('/');

        // Empty string or "/" means global scope
        if trimmed.is_empty() {
            return Ok(Self::global());
        }

        for config_scope in BundleConfig::all_scopes() {
            if config_scope.name == trimmed {
                return Ok(Scope::new(config_scope.name));
            }
        }
        Err(BundlebaseError::from(format!("Unknown scope: {}", name)))
    }

    /// Returns the global scope `"/"`, which matches everything.
    pub fn global() -> Self {
        Self("/".to_string())
    }

    /// Returns `true` if this is the global scope (`"/"`).
    pub fn is_global(&self) -> bool {
        self.0 == "/"
    }

    /// Check whether this scope matches a query scope.
    ///
    /// A scope matches a query if:
    /// - This scope is global (`"/"`) — matches everything
    /// - Exact match: this scope equals the query
    /// - Boundary prefix: the query starts with this scope AND the next
    ///   character in the query is `/`
    ///
    /// # Examples
    /// ```
    /// use bundlebase::bundle_config::Scope;
    /// let s = Scope::new("s3/abc");
    /// assert!(s.matches(&Scope::new("s3/abc")));       // exact
    /// assert!(s.matches(&Scope::new("s3/abc/def")));    // boundary prefix
    /// assert!(!s.matches(&Scope::new("s3/abcd")));      // NOT boundary
    /// assert!(!s.matches(&Scope::new("s3/ab")));        // too short
    ///
    /// let global = Scope::global();
    /// assert!(global.matches(&Scope::new("anything")));
    /// assert!(global.matches(&Scope::global()));
    /// ```
    pub fn matches(&self, query: &Scope) -> bool {
        if self.is_global() {
            return true;
        }
        if self.0 == query.0 {
            return true;
        }
        // Boundary prefix: query starts with self and next char is '/'
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

impl From<String> for Scope {
    /// Convert from a stored string (assumed to be already valid).
    /// Used during deserialization of trusted data.
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for Scope {
    /// Normalize a scope name into a Scope.
    ///
    /// This uses the same normalization as `from_path`:
    fn from(s: &str) -> Self {
        Self::from_path(s).expect("Invalid scope string")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== new() tests ====================

    #[test]
    fn test_new_global() {
        let s = Scope::new("/");
        assert_eq!(s.as_str(), "/");
        assert!(s.is_global());
    }

    #[test]
    #[should_panic(expected = "Scope cannot be empty")]
    fn test_new_rejects_empty() {
        Scope::new("");
    }

    #[test]
    fn test_new_simple_path() {
        let s = Scope::new("s3/abc/def");
        assert_eq!(s.as_str(), "s3/abc/def");
        assert!(!s.is_global());
    }

    #[test]
    #[should_panic(expected = "Scope must not start with '/'")]
    fn test_new_rejects_leading_slash() {
        Scope::new("/s3/abc");
    }

    #[test]
    #[should_panic(expected = "Scope must not contain '://'")]
    fn test_new_rejects_url_scheme() {
        Scope::new("s3://abc");
    }

    // ==================== normalize() tests ====================

    #[test]
    fn test_normalize_s3() {
        assert_eq!(Scope::from_path("s3://abc/def").unwrap().as_str(), "s3/abc/def");
    }

    #[test]
    fn test_normalize_from_name() {
        assert_eq!(Scope::from_name("s3").unwrap().as_str(), "s3");
    }

    #[test]
    fn test_normalize_from_name_with_slash() {
        // Legacy format with leading slash should still work
        assert_eq!(Scope::from_name("/s3").unwrap().as_str(), "s3");
    }

    #[test]
    fn test_normalize_bare_name() {
        // from_name only works for registered scopes like "s3"
        assert_eq!(Scope::from_name("s3").unwrap().as_str(), "s3");
    }

    #[test]
    fn test_from_name_unknown_scope() {
        // Unknown scope names should return an error
        assert!(Scope::from_name("xyz").is_err());
    }

    #[test]
    fn test_normalize_empty() {
        assert_eq!(Scope::from_path("").unwrap().as_str(), "/");
        assert!(Scope::from_path("").unwrap().is_global());
    }

    #[test]
    fn test_normalize_slash() {
        assert_eq!(Scope::from_path("/").unwrap().as_str(), "/");
        assert!(Scope::from_path("/").unwrap().is_global());
    }

    #[test]
    fn test_normalize_already_normalized() {
        assert_eq!(Scope::from_path("s3://bucket").unwrap().as_str(), "s3/bucket");
    }

    #[test]
    fn test_normalize_gs() {
        assert_eq!(Scope::from_path("gs://my-bucket/data/").unwrap().as_str(), "gs/my-bucket/data");
    }

    #[test]
    fn test_normalize_scheme_only() {
        assert_eq!(Scope::from_path("s3://").unwrap().as_str(), "s3");
    }

    #[test]
    fn test_normalize_kaggle() {
        assert_eq!(Scope::from_path("kaggle://").unwrap().as_str(), "kaggle");
    }

    // ==================== global() / is_global() tests ====================

    #[test]
    fn test_global() {
        let g = Scope::global();
        assert_eq!(g.as_str(), "/");
        assert!(g.is_global());
    }

    #[test]
    fn test_non_global() {
        let s = Scope::new("s3/bucket");
        assert!(!s.is_global());
    }

    // ==================== matches() tests ====================

    #[test]
    fn test_matches_exact() {
        let s = Scope::new("s3/abc");
        assert!(s.matches(&Scope::new("s3/abc")));
    }

    #[test]
    fn test_matches_boundary_prefix() {
        let s = Scope::new("s3/abc");
        assert!(s.matches(&Scope::new("s3/abc/def")));
    }

    #[test]
    fn test_matches_rejects_non_boundary() {
        let s = Scope::new("s3/abc");
        assert!(!s.matches(&Scope::new("s3/abcd")));
    }

    #[test]
    fn test_matches_rejects_shorter() {
        let s = Scope::new("s3/abc");
        assert!(!s.matches(&Scope::new("s3/ab")));
    }

    #[test]
    fn test_matches_global_matches_everything() {
        let g = Scope::global();
        assert!(g.matches(&Scope::new("s3/abc")));
        assert!(g.matches(&Scope::global()));
        assert!(g.matches(&Scope::new("anything/at/all")));
    }

    #[test]
    fn test_matches_non_global_does_not_match_global() {
        let s = Scope::new("s3/abc");
        assert!(!s.matches(&Scope::global()));
    }

    #[test]
    fn test_matches_deep_path() {
        let s = Scope::new("s3/bucket/path/to");
        assert!(s.matches(&Scope::new("s3/bucket/path/to/file")));
        assert!(s.matches(&Scope::new("s3/bucket/path/to")));
        assert!(!s.matches(&Scope::new("s3/bucket/path/tofile")));
        assert!(!s.matches(&Scope::new("s3/bucket/path")));
    }

    // ==================== Display ====================

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", Scope::new("s3/abc")), "s3/abc");
        assert_eq!(format!("{}", Scope::global()), "/");
    }

    // ==================== Serde ====================

    #[test]
    fn test_serde_roundtrip() {
        let s = Scope::new("s3/abc/def");
        let json = serde_json::to_string(&s).expect("serialize");
        assert_eq!(json, r#""s3/abc/def""#);
        let deserialized: Scope = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized, s);
    }

    #[test]
    fn test_serde_global() {
        let s = Scope::global();
        let json = serde_json::to_string(&s).expect("serialize");
        assert_eq!(json, r#""/""#);
        let deserialized: Scope = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized, s);
    }

    // ==================== Eq / Hash ====================

    #[test]
    fn test_equality() {
        assert_eq!(Scope::new("a/b"), Scope::new("a/b"));
        assert_ne!(Scope::new("a/b"), Scope::new("a/c"));
    }

    #[test]
    fn test_hash_consistency() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(Scope::new("a/b"));
        assert!(set.contains(&Scope::new("a/b")));
        assert!(!set.contains(&Scope::new("a/c")));
    }
}
