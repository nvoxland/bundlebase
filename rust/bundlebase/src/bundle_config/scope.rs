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
/// ```
/// use bundlebase::bundle_config::Scope;
/// let scope = Scope::try_from("s3").unwrap();
/// assert_eq!(scope.as_str(), "s3");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Scope(String);

/// Custom deserializer that validates scope strings, with backwards compatibility
/// for the legacy `"/"` global scope.
/// todo: remove legacy global scope
impl<'de> serde::Deserialize<'de> for Scope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        if s == "/" {
            log::warn!("Deserialized legacy global scope '/'. This entry will be ignored.");
            Ok(Scope(s))
        } else if s.is_empty() {
            Err(serde::de::Error::custom("Scope cannot be empty"))
        } else if s.starts_with('/') {
            Err(serde::de::Error::custom(format!(
                "Scope must not start with '/': got '{}'",
                s
            )))
        } else if s.contains("://") {
            Err(serde::de::Error::custom(format!(
                "Scope must not contain '://': got '{}'",
                s
            )))
        } else {
            Ok(Scope(s))
        }
    }
}

impl Scope {
    /// Create a scope from a correctly formatted string.
    ///
    /// The input must already be in canonical scope form:
    /// - Must not be empty
    /// - Must not start with `/`
    /// - Must not contain `://`
    ///
    /// Returns an error if the input is not in valid scope form.
    pub(crate) fn new(s: &str) -> Result<Self, BundlebaseError> {
        if s.is_empty() {
            return Err(BundlebaseError::from("Scope cannot be empty"));
        }
        if s.starts_with('/') {
            return Err(BundlebaseError::from(format!(
                "Scope must not start with '/': got '{}'",
                s
            )));
        }
        if s.contains("://") {
            return Err(BundlebaseError::from(format!(
                "Scope must not contain '://': got '{}'",
                s
            )));
        }
        Ok(Self(s.to_string()))
    }

    /// Parse any string — URL or name — into a Scope.
    ///
    /// Handles:
    /// - URLs like `"s3://bucket/path"` → iterates registered scopes, calls `url_to_name`
    /// - Names like `"s3"`, `"s3/bucket"`, `"/s3"` → validates first component against registered scopes
    ///
    /// # Examples
    /// ```
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
    pub(crate) fn parse(input: &str) -> Result<Self, BundlebaseError> {
        use super::BundleConfig;

        let trimmed = input.trim_start_matches('/');

        if trimmed.is_empty() {
            return Err(BundlebaseError::from(
                "Global scope '/' is not supported. Use a named scope like 's3' or 'kaggle'.",
            ));
        }

        if trimmed.contains("://") {
            // URL: iterate scopes, call url_to_name on each
            for config_scope in BundleConfig::all_scopes() {
                if let Some(name) = config_scope.url_to_name(trimmed) {
                    return Scope::new(&name);
                }
            }
            return Err(BundlebaseError::from(format!(
                "Unknown scope for URL '{}'",
                input
            )));
        }

        // Name: validate first path component against registered scopes
        let first = trimmed.split('/').next().unwrap_or(trimmed);

        for config_scope in BundleConfig::all_scopes() {
            if config_scope.name == first {
                return Scope::new(trimmed);
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
    /// ```
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
        Self::parse(s)
    }
}

impl TryFrom<&Url> for Scope {
    type Error = BundlebaseError;
    fn try_from(url: &Url) -> Result<Self, Self::Error> {
        Self::parse(url.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== new() tests ====================

    #[test]
    fn test_new_rejects_empty() {
        let err = Scope::new("").unwrap_err().to_string();
        assert!(err.contains("Scope cannot be empty"));
    }

    #[test]
    fn test_new_simple_path() {
        let s = Scope::new("s3/abc/def").expect("valid scope");
        assert_eq!(s.as_str(), "s3/abc/def");
    }

    #[test]
    fn test_new_rejects_leading_slash() {
        let err = Scope::new("/s3/abc").unwrap_err().to_string();
        assert!(err.contains("Scope must not start with '/'"));
    }

    #[test]
    fn test_new_rejects_global_slash() {
        let err = Scope::new("/").unwrap_err().to_string();
        assert!(err.contains("Scope must not start with '/'"));
    }

    #[test]
    fn test_new_rejects_url_scheme() {
        let err = Scope::new("s3://abc").unwrap_err().to_string();
        assert!(err.contains("Scope must not contain '://'"));
    }

    // ==================== normalize() tests ====================

    #[test]
    fn test_normalize_s3() {
        assert_eq!(Scope::try_from("s3://abc/def").unwrap().as_str(), "s3/abc/def");
    }

    #[test]
    fn test_parse_name() {
        assert_eq!(Scope::try_from("s3").unwrap().as_str(), "s3");
    }

    #[test]
    fn test_parse_name_with_leading_slash() {
        // Legacy format with leading slash should still work
        assert_eq!(Scope::try_from("/s3").unwrap().as_str(), "s3");
    }

    #[test]
    fn test_parse_bare_name() {
        // parse only works for registered scopes like "s3"
        assert_eq!(Scope::try_from("s3").unwrap().as_str(), "s3");
    }

    #[test]
    fn test_parse_compound_path() {
        // Compound paths like "s3/bucket" should work
        assert_eq!(Scope::try_from("s3/bucket").unwrap().as_str(), "s3/bucket");
        assert_eq!(Scope::try_from("s3/bucket/path").unwrap().as_str(), "s3/bucket/path");
        assert_eq!(Scope::try_from("kaggle/user/dataset").unwrap().as_str(), "kaggle/user/dataset");
    }

    #[test]
    fn test_parse_compound_path_with_leading_slash() {
        assert_eq!(Scope::try_from("/s3/bucket").unwrap().as_str(), "s3/bucket");
    }

    #[test]
    fn test_parse_unknown_scope() {
        // Unknown scope names should return an error
        assert!(Scope::try_from("xyz").is_err());
    }

    #[test]
    fn test_parse_unknown_compound() {
        // Compound path with unknown prefix should error
        assert!(Scope::try_from("xyz/bucket").is_err());
    }

    #[test]
    fn test_normalize_empty_errors() {
        assert!(Scope::try_from("").is_err());
    }

    #[test]
    fn test_normalize_slash_errors() {
        assert!(Scope::try_from("/").is_err());
    }

    #[test]
    fn test_normalize_already_normalized() {
        assert_eq!(Scope::try_from("s3://bucket").unwrap().as_str(), "s3/bucket");
    }

    #[test]
    fn test_normalize_gs() {
        assert_eq!(Scope::try_from("gs://my-bucket/data/").unwrap().as_str(), "gs/my-bucket/data");
    }

    #[test]
    fn test_normalize_scheme_only() {
        assert_eq!(Scope::try_from("s3://").unwrap().as_str(), "s3");
    }

    #[test]
    fn test_normalize_kaggle() {
        assert_eq!(Scope::try_from("kaggle://").unwrap().as_str(), "kaggle");
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
        // Leading slash
        assert!(serde_json::from_str::<Scope>(r#""/s3""#).is_err());
        // URL scheme
        assert!(serde_json::from_str::<Scope>(r#""s3://bucket""#).is_err());
    }

    #[test]
    fn test_deserialize_legacy_global_scope() {
        // "/" is accepted for backwards compat but logged as warning
        let s: Scope = serde_json::from_str(r#""/""#).expect("legacy scope");
        assert_eq!(s.as_str(), "/");
    }

    // ==================== Eq / Hash ====================

    #[test]
    fn test_equality() {
        assert_eq!(Scope::new("a/b").expect("valid scope"), Scope::new("a/b").expect("valid scope"));
        assert_ne!(Scope::new("a/b").expect("valid scope"), Scope::new("a/c").expect("valid scope"));
    }

    #[test]
    fn test_hash_consistency() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(Scope::new("a/b").expect("valid scope"));
        assert!(set.contains(&Scope::new("a/b").expect("valid scope")));
        assert!(!set.contains(&Scope::new("a/c").expect("valid scope")));
    }
}
