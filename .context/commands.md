# Adding New Commands

This guide documents how to add new commands to the bundlebase command system.

## Overview

Commands encapsulate operation logic and can be executed via SQL parsing or direct API calls. The command system uses:
- **Pest grammar** for bundlebase-specific syntax (FILTER, ATTACH, JOIN, etc.)
- **sqlparser-rs** for standard SQL (rarely used, most commands use Pest)

## Checklist

When adding a new command, update these files:

1. [ ] `rust/bundlebase/src/bundle/command/builder/<name>.rs` or `facade/<name>.rs` - Create command struct
2. [ ] `rust/bundlebase/src/bundle/command/builder.rs` or `facade.rs` - Add `mod <name>` and `pub use`
3. [ ] `rust/bundlebase/src/bundle/command.rs` - Add to `register_commands!` macro with syntax string
4. [ ] `rust/bundlebase/src/bundle/command.rs` - Add re-export
5. [ ] `rust/bundlebase/src/bundle/command/parser/grammar.pest` - Add `<name>_stmt` rule
6. [ ] `rust/bundlebase/src/bundle/command/parser/grammar.pest` - Add to appropriate category
7. [ ] `rust/bundlebase/src/bundle/command/syntax/<name>.md` - Add usage docs (description + examples)
8. [ ] For facade commands: update `FacadeCommand` enum, `into_facade_command()`, and `is_facade_command()`

The `register_commands!` macro auto-generates `available_commands()` from the syntax strings — no separate update needed in `pest_parser.rs`.

## Grammar Categories

The `statement` rule in `grammar.pest` is organized into semantic categories:

| Category | Description | Example Commands |
|----------|-------------|------------------|
| `data_modification_stmt` | Operations that change bundle data content | FILTER, SELECT, ATTACH, DETACH, REPLACE |
| `schema_stmt` | Operations that change bundle structure | JOIN, DROP JOIN, RENAME JOIN, columns, views |
| `source_stmt` | Operations for data sources and functions | CREATE SOURCE, FETCH, IMPORT/DROP/RENAME CONNECTOR, IMPORT/DROP/RENAME FUNCTION (including TEMP variants) |
| `index_stmt` | Operations for search indexes | CREATE INDEX, DROP INDEX, REBUILD INDEX, REINDEX |
| `transaction_stmt` | Operations for version control | COMMIT, RESET, UNDO, VERIFY DATA |
| `metadata_stmt` | Operations for bundle metadata | SET NAME, SET DESCRIPTION, SET CONFIG, SAVE CONFIG, SHOW, SYNTAX, DESCRIBE, EXPORT |

Add new statement rules to the appropriate category for maintainability.

## Execution Path

All commands are executed through a single unified method:

```
execute_command(cmd)
├─ Always applies change tracking (via do_change())
├─ Returns C::Output (any type the command defines)
└─ Examples:
    - FilterCommand → ()
    - FetchCommand → Vec<FetchResults>
    - VerifyDataCommand → VerificationResults
```

### Execution Path Summary

| Path | Output Type | Change Tracking | Use When |
|------|-------------|-----------------|----------|
| `execute_command()` | `C::Output` (any) | Yes (wrapped in `do_change()`) | All commands |

## Command Template

```rust
//! <Name> command implementation.

use crate::bundle::command::{Command, CommandContext, Rule};
use crate::BundlebaseError;
use async_trait::async_trait;

/// Command to <describe what it does>.
#[derive(Debug, Clone)]
pub struct <Name>Command {
    // fields
}

impl <Name>Command {
    /// Create a new <Name>Command.
    pub fn new(/* args */) -> Self {
        Self { /* fields */ }
    }
}

#[async_trait]
impl Command for <Name>Command {
    type Output = (); // or specific type like Vec<FetchResults>

    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<Self::Output, BundlebaseError> {
        // Implementation
        // Use ctx.apply_operation() for operations
        // Use ctx.bundle() for read access
        // Use ctx.bundle_mut() for write access
        Ok(())
    }

    fn rule() -> Option<Rule> {
        Some(Rule::<name>_stmt) // or None if not SQL-parseable
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        // Parse from pest pair
        // Extract fields from pair.into_inner()
        todo!()
    }

    fn to_statement(&self) -> String {
        format!("<SQL SYNTAX>")
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_<name>() {
        let input = "<SQL INPUT>";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::<Name>(c) => {
                // assertions about parsed fields
            }
            _ => panic!("Expected <Name> variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = <Name>Command::new(/* args */);
        let statement = cmd.to_statement();
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::<Name>(c) => {
                // verify fields match
            }
            _ => panic!("Expected <Name> variant"),
        }
    }
}
```

## Grammar Rule Template

Add to `rust/bundlebase/src/bundle/command/parser/grammar.pest`:

```pest
// ============================================================================
// <NAME> Statement
// ============================================================================
// Syntax: <SYNTAX DESCRIPTION>
// Examples:
//   <EXAMPLE 1>
//   <EXAMPLE 2>

<name>_stmt = {
    ^"<keyword>" ~ <other_rules>
}
```

Key grammar patterns:
- `^"keyword"` - Case-insensitive keyword
- `identifier` - SQL identifier (alphanumeric + underscore)
- `quoted_string` - Single or double quoted string
- `(^"optional")?` - Optional clause
- `(!EOI ~ ANY)+` - Capture everything to end (for raw SQL)

## BundleCommand Enum Integration

In `command.rs`, add:

```rust
// In the BundleCommand enum
pub enum BundleCommand {
    // ... existing variants
    <Name>(<Name>Command),
}

// In BundleCommand::execute()
impl BundleCommand {
    pub async fn execute(self, builder: &mut BundleBuilder) -> Result<CommandOutput, BundlebaseError> {
        match self {
            // ... existing matches
            BundleCommand::<Name>(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(CommandOutput::Empty)
            }
            // For commands returning values:
            BundleCommand::<Name>(cmd) => {
                let results = builder.execute_command(cmd).await?;
                Ok(CommandOutput::<Variant>(results))
            }
        }
    }
}
```

## Parser Integration

In `parser.rs::try_parse_pest()`, add a match arm:

```rust
Rule::<name>_stmt => BundleCommand::<Name>(<Name>Command::from_statement(inner_stmt)?),
```

If the command uses a keyword that might look like custom syntax, update `pest_parser.rs::is_likely_custom_syntax()`:

```rust
|| upper.starts_with("<KEYWORD>")
```

## Testing Requirements

Every command should have:

1. **Parsing tests** - Verify SQL → Command parsing
2. **Round-trip tests** - Verify Command → SQL → Command
3. **E2E tests** (in Python) - Test actual functionality

See [testing.md](testing.md) for full testing guidelines.

## Example: Complete Filter Command

Here's the complete implementation of `FilterCommand` as a reference:

### File: `rust/bundlebase/src/bundle/command/filter.rs`

```rust
//! Filter command implementation.

use crate::bundle::command::{Command, CommandContext, Rule};
use crate::bundle::operation::FilterOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use datafusion::scalar::ScalarValue;
use log::info;

/// Command to filter rows with a SELECT query.
#[derive(Debug, Clone)]
pub struct FilterCommand {
    /// The SELECT query
    pub query: String,
    /// Parameters for the query ($1, $2, etc.)
    pub params: Vec<ScalarValue>,
}

impl FilterCommand {
    /// Create a new FilterCommand.
    pub fn new(query: impl Into<String>, params: Vec<ScalarValue>) -> Self {
        Self {
            query: query.into(),
            params,
        }
    }
}

#[async_trait]
impl Command for FilterCommand {
    type Output = ();

    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        let statement = self.to_statement();
        ctx.apply_operation(
            FilterOp::setup(&self.query, self.params)
                .await?
                .into(),
        )
        .await?;
        info!("Filtered: {}", statement);
        Ok(())
    }

    fn rule() -> Option<Rule> {
        Some(Rule::filter_stmt)
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut query = None;

        for inner_pair in pair.into_inner() {
            if let Rule::filter_query = inner_pair.as_rule() {
                query = Some(inner_pair.as_str().trim().to_string());
            }
        }

        let query = query.ok_or_else(|| -> BundlebaseError {
            "FILTER statement missing query".into()
        })?;

        if query.is_empty() {
            return Err("FILTER query cannot be empty".into());
        }

        Ok(FilterCommand::new(query, vec![]))
    }

    fn to_statement(&self) -> String {
        format!("FILTER WITH {}", self.query)
    }
}
```

### Grammar rule in `grammar.pest`:

```pest
// ============================================================================
// FILTER Statement
// ============================================================================
// Syntax: FILTER WITH <query>
// Example: FILTER WITH SELECT * FROM bundle WHERE country = 'USA' AND age > 21

filter_stmt = {
    ^"filter" ~ ^"with" ~ filter_query
}

filter_query = @{
    // Capture everything from WITH to end as raw text
    // This allows DataFusion to parse the query later
    (!EOI ~ ANY)+
}
```

### Integration in `command.rs`:

```rust
// Module and re-export
mod filter;
pub use filter::FilterCommand;

// Enum variant
pub enum BundleCommand {
    Filter(FilterCommand),
    // ...
}

// Execute match arm
BundleCommand::Filter(cmd) => {
    builder.execute_command(cmd).await?;
    Ok(CommandOutput::Empty)
}
```

### Integration in `parser.rs`:

```rust
Rule::filter_stmt => BundleCommand::Filter(FilterCommand::from_statement(inner_stmt)?),
```

## Commands Without SQL Parsing

Some commands cannot be parsed from SQL (e.g., `CreateViewCommand` which requires a builder reference). For these:

1. Return `None` from `fn rule()`:
   ```rust
   fn rule() -> Option<Rule> {
       None
   }
   ```

2. Provide a stub `from_statement`:
   ```rust
   fn from_statement(_pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
       Err("This command cannot be parsed from SQL".into())
   }
   ```

3. In `parser.rs::try_parse_pest()`, return a helpful error:
   ```rust
   Rule::create_view_stmt => {
       return Err("CREATE VIEW cannot be parsed from SQL. Use builder.create_view() API instead.".into());
   }
   ```
