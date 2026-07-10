//! Safety classification for source URLs (bb-e0b.4 / bb-e0b.8).
//!
//! A bundle can be opened by someone other than its author. Sources that are
//! not copied into the bundle are re-fetched from their original URL on the
//! consumer's machine, so an untrusted bundle could:
//!
//! - reference a local path (`file://…`) and cause the consumer to read *their
//!   own* filesystem, or
//! - reference an internal endpoint (loopback, link-local, or a cloud
//!   metadata service like `169.254.169.254`) and turn the consumer into an
//!   SSRF proxy.
//!
//! [`classify_source_url`] labels a URL so callers can force local sources to
//! be copied into the bundle (never referenced) and reject internal targets.

use crate::system_config::is_internal_sources_allowed;
use crate::{BundlebaseError, ConfigProvider};
use url::{Host, Url};

/// How a source URL should be treated when it would otherwise be stored as a
/// bare reference in a bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlSafety {
    /// Safe to reference directly.
    Safe,
    /// A local-filesystem URL. It must be copied into the bundle rather than
    /// referenced, so a consumer never reads their own disk through it.
    LocalMustCopy,
    /// An internal/SSRF-prone target (loopback, link-local, cloud metadata).
    /// The contained string explains why.
    Blocked(String),
}

/// What a caller should do with a source URL after the safety policy is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceUrlAction {
    /// Use the URL directly (reference or fetch as normal).
    Reference,
    /// The URL is a local file; copy its bytes into the bundle instead of
    /// storing a bare reference.
    CopyLocal,
}

/// Apply the source-URL safety policy, honoring `system.allow_internal_sources`.
///
/// - Safe URLs → [`SourceUrlAction::Reference`].
/// - `file://` URLs → [`SourceUrlAction::CopyLocal`].
/// - Internal/metadata targets → an error, unless the operator has set
///   `system.allow_internal_sources = true` (then treated as `Reference`).
pub fn check_source_url(
    url: &Url,
    config: &dyn ConfigProvider,
) -> Result<SourceUrlAction, BundlebaseError> {
    match classify_source_url(url) {
        UrlSafety::Safe => Ok(SourceUrlAction::Reference),
        UrlSafety::LocalMustCopy => Ok(SourceUrlAction::CopyLocal),
        UrlSafety::Blocked(reason) => {
            if is_internal_sources_allowed(config)? {
                Ok(SourceUrlAction::Reference)
            } else {
                Err(reason.into())
            }
        }
    }
}

/// Render a URL for error messages with any userinfo (user:password) stripped,
/// so a credential embedded in the URL is never echoed into logs or errors.
fn redact_url(url: &Url) -> String {
    let mut u = url.clone();
    let _ = u.set_password(None);
    let _ = u.set_username("");
    u.to_string()
}

/// Classify a source URL for safe handling in a shared bundle.
pub fn classify_source_url(url: &Url) -> UrlSafety {
    // Local filesystem access must never be a bare reference in a shared bundle.
    if url.scheme() == "file" {
        return UrlSafety::LocalMustCopy;
    }

    let blocked = |reason: String| -> UrlSafety {
        UrlSafety::Blocked(format!(
            "Refusing source URL '{}': {} — not allowed as a source (would enable SSRF / \
             access to internal services when the bundle is opened).",
            redact_url(url),
            reason
        ))
    };

    match url.host() {
        Some(Host::Ipv4(ip)) => {
            if let Some(reason) = blocked_ipv4_reason(&ip) {
                return blocked(format!("{ip} is {reason}"));
            }
        }
        Some(Host::Ipv6(ip)) => {
            if let Some(reason) = blocked_ipv6_reason(&ip) {
                return blocked(format!("{ip} is {reason}"));
            }
        }
        Some(Host::Domain(name)) => {
            if let Some(reason) = blocked_hostname_reason(name) {
                return blocked(format!("host '{name}' {reason}"));
            }
        }
        None => {}
    }

    UrlSafety::Safe
}

/// Reason an IPv4 address is blocked, or `None` if it is allowed.
///
/// Private ranges (10/8, 192.168/16, 172.16/12) are deliberately *not* blocked:
/// per the project's threat model, trusting the network belongs to the
/// operator, so legitimate internal deployments must keep working. Only
/// self-referential / metadata targets are refused.
fn blocked_ipv4_reason(ip: &std::net::Ipv4Addr) -> Option<&'static str> {
    if ip.is_loopback() {
        Some("a loopback address")
    } else if ip.is_link_local() {
        // 169.254.0.0/16 — includes the 169.254.169.254 cloud metadata endpoint.
        Some("a link-local/metadata address")
    } else if ip.is_unspecified() {
        Some("the unspecified address")
    } else {
        None
    }
}

/// Reason an IPv6 address is blocked, or `None` if it is allowed.
fn blocked_ipv6_reason(ip: &std::net::Ipv6Addr) -> Option<&'static str> {
    if ip.is_loopback() {
        Some("a loopback address")
    } else if ip.is_unspecified() {
        Some("the unspecified address")
    } else if (ip.segments()[0] & 0xffc0) == 0xfe80 {
        // fe80::/10 link-local.
        Some("a link-local address")
    } else if let Some(v4) = ip.to_ipv4_mapped() {
        // ::ffff:a.b.c.d — an IPv4-mapped address must be judged as IPv4.
        blocked_ipv4_reason(&v4)
    } else {
        None
    }
}

/// Reason a hostname is blocked, or `None` if it is allowed.
fn blocked_hostname_reason(name: &str) -> Option<&'static str> {
    let lower = name.trim_end_matches('.').to_ascii_lowercase();
    match lower.as_str() {
        "localhost" => Some("resolves to loopback"),
        // Well-known cloud metadata hostnames.
        "metadata" | "metadata.google.internal" | "metadata.goog" => {
            Some("is a cloud metadata endpoint")
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify(u: &str) -> UrlSafety {
        classify_source_url(&Url::parse(u).expect("valid url"))
    }

    #[test]
    fn test_file_url_must_be_copied() {
        assert_eq!(classify("file:///etc/passwd"), UrlSafety::LocalMustCopy);
        assert_eq!(classify("file:///home/user/data.csv"), UrlSafety::LocalMustCopy);
    }

    #[test]
    fn test_public_http_is_safe() {
        assert_eq!(classify("https://example.com/data.csv"), UrlSafety::Safe);
        assert_eq!(classify("http://data.gov/x.parquet"), UrlSafety::Safe);
        assert_eq!(classify("s3://bucket/key"), UrlSafety::Safe);
    }

    #[test]
    fn test_cloud_metadata_ip_is_blocked() {
        assert!(matches!(
            classify("http://169.254.169.254/latest/meta-data/"),
            UrlSafety::Blocked(_)
        ));
    }

    #[test]
    fn test_loopback_is_blocked() {
        assert!(matches!(
            classify("http://127.0.0.1:8080/x"),
            UrlSafety::Blocked(_)
        ));
        assert!(matches!(classify("http://[::1]/x"), UrlSafety::Blocked(_)));
        assert!(matches!(
            classify("http://localhost/x"),
            UrlSafety::Blocked(_)
        ));
    }

    #[test]
    fn test_unspecified_and_link_local_blocked() {
        assert!(matches!(classify("http://0.0.0.0/x"), UrlSafety::Blocked(_)));
        assert!(matches!(
            classify("http://[fe80::1]/x"),
            UrlSafety::Blocked(_)
        ));
    }

    #[test]
    fn test_metadata_hostname_is_blocked() {
        assert!(matches!(
            classify("http://metadata.google.internal/computeMetadata/v1/"),
            UrlSafety::Blocked(_)
        ));
    }

    #[test]
    fn test_ipv4_mapped_loopback_is_blocked() {
        // ::ffff:127.0.0.1 must not sneak past the IPv6 path.
        assert!(matches!(
            classify("http://[::ffff:127.0.0.1]/x"),
            UrlSafety::Blocked(_)
        ));
    }

    #[test]
    fn test_private_ranges_are_allowed() {
        // Internal-network trust is the operator's responsibility per the threat
        // model, so private ranges stay usable.
        assert_eq!(classify("http://10.0.0.5/x"), UrlSafety::Safe);
        assert_eq!(classify("http://192.168.1.10/x"), UrlSafety::Safe);
    }

    #[test]
    fn test_blocked_message_does_not_leak_credentials() {
        // A userinfo credential in the URL must not be echoed into the error.
        if let UrlSafety::Blocked(msg) = classify("http://user:sekret@127.0.0.1/x") {
            assert!(!msg.contains("sekret"), "must not echo credentials: {msg}");
        } else {
            panic!("expected blocked");
        }
    }

    /// Minimal config that only answers `system.allow_internal_sources`.
    struct InternalCfg(bool);
    impl crate::ConfigProvider for InternalCfg {
        fn get_in_scope(
            &self,
            _scope: &crate::Scope,
            key: &crate::ConfigKey,
        ) -> Result<Option<String>, crate::BundlebaseError> {
            Ok(if self.0 && key.key == "allow_internal_sources" {
                Some("true".to_string())
            } else {
                None
            })
        }
    }

    #[test]
    fn test_check_blocks_internal_by_default() {
        let cfg = InternalCfg(false);
        let url = Url::parse("http://169.254.169.254/").expect("url");
        assert!(check_source_url(&url, &cfg).is_err());
    }

    #[test]
    fn test_check_allows_internal_when_opted_in() {
        // The escape hatch: system.allow_internal_sources=true permits it.
        let cfg = InternalCfg(true);
        let url = Url::parse("http://169.254.169.254/").expect("url");
        assert_eq!(
            check_source_url(&url, &cfg).expect("allowed"),
            SourceUrlAction::Reference
        );
    }

    #[test]
    fn test_check_file_is_copy_local_regardless_of_config() {
        let url = Url::parse("file:///data.csv").expect("url");
        assert_eq!(
            check_source_url(&url, &InternalCfg(false)).expect("ok"),
            SourceUrlAction::CopyLocal
        );
    }

    #[test]
    fn test_check_public_is_reference() {
        let url = Url::parse("https://example.com/x").expect("url");
        assert_eq!(
            check_source_url(&url, &InternalCfg(false)).expect("ok"),
            SourceUrlAction::Reference
        );
    }
}
