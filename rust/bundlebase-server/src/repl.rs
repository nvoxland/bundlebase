mod commands;
mod completion;
mod display;
mod progress_impl;

use crate::state::State;
use commands::{Command, ExecuteResult};
use completion::BundleCompleter;
use bundlebase::bundle::BundleFacade;
use bundlebase::BundlebaseError;
use reedline::{default_emacs_keybindings, DefaultPrompt, DefaultPromptSegment, Emacs, FileBackedHistory, Reedline, Signal};
use std::sync::Arc;
use tracing::{error, info};

pub fn print_header() {
    info!("Bundlebase REPL");
    info!("Type 'help' for available commands, 'exit' to quit");
    info!("----------------------------------------------------------");
}

pub async fn run(state: Arc<State>) -> Result<(), BundlebaseError> {
    // Install progress tracker for REPL
    let tracker = Box::new(progress_impl::IndicatifTracker::new());
    bundlebase::progress::set_tracker(tracker);

    // Setup history
    let history = Box::new(
        FileBackedHistory::with_file(1000, "repl-history.txt".into())
            .unwrap_or_else(|_| FileBackedHistory::default()),
    );

    // Setup completer
    let completer = Box::new(BundleCompleter::new(state.clone()));

    // Create reedline editor
    let mut line_editor = Reedline::create()
        .with_history(history)
        .with_completer(completer)
        .with_edit_mode(Box::new(Emacs::new(default_emacs_keybindings())));

    let prompt = DefaultPrompt {
        left_prompt: DefaultPromptSegment::Basic(state.bundle.read().url().to_string()),
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
                        ExecuteResult::List(items) => {
                            items.iter().for_each(|item| println!("- {}", item))
                        }
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
