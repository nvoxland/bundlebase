//! Bundlebase CLI - command-line interface for bundlebase.
//!
//! Provides subcommands for interactive REPL, single query execution,
//! Arrow Flight server, and MCP server modes.

mod cmd;

use bundlebase_common::BundlebaseError;
use clap::{Parser, Subcommand as ClapSubcommand};
use tracing_log::LogTracer;

#[derive(Parser, Debug)]
#[command(name = "bundlebase")]
#[command(about = "Bundlebase", long_about = None)]
struct Cli {
    /// Logging level (ui, trace, debug, info, warn, error)
    /// ui: Minimal format (message only), INFO level - good for interactive use
    #[arg(long, default_value = "ui", global = true)]
    log_level: String,

    /// OpenTelemetry endpoint for tracing (e.g., "http://localhost:4317")
    #[arg(long, global = true)]
    otel: Option<String>,

    #[command(subcommand)]
    subcommand: Subcommand,
}

#[derive(ClapSubcommand, Debug)]
enum Subcommand {
    /// Interactive REPL mode
    Repl(cmd::repl_cmd::ReplArgs),

    /// Create a new bundle, optionally executing initial commands
    Create(cmd::create_cmd::CreateArgs),

    /// Execute a read-only SQL query and exit
    Query(cmd::query_cmd::QueryArgs),

    /// Execute a mutating statement against an existing bundle and exit
    Extend(cmd::extend_cmd::ExtendArgs),

    /// Execute a mutating statement against a bundle and exit (alias for extend)
    #[command(hide = true)]
    Execute(cmd::extend_cmd::ExtendArgs),

    /// List all bundles found in a directory
    ListBundles(cmd::list_bundles_cmd::ListBundlesArgs),

    /// Start MCP (Model Context Protocol) server over stdio
    Mcp(cmd::mcp_cmd::McpArgs),

    /// Start Arrow Flight SQL server
    Server(cmd::server_cmd::ServerArgs),

    /// Install agent skills for coding agents
    SetupAgent(cmd::setup_agent_cmd::SetupAgentArgs),
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

#[tokio::main]
async fn main() -> Result<(), BundlebaseError> {
    let cli = Cli::parse();

    // Install catalog schema providers so Bundle/BundleBuilder creation
    // automatically registers blocks, packs, default, and bundle_info schemas.
    bundlebase_catalog::init();

    init_logging(&cli);

    match cli.subcommand {
        Subcommand::Repl(args) => cmd::repl_cmd::run(args).await?,
        Subcommand::Create(args) => cmd::create_cmd::run(args).await?,
        Subcommand::Query(args) => cmd::query_cmd::run(args).await?,
        Subcommand::Extend(args) | Subcommand::Execute(args) => cmd::extend_cmd::run(args).await?,
        Subcommand::ListBundles(args) => cmd::list_bundles_cmd::run(args).await?,
        Subcommand::Mcp(args) => cmd::mcp_cmd::run(args).await?,
        Subcommand::Server(args) => cmd::server_cmd::run(args).await?,
        Subcommand::SetupAgent(args) => cmd::setup_agent_cmd::run(args)?,
    }

    Ok(())
}

fn init_logging(cli: &Cli) {
    // Parse log level from CLI argument
    let log_config = parse_log_level(&cli.log_level).unwrap_or_else(|e| {
        eprintln!("Invalid log level '{}': {}", cli.log_level, e);
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

        let builder = create_result.expect("Should succeed");

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
