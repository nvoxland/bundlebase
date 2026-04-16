//! The `setup-agent` subcommand — install agent skills for coding agents.

use bundlebase_cli::agent_skills::AgentTarget;
use bundlebase_common::BundlebaseError;
use clap::{Args, ValueEnum};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SetupAgentScope {
    Local,
    Global,
}

/// Install local agent integration for Claude Code and GitHub Copilot.
#[derive(Args, Debug)]
pub struct SetupAgentArgs {
    /// Scope: local (project-level) or global (Claude-only user-level files)
    #[arg(long, value_enum, default_value_t = SetupAgentScope::Local)]
    pub scope: SetupAgentScope,

    /// Target agent: auto-detect from PATH by default, or explicitly choose claude or copilot
    #[arg(long, value_enum)]
    pub agent: Option<AgentTarget>,
}

pub fn run(args: SetupAgentArgs) -> Result<(), BundlebaseError> {
    let global = matches!(args.scope, SetupAgentScope::Global);
    bundlebase_cli::agent_skills::install(global, args.agent)
}
