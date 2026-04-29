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
use json_formatter::format_stream_json;
use bundlebase_command::parser::is_input_complete;
use reedline::{
    default_emacs_keybindings, DefaultPrompt, DefaultPromptSegment, Emacs, FileBackedHistory,
    Reedline, Signal, ValidationResult, Validator,
};

/// Reedline validator: a SQL/command buffer is "complete" only when every
/// statement ends in `;` (outside of any quoted string). Slash commands and
/// `:` shortcuts are single-line, so they're always complete. An empty buffer
/// is also complete — pressing Enter on a blank prompt should re-display it.
struct SqlStatementValidator;

impl Validator for SqlStatementValidator {
    fn validate(&self, line: &str) -> ValidationResult {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('/') {
            return ValidationResult::Complete;
        }
        if is_input_complete(line) {
            ValidationResult::Complete
        } else {
            ValidationResult::Incomplete
        }
    }
}
use std::sync::Arc;
use stream_formatter::format_stream;
use tracing::{error, info};

/// Render a query duration in a single human-readable form per magnitude:
/// `< 1 ms`, `42 ms`, `1.23 s`, `1m 23.4s`. Tuned for end-of-result lines —
/// no fractional ms (sub-ms is just "<1 ms"), no padding.
fn format_elapsed(d: std::time::Duration) -> String {
    let total_ms = d.as_millis();
    if total_ms < 1 {
        return "<1 ms".to_string();
    }
    if total_ms < 1_000 {
        return format!("{} ms", total_ms);
    }
    let secs = d.as_secs_f64();
    if secs < 60.0 {
        return format!("{:.2} s", secs);
    }
    let mins = (secs / 60.0).floor() as u64;
    let rem = secs - (mins as f64) * 60.0;
    format!("{}m {:.1}s", mins, rem)
}

fn prompt_label(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    trimmed
        .rsplit('/')
        .next()
        .filter(|segment| !segment.is_empty() && !segment.ends_with(':'))
        .unwrap_or(url)
        .to_string()
}

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
                    OutputFormat::Json => {
                        format_stream_json(stream, Some(shape), Some(1000)).await?
                    }
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

pub async fn start(
    bundle: Arc<dyn BundleFacade>,
    format: OutputFormat,
) -> Result<(), BundlebaseError> {
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
        .with_validator(Box::new(SqlStatementValidator))
        .with_edit_mode(Box::new(Emacs::new(default_emacs_keybindings())));

    let prompt = DefaultPrompt {
        left_prompt: DefaultPromptSegment::Basic(prompt_label(&bundle.url().to_string())),
        right_prompt: DefaultPromptSegment::CurrentDateTime,
    };

    // Tracks whether the previous loop iteration ended with a Ctrl-C at the
    // edit prompt (buffer cleared by reedline). A second consecutive Ctrl-C
    // exits; any other input — Enter, Ctrl-C during a query, etc. — disarms it.
    let mut exit_armed = false;

    loop {
        // Read line in current thread (reedline is sync but works fine in async context)
        let sig = line_editor.read_line(&prompt)?;

        match sig {
            Signal::Success(input) => {
                exit_armed = false;
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
                if cmds
                    .iter()
                    .any(|cmd| matches!(cmd, Command::Repl(ReplCommand::Exit)))
                {
                    info!("Goodbye!");
                    break;
                }

                // Execute all commands sequentially. Each command is racing
                // against `tokio::signal::ctrl_c()` so the user can interrupt
                // a long-running query without killing the REPL.
                let mut had_error = false;
                let mut interrupted = false;
                for cmd in cmds {
                    let started = std::time::Instant::now();
                    let exec = async {
                        match commands::execute(cmd, &bundle).await {
                            Ok(Some((stream, shape))) => {
                                let result = match format {
                                    OutputFormat::Json => {
                                        format_stream_json(stream, Some(shape), Some(1000)).await
                                    }
                                    OutputFormat::Table => {
                                        format_stream(stream, Some(shape), Some(1000)).await
                                    }
                                };
                                match result {
                                    Ok(output) => {
                                        if !output.is_empty() {
                                            println!("{}", output);
                                        }
                                        // `true` = had output worth timing; we
                                        // skip the timing line for commands
                                        // that print nothing (e.g. /clear) so
                                        // the screen doesn't fill with noise.
                                        Ok(!output.is_empty())
                                    }
                                    Err(e) => Err(format!("Error formatting output: {}", e)),
                                }
                            }
                            Ok(None) => Ok(false), // No output (Clear command)
                            Err(e) => Err(format!("Error executing command: {}", e)),
                        }
                    };

                    tokio::select! {
                        // Ensure the command future is polled first so that on
                        // immediate completion we don't gratuitously consume a
                        // pending signal.
                        biased;
                        res = exec => {
                            match res {
                                Ok(true) => println!("({})", format_elapsed(started.elapsed())),
                                Ok(false) => {} // Silent commands (clear, etc.)
                                Err(msg) => {
                                    error!("{}", msg);
                                    had_error = true;
                                    break;
                                }
                            }
                        }
                        _ = tokio::signal::ctrl_c() => {
                            // Print immediately so the user sees the cancel
                            // landed even if dropping the future takes a
                            // moment (some streams need to unwind I/O).
                            // Then drop the in-flight future and bail out of
                            // the per-statement loop. The outer loop will
                            // reprompt on the next iteration.
                            println!("<Cancelling Query...>");
                            interrupted = true;
                            break;
                        }
                    }
                }
                if had_error || interrupted {
                    continue;
                }
            }
            Signal::CtrlC => {
                // Reedline already cleared the buffer. First press warns;
                // a second consecutive press exits.
                if exit_armed {
                    info!("Goodbye!");
                    break;
                }
                exit_armed = true;
                info!("Press Ctrl-C again to exit, or /exit");
            }
            Signal::CtrlD => {
                info!("Goodbye!");
                break;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::prompt_label;

    #[test]
    fn prompt_label_uses_last_path_segment() {
        assert_eq!(
            prompt_label("file:///tmp/claude-history-bundle"),
            "claude-history-bundle"
        );
        assert_eq!(prompt_label("s3://bucket/path/to/bundle"), "bundle");
    }

    #[test]
    fn prompt_label_falls_back_when_url_has_no_path_segment() {
        assert_eq!(prompt_label("memory://"), "memory://");
        assert_eq!(prompt_label("file:///"), "file:///");
    }
}
