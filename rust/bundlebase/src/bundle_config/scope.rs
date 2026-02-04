use crate::BundlebaseError;
use serde::{Deserialize, Serialize};
use std::fmt;

/// A normalized, boundary-aware configuration scope.
///
/// Scopes identify which path prefix a config value applies to. They are always
/// stored in a canonical form:
/// - Always start with `/`
/// - No `://` sequences (paths are normalized: `s3://bucket` → `/s3/bucket`)
/// - No trailing `/` (except the global scope `/`)
/// - The global scope `/` matches everything
///
/// # Matching
///
/// Scopes match on complete `/` boundaries:
/// - `/s3/abc` matches `/s3/abc` (exact)
/// - `/s3/abc` matches `/s3/abc/def` (boundary prefix: next char is `/`)
/// - `/s3/abc` does NOT match `/s3/abcd` (next char is `d`, not `/`)
/// - `/` (global) matches everything
///
/// # Constructors
///
/// - [`Scope::new`] — requires pre-normalized input (must start with `/`, no `://`)
/// - [`Scope::from_path`] — validates against registered scopes and normalizes
/// - [`Scope::normalize`] — normalizes a path or raw string into scope form
/// - [`Scope::global`] — returns the global scope `/`
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Scope(String);

impl Scope {
    /// Create a scope from a correctly formatted string.
    ///
    /// The input must already be in canonical scope form:
    /// - Must start with `/`
    /// - Must not contain `://`
    ///
    /// # Panics
    /// Panics if the input is not in valid scope form.
    ///
    /// # Examples
    /// ```
    /// use bundlebase::bundle_config::Scope;
    /// let s = Scope::new("/s3/bucket/path");
    /// assert_eq!(s.as_str(), "/s3/bucket/path");
    ///
    /// let global = Scope::new("/");
    /// assert!(global.is_global());
    /// ```
    pub fn new(s: &str) -> Self {
        assert!(s.starts_with('/'), "Scope must start with '/': got '{}'", s);
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
    /// assert_eq!(scope.as_str(), "/s3/bucket/path");
    /// ```
    pub fn from_path(path: &str) -> Result<Self, BundlebaseError> {
        use super::BundleConfig;

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

    /// Normalize a path or raw string into a scope.
    ///
    /// This is the basic normalizer that converts `://` sequences into `/`
    /// path separators. Unlike [`from_path`], it does not validate against
    /// registered scopes.
    ///
    /// Transformation rules:
    /// - `"s3://abc/def"` → `/s3/abc/def`
    /// - `"s3://bucket/"` → `/s3/bucket`
    /// - `"xyz"` → `/xyz`
    /// - `""` → `/` (global)
    /// - `"/"` → `/` (global)
    ///
    /// # Examples
    /// ```
    /// use bundlebase::bundle_config::Scope;
    /// assert_eq!(Scope::normalize("s3://abc/def").as_str(), "/s3/abc/def");
    /// assert_eq!(Scope::normalize("s3://bucket/").as_str(), "/s3/bucket");
    /// assert_eq!(Scope::normalize("").as_str(), "/");
    /// assert_eq!(Scope::normalize("/").as_str(), "/");
    /// assert_eq!(Scope::normalize("xyz").as_str(), "/xyz");
    /// ```
    pub fn normalize(path: &str) -> Self {
        if path.is_empty() {
            return Self::global();
        }

        // Replace "://" with "/"
        let normalized = if let Some(idx) = path.find("://") {
            format!("/{}{}", &path[..idx], &path[idx + 2..])
        } else if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{}", path)
        };

        // Strip trailing slash (unless it's just "/")
        let normalized = if normalized.len() > 1 && normalized.ends_with('/') {
            &normalized[..normalized.len() - 1]
        } else {
            &normalized
        };

        Self(normalized.to_string())
    }

    /// Returns the global scope `/`, which matches everything.
    pub fn global() -> Self {
        Self("/".to_string())
    }

    /// Returns `true` if this is the global scope `/`.
    pub fn is_global(&self) -> bool {
        self.0 == "/"
    }

    /// Check whether this scope matches a query scope.
    ///
    /// A scope matches a query if:
    /// - This scope is global (`/`) — matches everything
    /// - Exact match: this scope equals the query
    /// - Boundary prefix: the query starts with this scope AND the next
    ///   character in the query is `/`
    ///
    /// # Examples
    /// ```
    /// use bundlebase::bundle_config::Scope;
    /// let s = Scope::new("/s3/abc");
    /// assert!(s.matches(&Scope::new("/s3/abc")));       // exact
    /// assert!(s.matches(&Scope::new("/s3/abc/def")));    // boundary prefix
    /// assert!(!s.matches(&Scope::new("/s3/abcd")));      // NOT boundary
    /// assert!(!s.matches(&Scope::new("/s3/ab")));        // too short
    ///
    /// let global = Scope::global();
    /// assert!(global.matches(&Scope::new("/anything")));
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
    /// Normalize a raw string (path or scope path) into a Scope.
    ///
    /// This uses the same normalization as `normalize`:
    /// - `"s3://bucket/"` → `/s3/bucket`
    /// - `""` → `/` (global)
    /// - `"/s3/bucket"` → `/s3/bucket` (already normalized)
    fn from(s: &str) -> Self {
        Self::normalize(s)
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
    fn test_new_simple_path() {
        let s = Scope::new("/s3/abc/def");
        assert_eq!(s.as_str(), "/s3/abc/def");
        assert!(!s.is_global());
    }

    #[test]
    #[should_panic(expected = "Scope must start with '/'")]
    fn test_new_rejects_no_slash() {
        Scope::new("s3/abc");
    }

    #[test]
    #[should_panic(expected = "Scope must not contain '://'")]
    fn test_new_rejects_url_scheme() {
        Scope::new("/s3://abc");
    }

    // ==================== normalize() tests ====================

    #[test]
    fn test_normalize_s3() {
        assert_eq!(Scope::normalize("s3://abc/def").as_str(), "/s3/abc/def");
    }

    #[test]
    fn test_normalize_trailing_slash() {
        assert_eq!(Scope::normalize("s3://bucket/").as_str(), "/s3/bucket");
    }

    #[test]
    fn test_normalize_bare_name() {
        assert_eq!(Scope::normalize("xyz").as_str(), "/xyz");
    }

    #[test]
    fn test_normalize_empty() {
        assert_eq!(Scope::normalize("").as_str(), "/");
        assert!(Scope::normalize("").is_global());
    }

    #[test]
    fn test_normalize_slash() {
        assert_eq!(Scope::normalize("/").as_str(), "/");
        assert!(Scope::normalize("/").is_global());
    }

    #[test]
    fn test_normalize_already_normalized() {
        assert_eq!(Scope::normalize("/s3/bucket").as_str(), "/s3/bucket");
    }

    #[test]
    fn test_normalize_gs() {
        assert_eq!(Scope::normalize("gs://my-bucket/data/").as_str(), "/gs/my-bucket/data");
    }

    #[test]
    fn test_normalize_scheme_only() {
        assert_eq!(Scope::normalize("s3://").as_str(), "/s3");
    }

    #[test]
    fn test_normalize_kaggle() {
        assert_eq!(Scope::normalize("kaggle://").as_str(), "/kaggle");
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
        let s = Scope::new("/s3/bucket");
        assert!(!s.is_global());
    }

    // ==================== matches() tests ====================

    #[test]
    fn test_matches_exact() {
        let s = Scope::new("/s3/abc");
        assert!(s.matches(&Scope::new("/s3/abc")));
    }

    #[test]
    fn test_matches_boundary_prefix() {
        let s = Scope::new("/s3/abc");
        assert!(s.matches(&Scope::new("/s3/abc/def")));
    }

    #[test]
    fn test_matches_rejects_non_boundary() {
        let s = Scope::new("/s3/abc");
        assert!(!s.matches(&Scope::new("/s3/abcd")));
    }

    #[test]
    fn test_matches_rejects_shorter() {
        let s = Scope::new("/s3/abc");
        assert!(!s.matches(&Scope::new("/s3/ab")));
    }

    #[test]
    fn test_matches_global_matches_everything() {
        let g = Scope::global();
        assert!(g.matches(&Scope::new("/s3/abc")));
        assert!(g.matches(&Scope::global()));
        assert!(g.matches(&Scope::new("/anything/at/all")));
    }

    #[test]
    fn test_matches_non_global_does_not_match_global() {
        let s = Scope::new("/s3/abc");
        assert!(!s.matches(&Scope::global()));
    }

    #[test]
    fn test_matches_deep_path() {
        let s = Scope::new("/s3/bucket/path/to");
        assert!(s.matches(&Scope::new("/s3/bucket/path/to/file")));
        assert!(s.matches(&Scope::new("/s3/bucket/path/to")));
        assert!(!s.matches(&Scope::new("/s3/bucket/path/tofile")));
        assert!(!s.matches(&Scope::new("/s3/bucket/path")));
    }

    // ==================== Display ====================

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", Scope::new("/s3/abc")), "/s3/abc");
        assert_eq!(format!("{}", Scope::global()), "/");
    }

    // ==================== Serde ====================

    #[test]
    fn test_serde_roundtrip() {
        let s = Scope::new("/s3/abc/def");
        let json = serde_json::to_string(&s).expect("serialize");
        assert_eq!(json, r#""/s3/abc/def""#);
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
        assert_eq!(Scope::new("/a/b"), Scope::new("/a/b"));
        assert_ne!(Scope::new("/a/b"), Scope::new("/a/c"));
    }

    #[test]
    fn test_hash_consistency() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(Scope::new("/a/b"));
        assert!(set.contains(&Scope::new("/a/b")));
        assert!(!set.contains(&Scope::new("/a/c")));
    }
}
