//! MCP (Model Context Protocol) server for bundlebase.
//!
//! This module provides an MCP server over stdio, allowing AI assistants
//! to create, open, query, and interact with bundlebase bundles. Can start
//! with or without a bundle — use the `open_bundle` or `create_bundle` tools
//! to load one during the session.

mod server;
mod tools;

pub use server::start;
