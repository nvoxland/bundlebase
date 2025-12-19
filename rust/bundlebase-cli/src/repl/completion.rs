use crate::repl::commands;
use crate::state::State;
use bundlebase::bundle::BundleFacade;
use reedline::{Completer, Span, Suggestion};
use std::sync::Arc;

pub struct BundleCompleter {
    state: Arc<State>,
    commands: Vec<String>,
}

impl BundleCompleter {
    pub fn new(state: Arc<State>) -> Self {
        let commands = vec![
            "attach".to_string(),
            "show".to_string(),
            "filter".to_string(),
            "select".to_string(),
            "remove".to_string(),
            "rename".to_string(),
            "query".to_string(),
            "join".to_string(),
            "schema".to_string(),
            "count".to_string(),
            "explain".to_string(),
            "history".to_string(),
            "index".to_string(),
            "drop-index".to_string(),
            "reindex".to_string(),
            "commit".to_string(),
            "help".to_string(),
            "exit".to_string(),
            "quit".to_string(),
            "clear".to_string(),
        ];

        Self { state, commands }
    }

    /// Get column names from current schema
    async fn get_column_names(&self) -> Vec<String> {
        if let Ok(schema) = self.state.bundle.read().schema().await {
            return schema
                .fields()
                .iter()
                .map(|f| f.name().to_string())
                .collect();
        }
        vec![]
    }

    /// Check if we're completing a parameter value after '='
    fn is_completing_value(&self, line: &str, pos: usize) -> Option<(String, String, usize)> {
        let before_cursor = &line[..pos];

        // Find the last '=' before cursor
        if let Some(eq_pos) = before_cursor.rfind('=') {
            // Extract parameter name (word before '=')
            let before_eq = &before_cursor[..eq_pos];
            let param_start = before_eq
                .rfind(|c: char| c.is_whitespace())
                .map(|i| i + 1)
                .unwrap_or(0);
            let param_name = before_eq[param_start..].trim().to_string();

            // Extract partial value (after '=')
            let value_start = eq_pos + 1;
            let partial_value = before_cursor[value_start..].to_string();

            return Some((param_name, partial_value, value_start));
        }

        None
    }
}

impl Completer for BundleCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        let mut suggestions = Vec::new();

        // Safety check: ensure pos is within bounds
        if pos > line.len() {
            return suggestions;
        }

        let before_cursor = &line[..pos];
        let words: Vec<&str> = before_cursor.split_whitespace().collect();

        // Case 1: Check if we're completing a value after '='
        if let Some((param_name, partial_value, value_start)) = self.is_completing_value(line, pos)
        {
            let span = Span::new(value_start, pos);

            // Provide value suggestions based on parameter type
            match param_name.as_str() {
                "join_type" => {
                    for join_type in &["Inner", "Left", "Right", "Full"] {
                        if join_type
                            .to_lowercase()
                            .starts_with(&partial_value.to_lowercase())
                        {
                            suggestions.push(Suggestion {
                                value: join_type.to_string(),
                                description: None,
                                style: None,
                                extra: None,
                                span,
                                append_whitespace: false,
                            });
                        }
                    }
                }
                "column" | "old" | "new" | "columns" => {
                    // Suggest column names from schema
                    let columns = tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(self.get_column_names())
                    });
                    for col in columns {
                        if col
                            .to_lowercase()
                            .starts_with(&partial_value.to_lowercase())
                        {
                            suggestions.push(Suggestion {
                                value: col,
                                description: Some("column".to_string()),
                                style: None,
                                extra: None,
                                span,
                                append_whitespace: false,
                            });
                        }
                    }
                }
                _ => {}
            }

            return suggestions;
        }

        // Case 2: Complete command names
        if words.is_empty() || (words.len() == 1 && !before_cursor.ends_with(' ')) {
            let partial = words.first().unwrap_or(&"");
            let start = before_cursor.len() - partial.len();
            let span = Span::new(start, pos);

            for cmd in &self.commands {
                if cmd.starts_with(partial) {
                    suggestions.push(Suggestion {
                        value: cmd.clone(),
                        description: None,
                        style: None,
                        extra: None,
                        span,
                        append_whitespace: true,
                    });
                }
            }
            return suggestions;
        }

        // Case 3: Complete parameter names for known commands
        if !words.is_empty() {
            let command = words[0];
            let param_names = commands::get_parameter_names(command);

            if !param_names.is_empty() {
                // Check if we're in the middle of typing a parameter
                let last_word = words.last().unwrap_or(&"");

                // If last word doesn't contain '=', suggest parameter names
                if !last_word.contains('=') && !before_cursor.ends_with('=') {
                    let start = before_cursor.len() - last_word.len();
                    let span = Span::new(start, pos);

                    // Find which parameters have already been provided
                    let provided_params: Vec<String> = words[1..]
                        .iter()
                        .filter_map(|w| w.split('=').next().map(|s| s.to_string()))
                        .collect();

                    for param in param_names {
                        // Only suggest parameters not already provided
                        if !provided_params.contains(&param) && param.starts_with(last_word) {
                            suggestions.push(Suggestion {
                                value: format!("{}=", param),
                                description: Some(format!("parameter for {}", command)),
                                style: None,
                                extra: None,
                                span,
                                append_whitespace: false,
                            });
                        }
                    }
                }
            }
        }

        suggestions
    }
}
