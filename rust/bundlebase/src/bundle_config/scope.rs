//! Bundle-config scope with full registry validation.
//!
//! This module re-exports `Scope` from `bundlebase_common` and provides
//! `TryFrom` impls and validated constructors that check against the
//! registered config scope registry.

// Re-export the Scope type from common
pub use bundlebase_common::config::Scope;

use crate::BundlebaseError;
use url::Url;

/// Validate and create a scope from user input, checking against registered scopes.
///
/// Handles both URLs (e.g., `s3://bucket/path`) and names (e.g., `s3/bucket`).
/// This is the validated constructor for user-facing input.
pub fn validated_scope(input: &str) -> Result<Scope, BundlebaseError> {
    let scopes = super::BundleConfig::all_scopes();
    Scope::new(input, &scopes)
}

/// Validate and create a scope from a URL, checking against registered scopes.
pub fn validated_scope_from_url(url: &Url) -> Result<Scope, BundlebaseError> {
    validated_scope(url.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== validated_scope() basic tests ====================

    #[test]
    fn test_new_rejects_empty() {
        assert!(validated_scope("").is_err());
    }

    #[test]
    fn test_new_rejects_leading_slash() {
        assert!(validated_scope("/").is_err());
        assert!(validated_scope("/s3").is_err());
        assert!(validated_scope("/s3/bucket").is_err())
    }

    #[test]
    fn test_new_rejects_unknown_scope() {
        assert!(validated_scope("xyz").is_err());
        assert!(validated_scope("xyz/bucket").is_err());
    }

    #[test]
    fn test_new_simple_name() {
        assert_eq!(validated_scope("s3").unwrap().as_str(), "s3");
    }

    #[test]
    fn test_new_compound_path() {
        assert_eq!(validated_scope("s3/bucket").unwrap().as_str(), "s3/bucket");
        assert_eq!(validated_scope("s3/bucket/path").unwrap().as_str(), "s3/bucket/path");
        #[cfg(feature = "connector-kaggle")]
        assert_eq!(validated_scope("kaggle/user/dataset").unwrap().as_str(), "kaggle/user/dataset");
    }

    // ==================== URL handling ====================

    #[test]
    fn test_new_normalizes_url() {
        assert_eq!(validated_scope("s3://bucket/path").unwrap().as_str(), "s3/bucket/path");
        assert_eq!(validated_scope("s3://bucket").unwrap().as_str(), "s3/bucket");
        assert_eq!(validated_scope("gs://my-bucket/data/").unwrap().as_str(), "gs/my-bucket/data");
    }

    #[test]
    fn test_new_scheme_only_url() {
        assert_eq!(validated_scope("s3://").unwrap().as_str(), "s3");
        #[cfg(feature = "connector-kaggle")]
        assert_eq!(validated_scope("kaggle://").unwrap().as_str(), "kaggle");
    }

    #[test]
    fn test_new_rejects_bare_scheme() {
        assert!(validated_scope("://").is_err());
    }

    // ==================== matches() tests ====================

    #[test]
    fn test_matches_exact() {
        let s = validated_scope("s3/abc").expect("valid scope");
        assert!(s.matches(&validated_scope("s3/abc").expect("valid scope")));
    }

    #[test]
    fn test_matches_boundary_prefix() {
        let s = validated_scope("s3/abc").expect("valid scope");
        assert!(s.matches(&validated_scope("s3/abc/def").expect("valid scope")));
    }

    #[test]
    fn test_matches_rejects_non_boundary() {
        let s = validated_scope("s3/abc").expect("valid scope");
        assert!(!s.matches(&validated_scope("s3/abcd").expect("valid scope")));
    }

    #[test]
    fn test_matches_rejects_shorter() {
        let s = validated_scope("s3/abc").expect("valid scope");
        assert!(!s.matches(&validated_scope("s3/ab").expect("valid scope")));
    }

    #[test]
    fn test_matches_deep_path() {
        let s = validated_scope("s3/bucket/path/to").expect("valid scope");
        assert!(s.matches(&validated_scope("s3/bucket/path/to/file").expect("valid scope")));
        assert!(s.matches(&validated_scope("s3/bucket/path/to").expect("valid scope")));
        assert!(!s.matches(&validated_scope("s3/bucket/path/tofile").expect("valid scope")));
        assert!(!s.matches(&validated_scope("s3/bucket/path").expect("valid scope")));
    }

    // ==================== Display ====================

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", validated_scope("s3/abc").expect("valid scope")), "s3/abc");
    }

    // ==================== Serde ====================

    #[test]
    fn test_serde_roundtrip() {
        let s = Scope::from_name("s3/abc/def").expect("valid scope");
        let json = serde_json::to_string(&s).expect("serialize");
        assert_eq!(json, r#""s3/abc/def""#);
        let deserialized: Scope = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized, s);
    }

    #[test]
    fn test_deserialize_accepts_valid_names() {
        // In common, deserialization uses from_name which accepts any well-formed name
        let s: Scope = serde_json::from_str(r#""s3/bucket""#).expect("deserialize");
        assert_eq!(s.as_str(), "s3/bucket");
    }

    // ==================== Eq / Hash ====================

    #[test]
    fn test_equality() {
        assert_eq!(Scope::from_name("s3/b").unwrap(), Scope::from_name("s3/b").unwrap());
        assert_ne!(Scope::from_name("s3/b").unwrap(), Scope::from_name("s3/c").unwrap());
    }

    #[test]
    fn test_hash_consistency() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(Scope::from_name("s3/b").unwrap());
        assert!(set.contains(&Scope::from_name("s3/b").unwrap()));
        assert!(!set.contains(&Scope::from_name("s3/c").unwrap()));
    }
}
