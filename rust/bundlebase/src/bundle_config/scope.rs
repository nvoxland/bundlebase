use crate::BundlebaseError;
use serde::Serialize;
use std::fmt;
use url::Url;

/// A normalized, boundary-aware configuration scope.
///
/// Scopes identify which path prefix a config value applies to. They are always
/// stored in a canonical form:
/// - No leading `/` (paths are normalized: `s3://bucket` → `s3/bucket`)
/// - No `://` sequences
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
/// Use `TryFrom<&str>` to create a scope from user input:
/// ```no_run
/// use bundlebase::bundle_config::Scope;
/// let scope = Scope::try_from("s3").unwrap();
/// assert_eq!(scope.as_str(), "s3");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Scope(String);

/// Custom deserializer that validates scope strings via `Scope::new()`.
impl<'de> serde::Deserialize<'de> for Scope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Scope::new(&s).map_err(|e| serde::de::Error::custom(e.to_string()))
    }
}

impl Scope {
    /// Create a validated Scope from any string — URL or name.
    ///
    /// Handles:
    /// - URLs like `"s3://bucket/path"` → resolves via registered scopes' `url_to_name`
    /// - Names like `"s3"`, `"s3/bucket"` → validates first component against registered scopes
    ///
    /// # Examples
    /// ```no_run
    /// use bundlebase::bundle_config::Scope;
    /// let scope = Scope::try_from("s3://bucket/path").unwrap();
    /// assert_eq!(scope.as_str(), "s3/bucket/path");
    ///
    /// let scope = Scope::try_from("s3").unwrap();
    /// assert_eq!(scope.as_str(), "s3");
    ///
    /// let scope = Scope::try_from("s3/bucket").unwrap();
    /// assert_eq!(scope.as_str(), "s3/bucket");
    /// ```
    pub(crate) fn new(input: &str) -> Result<Self, BundlebaseError> {
        use super::BundleConfig;

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
            // URL: iterate scopes, call url_to_name on each
            for config_scope in BundleConfig::all_scopes() {
                if let Some(name) = config_scope.url_to_name(input) {
                    return Ok(Self::new(&name)?);
                }
            }
            return Err(BundlebaseError::from(format!(
                "Unknown scope for URL '{}'",
                input
            )));
        }

        // Name: validate first path component against registered scopes
        let first = input.split('/').next().unwrap_or(input);

        for config_scope in BundleConfig::all_scopes() {
            if config_scope.name == first {
                return Ok(Self(input.to_string()));
            }
        }
        Err(BundlebaseError::from(format!("Unknown scope: {}", input)))
    }

    /// Check whether this scope matches a query scope.
    ///
    /// A scope matches a query if:
    /// - Exact match: this scope equals the query
    /// - Boundary prefix: the query starts with this scope AND the next
    ///   character in the query is `/`
    ///
    /// # Examples
    /// ```no_run
    /// use bundlebase::bundle_config::Scope;
    /// let s = Scope::try_from("s3/abc").unwrap();
    /// assert!(s.matches(&Scope::try_from("s3/abc").unwrap()));       // exact
    /// assert!(s.matches(&Scope::try_from("s3/abc/def").unwrap()));   // boundary prefix
    /// assert!(!s.matches(&Scope::try_from("s3/abcd").unwrap()));     // NOT boundary
    /// assert!(!s.matches(&Scope::try_from("s3/ab").unwrap()));       // too short
    /// ```
    pub fn matches(&self, query: &Scope) -> bool {
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

impl TryFrom<&str> for Scope {
    type Error = BundlebaseError;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl TryFrom<&Url> for Scope {
    type Error = BundlebaseError;
    fn try_from(url: &Url) -> Result<Self, Self::Error> {
        Self::new(url.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== new() basic tests ====================

    #[test]
    fn test_new_rejects_empty() {
        assert!(Scope::new("").is_err());
    }

    #[test]
    fn test_new_rejects_leading_slash() {
        assert!(Scope::new("/").is_err());
        assert!(Scope::new("/s3").is_err());
        assert!(Scope::new("/s3/bucket").is_err())
    }

    #[test]
    fn test_new_rejects_unknown_scope() {
        assert!(Scope::new("xyz").is_err());
        assert!(Scope::new("xyz/bucket").is_err());
    }

    #[test]
    fn test_new_simple_name() {
        assert_eq!(Scope::new("s3").unwrap().as_str(), "s3");
    }

    #[test]
    fn test_new_compound_path() {
        assert_eq!(Scope::new("s3/bucket").unwrap().as_str(), "s3/bucket");
        assert_eq!(Scope::new("s3/bucket/path").unwrap().as_str(), "s3/bucket/path");
        assert_eq!(Scope::new("kaggle/user/dataset").unwrap().as_str(), "kaggle/user/dataset");
    }

    // ==================== new() URL handling ====================

    #[test]
    fn test_new_normalizes_url() {
        assert_eq!(Scope::new("s3://bucket/path").unwrap().as_str(), "s3/bucket/path");
        assert_eq!(Scope::new("s3://bucket").unwrap().as_str(), "s3/bucket");
        assert_eq!(Scope::new("gs://my-bucket/data/").unwrap().as_str(), "gs/my-bucket/data");
    }

    #[test]
    fn test_new_scheme_only_url() {
        assert_eq!(Scope::new("s3://").unwrap().as_str(), "s3");
        assert_eq!(Scope::new("kaggle://").unwrap().as_str(), "kaggle");
    }

    #[test]
    fn test_new_rejects_bare_scheme() {
        // "://" without a valid parseable URL should error
        assert!(Scope::new("://").is_err());
    }

    // ==================== matches() tests ====================

    #[test]
    fn test_matches_exact() {
        let s = Scope::new("s3/abc").expect("valid scope");
        assert!(s.matches(&Scope::new("s3/abc").expect("valid scope")));
    }

    #[test]
    fn test_matches_boundary_prefix() {
        let s = Scope::new("s3/abc").expect("valid scope");
        assert!(s.matches(&Scope::new("s3/abc/def").expect("valid scope")));
    }

    #[test]
    fn test_matches_rejects_non_boundary() {
        let s = Scope::new("s3/abc").expect("valid scope");
        assert!(!s.matches(&Scope::new("s3/abcd").expect("valid scope")));
    }

    #[test]
    fn test_matches_rejects_shorter() {
        let s = Scope::new("s3/abc").expect("valid scope");
        assert!(!s.matches(&Scope::new("s3/ab").expect("valid scope")));
    }

    #[test]
    fn test_matches_deep_path() {
        let s = Scope::new("s3/bucket/path/to").expect("valid scope");
        assert!(s.matches(&Scope::new("s3/bucket/path/to/file").expect("valid scope")));
        assert!(s.matches(&Scope::new("s3/bucket/path/to").expect("valid scope")));
        assert!(!s.matches(&Scope::new("s3/bucket/path/tofile").expect("valid scope")));
        assert!(!s.matches(&Scope::new("s3/bucket/path").expect("valid scope")));
    }

    // ==================== Display ====================

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", Scope::new("s3/abc").expect("valid scope")), "s3/abc");
    }

    // ==================== Serde ====================

    #[test]
    fn test_serde_roundtrip() {
        let s = Scope::new("s3/abc/def").expect("valid scope");
        let json = serde_json::to_string(&s).expect("serialize");
        assert_eq!(json, r#""s3/abc/def""#);
        let deserialized: Scope = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized, s);
    }

    #[test]
    fn test_deserialize_rejects_invalid() {
        // Empty string
        assert!(serde_json::from_str::<Scope>(r#""""#).is_err());
        // Unknown scope
        assert!(serde_json::from_str::<Scope>(r#""xyz/bucket""#).is_err());
    }

    #[test]
    fn test_deserialize_rejects_global_scope() {
        assert!(serde_json::from_str::<Scope>(r#""/""#).is_err());
    }

    // ==================== Eq / Hash ====================

    #[test]
    fn test_equality() {
        assert_eq!(Scope::new("s3/b").expect("valid scope"), Scope::new("s3/b").expect("valid scope"));
        assert_ne!(Scope::new("s3/b").expect("valid scope"), Scope::new("s3/c").expect("valid scope"));
    }

    #[test]
    fn test_hash_consistency() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(Scope::new("s3/b").expect("valid scope"));
        assert!(set.contains(&Scope::new("s3/b").expect("valid scope")));
        assert!(!set.contains(&Scope::new("s3/c").expect("valid scope")));
    }
}
