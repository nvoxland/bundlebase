//! Bundlebase CLI library.
//!
//! This crate provides the CLI components for bundlebase, including:
//! - Interactive REPL mode
//! - Arrow Flight server
//! - SQL execution utilities
//!
//! The library can be used to embed bundlebase CLI functionality in other applications
//! or for testing purposes.

pub mod auth;
pub mod flight;
pub mod repl;
