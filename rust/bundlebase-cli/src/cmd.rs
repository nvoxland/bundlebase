//! CLI subcommand modules and shared argument types.

pub mod create_cmd;
pub mod extend_cmd;
pub mod list_bundles_cmd;
pub mod mcp_cmd;
pub mod query_cmd;
pub mod repl_cmd;
pub mod report_cmd;
pub mod server_cmd;
pub mod setup_agent_cmd;
pub mod upgrade_bundle_cmd;

use bundlebase::{Bundle, BundleFacade, PassedBundleConfig};
use bundlebase_common::BundlebaseError;
use clap::Args;
use std::path::Path;
use std::sync::Arc;
use tracing::debug;

/// Shared flags for opening an existing bundle.
#[derive(Args, Debug, Clone)]
pub struct BundleArgs {
    /// Path or URL to the bundle
    #[arg(long)]
    pub bundle: String,

    /// Open bundle in read-only mode (default: false).
    /// When true, only SELECT and EXPLAIN commands are allowed.
    #[arg(long, default_value = "false")]
    pub read_only: bool,

    /// Path to a YAML or JSON config file
    #[arg(long)]
    pub config: Option<String>,
}

/// Open an existing bundle based on the shared flags.
///
/// If the bundle doesn't exist, returns a helpful error suggesting `bundlebase create`.
pub async fn open_bundle(args: &BundleArgs) -> Result<Arc<dyn BundleFacade>, BundlebaseError> {
    open_bundle_with_create_hint(args, "To create a new bundle, use 'bundlebase create'.").await
}

/// Like [`open_bundle`], but lets the caller customize the hint shown when the
/// bundle doesn't exist. Used by commands like `repl` and `serve` that have
/// their own `--create` flag and want to point at it instead of the separate
/// `bundlebase create` subcommand.
pub async fn open_bundle_with_create_hint(
    args: &BundleArgs,
    create_hint: &str,
) -> Result<Arc<dyn BundleFacade>, BundlebaseError> {
    let config = load_config(args.config.as_deref())?;

    // Demoted from `info!` to `debug!` — every CLI invocation printed
    // this to stderr, which is noise in scripts that pipe many queries.
    // The information is still there at `--log-level debug`.
    let result = if args.read_only {
        debug!("Opening bundle in read-only mode: {}", args.bundle);
        Bundle::open(&args.bundle, config)
            .await
            .map(|b| b as Arc<dyn BundleFacade>)
    } else {
        debug!("Opening bundle in read-write mode: {}", args.bundle);
        match Bundle::open(&args.bundle, config).await {
            Ok(b) => b.extend(None).await.map(|b| b as Arc<dyn BundleFacade>),
            Err(e) => Err(e),
        }
    };

    result.map_err(|e| {
        let msg = e.to_string();
        if msg.contains("init.yaml") || msg.contains("not found") || msg.contains("No such file") || msg.contains("does not exist") {
            BundlebaseError::from(format!(
                "No bundle found at '{}'. {}\n\nUnderlying error: {}",
                args.bundle, create_hint, msg
            ))
        } else {
            e
        }
    })
}

/// Load a `PassedBundleConfig` from a YAML or JSON file, if a path is provided.
pub fn load_config(path: Option<&str>) -> Result<Option<PassedBundleConfig>, BundlebaseError> {
    let path = match path {
        Some(p) => p,
        None => return Ok(None),
    };

    let contents = std::fs::read_to_string(path).map_err(|e| {
        BundlebaseError::from(format!("Failed to read config file '{}': {}", path, e))
    })?;

    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    let config: PassedBundleConfig = match ext {
        "json" => serde_json::from_str(&contents).map_err(|e| {
            BundlebaseError::from(format!("Failed to parse JSON config '{}': {}", path, e))
        })?,
        "yaml" | "yml" => serde_yaml_ng::from_str(&contents).map_err(|e| {
            BundlebaseError::from(format!("Failed to parse YAML config '{}': {}", path, e))
        })?,
        _ => {
            return Err(BundlebaseError::from(format!(
                "Unrecognized config file extension '{}'. Use .json, .yaml, or .yml",
                ext
            )))
        }
    };

    Ok(Some(config))
}

/// Generate a commit message from the SQL command, truncating if too long.
pub fn auto_commit_message(sql: &str) -> String {
    let normalized: String = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.len() <= 72 {
        normalized
    } else {
        format!("{}...", &normalized[..69])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init() {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            bundlebase_catalog::init();
        });
    }

    #[tokio::test]
    async fn test_open_nonexistent_bundle_suggests_create() {
        init();
        let args = BundleArgs {
            bundle: "memory:///nonexistent_bundle_test".to_string(),
            read_only: false,
            config: None,
        };
        let result = open_bundle(&args).await;
        assert!(result.is_err(), "Expected error opening nonexistent bundle");
        let msg = result.err().unwrap().to_string();
        assert!(
            msg.contains("No bundle found"),
            "Expected 'No bundle found' in error, got: {}",
            msg
        );
        assert!(
            msg.contains("bundlebase create"),
            "Expected suggestion to use 'bundlebase create' in error, got: {}",
            msg
        );
    }

    #[tokio::test]
    async fn test_open_nonexistent_bundle_readonly_suggests_create() {
        init();
        let args = BundleArgs {
            bundle: "memory:///nonexistent_readonly_test".to_string(),
            read_only: true,
            config: None,
        };
        let result = open_bundle(&args).await;
        assert!(result.is_err(), "Expected error opening nonexistent bundle");
        let msg = result.err().unwrap().to_string();
        assert!(
            msg.contains("No bundle found"),
            "Expected 'No bundle found' in error, got: {}",
            msg
        );
        assert!(
            msg.contains("bundlebase create"),
            "Expected suggestion to use 'bundlebase create' in error, got: {}",
            msg
        );
    }

    #[tokio::test]
    async fn test_create_existing_bundle_suggests_extend() {
        init();
        let url = format!(
            "memory:///create_existing_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("System time before UNIX epoch")
                .as_nanos()
        );

        // Create a bundle first
        let builder = bundlebase::BundleBuilder::create(&url, None)
            .await
            .expect("Failed to create bundle");
        builder.commit("Initial").await.expect("Failed to commit");

        // Now try to create again — should get a helpful error
        let err = create_cmd::run(create_cmd::CreateArgs {
            bundle: url.clone(),
            sql: None,
            format: bundlebase_cli::OutputFormat::Table,
            message: None,
            config: None,
        })
        .await
        .unwrap_err();

        let msg = err.to_string();
        assert!(
            msg.contains("already exists"),
            "Expected 'already exists' in error, got: {}",
            msg
        );
        assert!(
            msg.contains("bundlebase extend"),
            "Expected suggestion to use 'bundlebase extend' in error, got: {}",
            msg
        );
    }

    #[tokio::test]
    async fn test_create_new_bundle_succeeds() {
        init();
        let url = format!(
            "memory:///create_new_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("System time before UNIX epoch")
                .as_nanos()
        );

        let result = create_cmd::run(create_cmd::CreateArgs {
            bundle: url,
            sql: None,
            format: bundlebase_cli::OutputFormat::Table,
            message: None,
            config: None,
        })
        .await;

        assert!(
            result.is_ok(),
            "Expected create to succeed, got: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    async fn test_extend_nonexistent_bundle_suggests_create() {
        init();
        let err = extend_cmd::run(extend_cmd::ExtendArgs {
            bundle: "memory:///extend_nonexistent_test".to_string(),
            to: None,
            sql: Some("SHOW COUNT".to_string()),
            format: bundlebase_cli::OutputFormat::Table,
            message: None,
            config: None,
        })
        .await
        .unwrap_err();

        let msg = err.to_string();
        assert!(
            msg.contains("No bundle found"),
            "Expected 'No bundle found' in error, got: {}",
            msg
        );
        assert!(
            msg.contains("bundlebase create"),
            "Expected suggestion to use 'bundlebase create' in error, got: {}",
            msg
        );
    }

    #[tokio::test]
    async fn test_repl_create_flag_creates_bundle() {
        init();
        let url = format!(
            "memory:///repl_create_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("System time before UNIX epoch")
                .as_nanos()
        );

        // Pipe empty stdin so the REPL falls through to one-shot mode
        // and exits without trying to enter raw terminal mode.
        // (stdin is non-tty under cargo test, so this works automatically.)
        let result = repl_cmd::run(repl_cmd::ReplArgs {
            bundle: BundleArgs {
                bundle: url.clone(),
                read_only: false,
                config: None,
            },
            format: bundlebase_cli::OutputFormat::Table,
            create: true,
        })
        .await;

        assert!(
            result.is_ok(),
            "Expected repl --create to succeed, got: {:?}",
            result.err()
        );

        // Bundle should now be persisted and re-openable
        let open_args = BundleArgs {
            bundle: url,
            read_only: true,
            config: None,
        };
        assert!(
            open_bundle(&open_args).await.is_ok(),
            "Expected bundle to be openable after repl --create"
        );
    }

    #[tokio::test]
    async fn test_repl_open_missing_bundle_suggests_create_flag() {
        init();
        let args = BundleArgs {
            bundle: "memory:///repl_open_missing_test".to_string(),
            read_only: false,
            config: None,
        };
        let err = repl_cmd::run(repl_cmd::ReplArgs {
            bundle: args,
            format: bundlebase_cli::OutputFormat::Table,
            create: false,
        })
        .await
        .unwrap_err();

        let msg = err.to_string();
        assert!(
            msg.contains("--create"),
            "Expected error to mention --create flag, got: {}",
            msg
        );
    }

    #[tokio::test]
    async fn test_serve_create_existing_bundle_errors() {
        init();
        let url = format!(
            "memory:///serve_create_existing_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("System time before UNIX epoch")
                .as_nanos()
        );

        // Create the bundle first
        let builder = bundlebase::BundleBuilder::create(&url, None)
            .await
            .expect("Failed to create bundle");
        builder.commit("Initial").await.expect("Failed to commit");

        let err = server_cmd::run(server_cmd::ServerArgs {
            bundle: BundleArgs {
                bundle: url,
                read_only: false,
                config: None,
            },
            host: "127.0.0.1".to_string(),
            port: Some(0),
            create: true,
        })
        .await
        .unwrap_err();

        let msg = err.to_string();
        assert!(
            msg.contains("already exists"),
            "Expected 'already exists' error from serve --create, got: {}",
            msg
        );
    }

    #[tokio::test]
    async fn test_repl_create_existing_bundle_errors() {
        init();
        let url = format!(
            "memory:///repl_create_existing_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("System time before UNIX epoch")
                .as_nanos()
        );

        // Create the bundle first
        let builder = bundlebase::BundleBuilder::create(&url, None)
            .await
            .expect("Failed to create bundle");
        builder.commit("Initial").await.expect("Failed to commit");

        let err = repl_cmd::run(repl_cmd::ReplArgs {
            bundle: BundleArgs {
                bundle: url,
                read_only: false,
                config: None,
            },
            format: bundlebase_cli::OutputFormat::Table,
            create: true,
        })
        .await
        .unwrap_err();

        let msg = err.to_string();
        assert!(
            msg.contains("already exists"),
            "Expected 'already exists' in error, got: {}",
            msg
        );
    }

    #[tokio::test]
    async fn test_extend_with_to_flag() {
        init();
        let source_url = format!(
            "memory:///extend_to_source_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("System time before UNIX epoch")
                .as_nanos()
        );
        let target_url = format!(
            "memory:///extend_to_target_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("System time before UNIX epoch")
                .as_nanos()
        );

        // Create source bundle
        let builder = bundlebase::BundleBuilder::create(&source_url, None)
            .await
            .expect("Failed to create source bundle");
        builder.commit("Initial").await.expect("Failed to commit");

        // Extend to a new location
        let result = extend_cmd::run(extend_cmd::ExtendArgs {
            bundle: source_url,
            to: Some(target_url),
            sql: Some("SHOW COUNT".to_string()),
            format: bundlebase_cli::OutputFormat::Table,
            message: None,
            config: None,
        })
        .await;

        assert!(
            result.is_ok(),
            "Expected extend --to to succeed, got: {:?}",
            result.err()
        );
    }
}
