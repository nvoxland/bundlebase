//! Interactive REPL (Read-Eval-Print-Loop) for bundlebase.
//!
//! This module provides an interactive command-line interface for working with
//! bundlebase bundles. It supports SQL commands and REPL-specific meta commands.

mod commands;
mod completion;
pub mod display;
mod progress_impl;

use crate::state::BundleState;
use bundlebase::BundlebaseError;
use commands::{Command, ExecuteResult};
use completion::BundleCompleter;
use reedline::{
    default_emacs_keybindings, DefaultPrompt, DefaultPromptSegment, Emacs, FileBackedHistory,
    Reedline, Signal,
};
use std::sync::Arc;
use tracing::{error, info};

/// Print the REPL header.
pub fn print_header() {
    info!("Bundlebase REPL");
    info!("Type '/help' for available commands, '/exit' to quit");
    info!("----------------------------------------------------------");
}

/// Start the interactive REPL.
///
/// This is the main entry point for the REPL mode. It sets up the readline
/// interface with history and completion, then enters the read-eval-print loop.
///
/// # Arguments
///
/// * `state` - The shared bundle state to work with
///
/// # Returns
///
/// * `Ok(())` - REPL exited normally
/// * `Err(BundlebaseError)` - An error occurred
pub async fn start(state: Arc<BundleState>) -> Result<(), BundlebaseError> {
    // Install progress tracker for REPL
    let tracker = Box::new(progress_impl::IndicatifTracker::new());
    bundlebase::progress::set_tracker(tracker);

    // Setup history in ~/.bundlebase/history.txt
    let history = Box::new({
        let history_path = dirs::home_dir()
            .map(|home| home.join(".bundlebase").join("history.txt"))
            .unwrap_or_else(|| "repl-history.txt".into());

        // Create parent directory if it doesn't exist
        if let Some(parent) = history_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        FileBackedHistory::with_file(1000, history_path)
            .unwrap_or_else(|_| FileBackedHistory::default())
    });

    // Setup completer
    let completer = Box::new(BundleCompleter::new(state.clone()));

    // Create reedline editor
    let mut line_editor = Reedline::create()
        .with_history(history)
        .with_completer(completer)
        .with_edit_mode(Box::new(Emacs::new(default_emacs_keybindings())));

    let prompt = DefaultPrompt {
        left_prompt: DefaultPromptSegment::Basic(state.url()),
        right_prompt: DefaultPromptSegment::CurrentDateTime,
    };

    loop {
        // Read line in current thread (reedline is sync but works fine in async context)
        let sig = line_editor.read_line(&prompt)?;

        match sig {
            Signal::Success(input) => {
                let input = input.trim();
                if input.is_empty() {
                    continue;
                }

                // Parse command
                let cmd = match commands::parse(input) {
                    Ok(cmd) => cmd,
                    Err(e) => {
                        error!("Error parsing command: {}", e);
                        continue;
                    }
                };

                // Check for exit command
                if matches!(cmd, Command::Exit) {
                    info!("Goodbye!");
                    break;
                }

                // Execute command
                match commands::execute(cmd, &state).await {
                    Ok(result) => match result {
                        ExecuteResult::Message(msg) => println!("{}", msg),
                        ExecuteResult::Table(table) => println!("{}", table),
                        ExecuteResult::None => {}
                    },
                    Err(e) => {
                        error!("Error executing command: {}", e);
                    }
                }
            }
            Signal::CtrlC | Signal::CtrlD => {
                info!("Goodbye!");
                break;
            }
        }
    }

    Ok(())
}

/// Run the interactive REPL.
///
/// This is an alias for `start()` for backwards compatibility.
#[deprecated(since = "0.4.0", note = "Use start() instead")]
pub async fn run(state: Arc<BundleState>) -> Result<(), BundlebaseError> {
    start(state).await
}
