//! Interactive REPL (Read-Eval-Print-Loop) for bundlebase.
//!
//! This module provides an interactive command-line interface for working with
//! bundlebase bundles. It supports SQL commands and REPL-specific meta commands.

mod commands;
mod completion;
pub mod display;
mod progress_impl;
pub mod stream_formatter;
pub mod table_utils;

use bundlebase::{BundlebaseError, BundleFacade};
use commands::{Command, ReplCommand};
use completion::BundleCompleter;
use reedline::{
    default_emacs_keybindings, DefaultPrompt, DefaultPromptSegment, Emacs, FileBackedHistory,
    Reedline, Signal,
};
use std::sync::Arc;
use stream_formatter::format_stream;
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
pub async fn start(bundle: Arc<dyn BundleFacade>) -> Result<(), BundlebaseError> {
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
    let completer = Box::new(BundleCompleter::new(bundle.clone()));

    // Create reedline editor
    let mut line_editor = Reedline::create()
        .with_history(history)
        .with_completer(completer)
        .with_edit_mode(Box::new(Emacs::new(default_emacs_keybindings())));

    let prompt = DefaultPrompt {
        left_prompt: DefaultPromptSegment::Basic(bundle.url().to_string()),
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
                if matches!(cmd, Command::Repl(ReplCommand::Exit)) {
                    info!("Goodbye!");
                    break;
                }

                // Execute command
                match commands::execute(cmd, &bundle).await {
                    Ok(Some((stream, shape))) => {
                        // Use format_stream for consistent output formatting
                        match format_stream(stream, Some(shape), Some(100)).await {
                            Ok(output) => {
                                if !output.is_empty() {
                                    println!("{}", output);
                                }
                            }
                            Err(e) => {
                                error!("Error formatting output: {}", e);
                            }
                        }
                    }
                    Ok(None) => {
                        // No output (Clear command)
                    }
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
