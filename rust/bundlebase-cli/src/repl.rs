//! Interactive REPL (Read-Eval-Print-Loop) for bundlebase.
//!
//! This module provides an interactive command-line interface for working with
//! bundlebase bundles. It supports SQL commands and REPL-specific meta commands.

pub(crate) mod commands;
mod completion;
pub mod display;
pub mod json_formatter;
mod progress_impl;
pub mod stream_formatter;
pub mod table_utils;

use crate::OutputFormat;
use bundlebase::BundleFacade;
use bundlebase_common::BundlebaseError;
use commands::{Command, ReplCommand};
use completion::BundleCompleter;
use reedline::{
    default_emacs_keybindings, DefaultPrompt, DefaultPromptSegment, Emacs, FileBackedHistory,
    Reedline, Signal,
};
use std::sync::Arc;
use stream_formatter::format_stream;
use json_formatter::format_stream_json;
use tracing::{error, info};

/// Print the REPL header with bundle info.
pub fn print_header(bundle: &dyn BundleFacade) {
    let url = bundle.url();
    let version = bundle.version();
    let commit_count = bundle.history().len();

    if commit_count == 0 {
        info!("Opened new bundle at {}", url);
    } else {
        info!(
            "Opened bundle at {} (version {}, {} commit{})",
            url,
            version,
            commit_count,
            if commit_count == 1 { "" } else { "s" }
        );
    }
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
/// Execute one or more semicolon-separated commands non-interactively and exit.
pub async fn execute_single(
    bundle: Arc<dyn BundleFacade>,
    sql: &str,
    format: OutputFormat,
) -> Result<(), BundlebaseError> {
    // Install progress tracker
    let tracker = Box::new(progress_impl::IndicatifTracker::new());
    bundlebase_common::progress::set_tracker(tracker);

    // Parse all commands (validates all before executing any)
    let cmds = match commands::parse(sql) {
        Ok(cmds) => cmds,
        Err(e) => {
            let error_msg = format!("Error: {}", e);
            match format {
                OutputFormat::Json => {
                    eprintln!("{}", serde_json::json!({"error": error_msg}));
                }
                OutputFormat::Table => {
                    eprintln!("{}", error_msg);
                }
            }
            std::process::exit(1);
        }
    };

    // Execute all commands sequentially
    for cmd in cmds {
        // Handle exit/quit commands as no-ops
        if matches!(cmd, Command::Repl(ReplCommand::Exit)) {
            return Ok(());
        }

        match commands::execute(cmd, &bundle).await {
            Ok(Some((stream, shape))) => {
                let output = match format {
                    OutputFormat::Json => format_stream_json(stream, Some(shape), Some(1000)).await?,
                    OutputFormat::Table => format_stream(stream, Some(shape), Some(1000)).await?,
                };
                if !output.is_empty() {
                    println!("{}", output);
                }
            }
            Ok(None) => {
                // No output (Clear command, etc.)
            }
            Err(e) => {
                let error_msg = format!("{}", e);
                match format {
                    OutputFormat::Json => {
                        eprintln!("{}", serde_json::json!({"error": error_msg}));
                    }
                    OutputFormat::Table => {
                        eprintln!("Error: {}", error_msg);
                    }
                }
                std::process::exit(1);
            }
        }
    }

    Ok(())
}

pub async fn start(bundle: Arc<dyn BundleFacade>, format: OutputFormat) -> Result<(), BundlebaseError> {
    // Install progress tracker for REPL
    let tracker = Box::new(progress_impl::IndicatifTracker::new());
    bundlebase_common::progress::set_tracker(tracker);

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

                // Parse all statements (validates all before executing any)
                let cmds = match commands::parse(input) {
                    Ok(cmds) => cmds,
                    Err(e) => {
                        error!("Error parsing command: {}", e);
                        continue;
                    }
                };

                // Check for exit command
                if cmds.iter().any(|cmd| matches!(cmd, Command::Repl(ReplCommand::Exit))) {
                    info!("Goodbye!");
                    break;
                }

                // Execute all commands sequentially
                let mut had_error = false;
                for cmd in cmds {
                    match commands::execute(cmd, &bundle).await {
                        Ok(Some((stream, shape))) => {
                            let result = match format {
                                OutputFormat::Json => format_stream_json(stream, Some(shape), Some(1000)).await,
                                OutputFormat::Table => format_stream(stream, Some(shape), Some(1000)).await,
                            };
                            match result {
                                Ok(output) => {
                                    if !output.is_empty() {
                                        println!("{}", output);
                                    }
                                }
                                Err(e) => {
                                    error!("Error formatting output: {}", e);
                                    had_error = true;
                                    break;
                                }
                            }
                        }
                        Ok(None) => {
                            // No output (Clear command)
                        }
                        Err(e) => {
                            error!("Error executing command: {}", e);
                            had_error = true;
                            break;
                        }
                    }
                }
                if had_error {
                    continue;
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
