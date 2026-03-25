//! CLI subcommand modules and shared argument types.

pub mod create_cmd;
pub mod extend_cmd;
pub mod list_bundles_cmd;
pub mod mcp_cmd;
pub mod query_cmd;
pub mod repl_cmd;
pub mod server_cmd;
pub mod setup_agent_cmd;

use bundlebase::{Bundle, BundleFacade, PassedBundleConfig};
use bundlebase_common::BundlebaseError;
use clap::Args;
use std::path::Path;
use std::sync::Arc;
use tracing::info;

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
    let config = load_config(args.config.as_deref())?;

    let result = if args.read_only {
        info!("Opening bundle in read-only mode: {}", args.bundle);
        Bundle::open(&args.bundle, config).await.map(|b| b as Arc<dyn BundleFacade>)
    } else {
        info!("Opening bundle in read-write mode: {}", args.bundle);
        match Bundle::open(&args.bundle, config).await {
            Ok(b) => b.extend(None).await.map(|b| b as Arc<dyn BundleFacade>),
            Err(e) => Err(e),
        }
    };

    result.map_err(|e| {
        let msg = e.to_string();
        if msg.contains("init.yaml") || msg.contains("not found") || msg.contains("No such file") || msg.contains("does not exist") {
            BundlebaseError::from(format!(
                "No bundle found at '{}'. To create a new bundle, use 'bundlebase create'.\n\nUnderlying error: {}",
                args.bundle, msg
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

    let contents = std::fs::read_to_string(path)
        .map_err(|e| BundlebaseError::from(format!("Failed to read config file '{}': {}", path, e)))?;

    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    let config: PassedBundleConfig = match ext {
        "json" => serde_json::from_str(&contents)
            .map_err(|e| BundlebaseError::from(format!("Failed to parse JSON config '{}': {}", path, e)))?,
        "yaml" | "yml" => serde_yaml_ng::from_str(&contents)
            .map_err(|e| BundlebaseError::from(format!("Failed to parse YAML config '{}': {}", path, e)))?,
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
        INIT.call_once(|| { bundlebase_catalog::init(); });
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

        assert!(result.is_ok(), "Expected create to succeed, got: {:?}", result.err());
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

        assert!(result.is_ok(), "Expected extend --to to succeed, got: {:?}", result.err());
    }
}
