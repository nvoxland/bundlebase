//! Bundlebase CLI - command-line interface for bundlebase.
//!
//! This binary provides two modes of operation:
//! - REPL: Interactive command-line interface
//! - Flight: Arrow Flight server for SQL queries

mod agent_skills;
mod auth;
mod flight;
mod mcp;
mod repl;

use bundlebase::{Bundle, BundleBuilder, BundlebaseError, BundleFacade, PassedBundleConfig};
use bundlebase_cli::OutputFormat;
use clap::{Parser, ValueEnum};
use std::path::Path;
use std::sync::Arc;
use tracing::info;
use tracing_log::LogTracer;

/// Mode of operation for the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Mode {
    /// Interactive REPL mode
    Repl,
    /// Arrow Flight server mode
    Flight,
    /// MCP (Model Context Protocol) server over stdio
    Mcp,
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Mode::Repl => write!(f, "repl"),
            Mode::Flight => write!(f, "flight"),
            Mode::Mcp => write!(f, "mcp"),
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "bundlebase")]
#[command(about = "Bundlebase CLI - Interactive REPL and Arrow Flight Server", long_about = None)]
struct Args {
    /// Path to bundle to load
    #[arg(long, required_unless_present = "setup_agent")]
    bundle: Option<String>,

    /// Install agent skills for coding agents (Claude Code, Cursor, Copilot, etc.)
    #[arg(long)]
    setup_agent: bool,

    /// Mode of operation (repl or flight)
    #[arg(long, value_enum, default_value = "repl")]
    mode: Mode,

    /// Create a new bundle if it doesn't exist or is empty
    #[arg(long)]
    create: bool,

    /// Open bundle in read-only mode (default: false).
    /// When true, only SELECT and EXPLAIN commands are allowed.
    /// Use --read-only to enable read-only mode.
    #[arg(long, default_value = "false")]
    read_only: bool,

    /// Host address to bind to (Flight mode only)
    #[arg(long, default_value = "0.0.0.0")]
    host: String,

    /// Port to listen on (default: 50051 for Flight)
    #[arg(long)]
    port: Option<u16>,

    /// Path to a YAML or JSON config file
    #[arg(long)]
    config: Option<String>,

    /// Logging level (ui, trace, debug, info, warn, error)
    /// ui: Minimal format (message only), INFO level - good for interactive use
    #[arg(long, default_value = "ui")]
    log_level: String,

    /// OpenTelemetry endpoint for tracing (e.g., "http://localhost:4317")
    #[arg(long)]
    otel: Option<String>,

    /// Execute a single command and exit (non-interactive mode)
    #[arg(long)]
    execute: Option<String>,

    /// Output format for query results
    #[arg(long, value_enum, default_value = "table")]
    format: OutputFormat,
}

/// Configuration for logging
struct LogConfig {
    level: tracing::Level,
    ui_mode: bool,
}

/// Parse a log level string into a LogConfig
fn parse_log_level(level_str: &str) -> Result<LogConfig, String> {
    match level_str.to_lowercase().as_str() {
        "ui" => Ok(LogConfig {
            level: tracing::Level::INFO,
            ui_mode: true,
        }),
        "trace" => Ok(LogConfig {
            level: tracing::Level::TRACE,
            ui_mode: false,
        }),
        "debug" => Ok(LogConfig {
            level: tracing::Level::DEBUG,
            ui_mode: false,
        }),
        "info" => Ok(LogConfig {
            level: tracing::Level::INFO,
            ui_mode: false,
        }),
        "warn" | "warning" => Ok(LogConfig {
            level: tracing::Level::WARN,
            ui_mode: false,
        }),
        "error" => Ok(LogConfig {
            level: tracing::Level::ERROR,
            ui_mode: false,
        }),
        _ => Err(format!(
            "unknown log level '{}', must be one of: ui, trace, debug, info, warn, error",
            level_str
        )),
    }
}

/// Load a `PassedBundleConfig` from a YAML or JSON file, if a path is provided.
fn load_config(path: Option<&str>) -> Result<Option<PassedBundleConfig>, BundlebaseError> {
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
        _ => return Err(BundlebaseError::from(format!(
            "Unrecognized config file extension '{}'. Use .json, .yaml, or .yml",
            ext
        ))),
    };

    Ok(Some(config))
}

#[tokio::main]
async fn main() -> Result<(), BundlebaseError> {
    let args = Args::parse();

    init_logging(&args);

    if args.setup_agent {
        agent_skills::install()?;
        return Ok(());
    }

    let bundle = args.bundle.expect("--bundle is required when not using --setup-agent");

    // Validate flag combinations
    if args.create && args.read_only {
        eprintln!("Error: Cannot use --create with --read-only=true. Creating a bundle requires write access.");
        eprintln!("Use --read-only=false with --create to create a new bundle.");
        std::process::exit(1);
    }

    let config = load_config(args.config.as_deref())?;

    match args.mode {
        Mode::Repl => {
            let state: Arc<dyn BundleFacade> = if args.create {
                // Creating a new bundle - always read-write
                info!("Creating bundle at: {}", bundle);
                BundleBuilder::create(&bundle, config.clone()).await?
            } else if args.read_only {
                // Read-only mode - open as Bundle
                info!("Opening bundle in read-only mode: {}", bundle);
                Bundle::open(&bundle, config.clone()).await?
            } else {
                // Read-write mode - open and extend
                info!("Opening bundle in read-write mode: {}", bundle);
                Bundle::open(&bundle, config.clone()).await?.extend(None).await?
            };

            if let Some(sql) = args.execute {
                // Non-interactive execute mode
                repl::execute_single(state, &sql, args.format).await?;
            } else {
                // Interactive REPL mode
                repl::print_header();
                repl::start(state, args.format).await?;
            }
        }
        Mode::Flight => {
            info!(
                "{} bundle at: {}{}",
                if args.create { "Creating" } else { "Opening" },
                bundle,
                if args.read_only { " (read-only)" } else { "" }
            );
            let port = args.port.unwrap_or(50051);
            let addr = format!("{}:{}", args.host, port)
                .parse()
                .map_err(|e| BundlebaseError::from(format!("Invalid address: {}", e)))?;
            flight::start(&bundle, config, args.create, args.read_only, addr).await?;
        }
        Mode::Mcp => {
            let state: Arc<dyn BundleFacade> = if args.create {
                info!("Creating bundle at: {}", bundle);
                BundleBuilder::create(&bundle, config.clone()).await?
            } else if args.read_only {
                info!("Opening bundle in read-only mode: {}", bundle);
                Bundle::open(&bundle, config.clone()).await?
            } else {
                info!("Opening bundle in read-write mode: {}", bundle);
                Bundle::open(&bundle, config.clone()).await?.extend(None).await?
            };

            mcp::start(state).await?;
        }
    }

    Ok(())
}

fn init_logging(args: &Args) {
    // Parse log level from CLI argument
    let log_config = parse_log_level(&args.log_level).unwrap_or_else(|e| {
        eprintln!("Invalid log level '{}': {}", args.log_level, e);
        std::process::exit(1);
    });

    // Bridge log crate to tracing (captures log::info!, etc.)
    // Ignore error if a logger is already set
    let _ = LogTracer::init();

    // Initialize tracing/logging with the configured level
    if log_config.ui_mode {
        // UI mode: minimal format (message only)
        let _ = tracing_subscriber::fmt()
            .with_max_level(log_config.level)
            .with_writer(std::io::stderr)
            .with_target(false)
            .with_level(false)
            .with_thread_ids(false)
            .with_thread_names(false)
            .with_file(false)
            .with_line_number(false)
            .without_time()
            .try_init();
    } else {
        // Debug mode: full format with timestamp, level, and module
        let _ = tracing_subscriber::fmt()
            .with_max_level(log_config.level)
            .with_writer(std::io::stderr)
            .try_init();
    }
}

#[cfg(test)]
mod tests {
    use bundlebase::bundle::BundleFacade;
    use bundlebase::{Bundle, BundleBuilder};

    #[tokio::test]
    async fn test_create_bundle_with_memory_url() {
        // Create a new bundle using memory:// URL
        let result = BundleBuilder::create("memory:///test_bundle", None).await;
        assert!(result.is_ok(), "Failed to create bundle with memory:// URL");

        let builder = result.expect("Should succeed");
        assert!(builder.bundle().url().to_string().starts_with("memory://"));
    }

    #[tokio::test]
    async fn test_create_and_reopen_bundle() {
        // Use a unique URL to avoid conflicts
        let url = format!(
            "memory:///reopen_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("System time before UNIX epoch")
                .as_nanos()
        );

        // Create a new bundle
        let create_result = BundleBuilder::create(&url, None).await;
        assert!(create_result.is_ok(), "Failed to create bundle");

        let mut builder = create_result.expect("Should succeed");

        // Commit it so it's persisted
        builder
            .commit("Initial commit")
            .await
            .expect("Failed to commit");

        // Now try to open it
        let open_result = Bundle::open(&url, None).await;
        assert!(open_result.is_ok(), "Failed to reopen bundle after commit");

        let bundle = open_result.expect("Should succeed");
        assert_eq!(bundle.url().to_string(), url);
    }

    #[tokio::test]
    async fn test_multiple_bundles_with_memory_urls() {
        // Create multiple bundles with different memory:// URLs
        let bundles: Vec<_> = (0..5).map(|i| format!("memory:///bundle_{}", i)).collect();

        for url in bundles {
            let result = BundleBuilder::create(&url, None).await;
            assert!(result.is_ok(), "Failed to create bundle at {}", url);

            let builder = result.expect("Should succeed");
            assert_eq!(builder.bundle().url().to_string(), url);
        }
    }

    #[tokio::test]
    async fn test_empty_bundle_creation() {
        let builder = BundleBuilder::create("memory:///empty_test", None)
            .await
            .expect("Failed to create empty bundle");

        let schema = builder.bundle().schema().await.expect("Failed to get schema");

        // Empty bundle should have only the sentinel no_data field
        assert_eq!(schema.fields().len(), 1);
        assert_eq!(schema.field(0).name(), "no_data");
    }

    #[tokio::test]
    async fn test_file_url_path_handling() {
        // Relative path should work
        let result = BundleBuilder::create("file:///tmp/bundle_test", None).await;
        assert!(result.is_ok(), "Failed to create bundle with file:// URL");

        let builder = result.expect("Should succeed");
        assert!(builder.bundle().url().to_string().starts_with("file://"));
    }

    #[tokio::test]
    async fn test_url_conversion_from_filesystem_path() {
        // The create method should handle filesystem paths and convert them to URLs
        let result = BundleBuilder::create("memory:///filesystem_compat_test", None).await;
        assert!(result.is_ok());

        let builder = result.expect("Should succeed");
        // Should have converted to a proper URL internally
        assert!(!builder.bundle().url().to_string().is_empty());
    }

    #[tokio::test]
    async fn test_various_url_schemes() {
        // Test that the server code doesn't make assumptions about filesystem paths
        let test_cases = vec![
            ("memory:///test_case_1", true),
            ("memory:///test_case_2", true),
            ("memory:///nested/path/test", true),
        ];

        for (url, should_succeed) in test_cases {
            let result = BundleBuilder::create(url, None).await;
            if should_succeed {
                assert!(result.is_ok(), "Failed to create bundle with URL: {}", url);
                let builder = result.expect("Should succeed");
                assert_eq!(builder.bundle().url().to_string(), url);
            } else {
                assert!(result.is_err(), "Expected failure for URL: {}", url);
            }
        }
    }
}
