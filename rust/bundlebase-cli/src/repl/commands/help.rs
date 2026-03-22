//! Help command - displays available commands and usage.

use super::{ReplCommandResult, ReplCommand, ReplCommandDef};
use bundlebase::BundleFacade;
use futures::future::BoxFuture;
use std::sync::Arc;

/// Command metadata
pub const DEF: ReplCommandDef = ReplCommandDef {
    name: "help",
    aliases: &[],
    description: "Show help",
    usage: "/help",
    create,
    execute,
};

fn create(_args: &str) -> Result<ReplCommand, String> {
    Ok(ReplCommand::Help)
}

fn execute(_cmd: &ReplCommand, _bundle: &Arc<dyn BundleFacade>) -> BoxFuture<'static, ReplCommandResult> {
    Box::pin(async {
        let response: Box<dyn bundlebase::bundle::CommandResponse> = Box::new(help_text());
        let (stream, shape) = super::response_to_stream(response)?;
        Ok(Some((stream, shape)))
    })
}

fn help_text() -> String {
    let mut text = String::from(
        "Bundlebase REPL\n\
         \n\
         Enter any bundlebase SQL statement or command directly.\n\
         Type a query like SELECT ... FROM bundle, or use a command like ATTACH, FILTER, COMMIT, etc.\n\
         \n\
         For metadata, use SHOW (e.g., SHOW HISTORY, SHOW COLUMNS). Type SHOW COMMANDS for the full list.\n\
         \n\
         Available commands:",
    );

    // Find the longest usage string for column alignment
    let max_usage_len = ReplCommand::all_commands()
        .map(|def| {
            let alias_suffix = if def.aliases.is_empty() {
                0
            } else {
                // " (/alias1, /alias2)"
                3 + def.aliases.iter().map(|a| a.len() + 1).sum::<usize>()
                    + (def.aliases.len() - 1) * 2
            };
            def.usage.len() + alias_suffix
        })
        .max()
        .unwrap_or(0);

    for def in ReplCommand::all_commands() {
        let usage_with_aliases = if def.aliases.is_empty() {
            def.usage.to_string()
        } else {
            let aliases: Vec<String> = def.aliases.iter().map(|a| format!("/{}", a)).collect();
            format!("{} ({})", def.usage, aliases.join(", "))
        };

        text.push_str(&format!(
            "\n  {:<width$}   {}",
            usage_with_aliases,
            def.description,
            width = max_usage_len,
        ));
    }

    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use bundlebase::bundle::CommandResponse;
    use futures::StreamExt;

    #[tokio::test]
    async fn test_help_result() {
        let result = help_text();
        let response: Box<dyn CommandResponse> = Box::new(result);
        let mut stream = response.into_stream().unwrap();
        let batch = stream.next().await.unwrap().unwrap();
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 1);
    }
}
