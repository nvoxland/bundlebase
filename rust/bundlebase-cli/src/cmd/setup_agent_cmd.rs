//! The `setup-agent` subcommand — install agent skills for coding agents.

use bundlebase::BundlebaseError;
use clap::Args;

/// Install agent skills for coding agents (Claude Code, Cursor, Copilot, etc.)
#[derive(Args, Debug)]
pub struct SetupAgentArgs {
    /// Scope: local (project-level) or global (user-level ~/.agents/skills/)
    #[arg(long, default_value = "local")]
    pub scope: String,
}

pub fn run(args: SetupAgentArgs) -> Result<(), BundlebaseError> {
    let global = match args.scope.as_str() {
        "local" => false,
        "global" => true,
        other => {
            eprintln!(
                "Error: Invalid --scope value '{}'. Use 'local' or 'global'.",
                other
            );
            std::process::exit(1);
        }
    };
    bundlebase_cli::agent_skills::install(global)
}
