//! Bundlebase CLI library.
//!
//! This crate provides the CLI components for bundlebase, including:
//! - Interactive REPL mode
//! - Arrow Flight server
//! - SQL execution utilities
//!
//! The library can be used to embed bundlebase CLI functionality in other applications
//! or for testing purposes.

pub mod agent_skills;
pub mod auth;
pub mod flight;
pub mod mcp;
pub mod repl;

use clap::ValueEnum;

/// Output format for query results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// Human-readable table format
    Table,
    /// Machine-readable JSON format
    Json,
}

impl std::fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputFormat::Table => write!(f, "table"),
            OutputFormat::Json => write!(f, "json"),
        }
    }
}
