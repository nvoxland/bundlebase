//! MCP (Model Context Protocol) server for bundlebase.
//!
//! This module provides an MCP server over stdio, allowing AI assistants
//! to query and interact with bundlebase bundles. The bundle is opened once
//! at startup and kept alive, preserving cache and state between calls.

mod server;
mod tools;

pub use server::start;
