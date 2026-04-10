//! Static command metadata for the commands table.
//!
//! This provides a hardcoded list of all registered bundlebase commands and their
//! syntax descriptions. The data is used by the `commands_table` catalog provider
//! to populate the `bundle_info.commands` virtual table.
//!
//! This list must be kept in sync with the `register_commands!` macro invocation
//! in the `bundlebase-command` crate.

/// Returns metadata for all registered commands: (name, syntax, mode).
///
/// Mode is "read-write" for builder commands and "read-only" for facade commands.
pub fn command_metadata() -> Vec<(&'static str, &'static str, &'static str)> {
    let mut entries = vec![
        // Message commands (read-write)
        ("ATTACH", "ATTACH '<path>' [TO <pack>] [WITH (<options>)]", "read-write"),
        ("DETACH", "DETACH '<location>'", "read-write"),
        ("FILTER", "FILTER WITH <select_query>", "read-write"),
        ("IMPORT JOIN", "IMPORT JOIN <name> [FLATTEN HISTORY]", "read-write"),
        ("JOIN", "[LEFT|RIGHT|FULL|INNER] JOIN '<path>' AS <name> ON <expression>", "read-write"),
        ("REPLACE", "REPLACE '<old_location>' WITH '<new_location>'", "read-write"),
        ("ADD COLUMN", "ADD COLUMN <name> AS <expression>", "read-write"),
        ("CAST COLUMN", "CAST COLUMN <name> TO <type>", "read-write"),
        ("DROP COLUMN", "DROP COLUMN <name>", "read-write"),
        ("RENAME COLUMN", "RENAME COLUMN <old> TO <new>", "read-write"),
        ("CREATE INDEX", "CREATE <BTREE|TEXT> INDEX ON <column>", "read-write"),
        ("DROP INDEX", "DROP INDEX <column>", "read-write"),
        ("REBUILD INDEX", "REBUILD INDEX ON <column>", "read-write"),
        ("REINDEX", "REINDEX [ON data(<column>)]", "read-write"),
        ("CREATE VIEW", "CREATE VIEW <name> AS <sql>", "read-write"),
        ("RENAME VIEW", "RENAME VIEW <old> TO <new>", "read-write"),
        ("DROP VIEW", "DROP VIEW <name>", "read-write"),
        ("DROP JOIN", "DROP JOIN <name>", "read-write"),
        ("RENAME JOIN", "RENAME JOIN <old> TO <new>", "read-write"),
        ("SET NAME", "SET NAME '<name>'", "read-write"),
        ("SET DESCRIPTION", "SET DESCRIPTION '<description>'", "read-write"),
        ("SET MIN VERSION", "SET MIN VERSION '<version>'", "read-write"),
        ("SET MAX VERSION", "SET MAX VERSION '<version>'", "read-write"),
        ("SAVE CONFIG", "SAVE CONFIG <key> = '<value>' FOR '<scope>'", "read-write"),
        ("IMPORT CONNECTOR", "IMPORT CONNECTOR <name> FROM '<runtime::entrypoint>' [WITH (<args>)]", "read-write"),
        ("IMPORT FUNCTION", "IMPORT FUNCTION <name> FROM '<runtime::entrypoint>' [WITH (<args>)]", "read-write"),
        ("RENAME CONNECTOR", "RENAME CONNECTOR <old> TO <new>", "read-write"),
        ("RENAME FUNCTION", "RENAME FUNCTION <old> TO <new>", "read-write"),
        ("DROP CONNECTOR", "DROP CONNECTOR <name> [FOR PLATFORM '<platform>']", "read-write"),
        ("DROP FUNCTION", "DROP FUNCTION <name>", "read-write"),
        ("CREATE SOURCE", "CREATE SOURCE [FOR <pack>] USING <connector> [WITH (<args>)]", "read-write"),
        ("RESET", "RESET", "read-write"),
        ("UNDO", "UNDO", "read-write"),
        ("COMMIT", "COMMIT '<message>'", "read-write"),
        // Fetch commands (read-write)
        ("FETCH", "FETCH <pack> <ADD|UPDATE|SYNC> [DRY RUN]", "read-write"),
        ("FETCH ALL", "FETCH ALL <ADD|UPDATE|SYNC> [DRY RUN]", "read-write"),
        // Verification commands (read-write)
        ("VERIFY DATA", "VERIFY DATA [UPDATE]", "read-write"),
        // Facade commands (read-only)
        ("EXPORT DATA", "EXPORT DATA TO '<path>' <sql>", "read-only"),
        ("EXPORT HOLLOW", "EXPORT HOLLOW TO '<path>'", "read-only"),
        ("DESCRIBE CONNECTOR", "DESCRIBE CONNECTOR <name>", "read-only"),
        ("DESCRIBE FUNCTION", "DESCRIBE FUNCTION <name>", "read-only"),
        ("IMPORT TEMP CONNECTOR", "IMPORT TEMP CONNECTOR <name> FROM '<runtime::entrypoint>' [WITH (<args>)]", "read-only"),
        ("IMPORT TEMP FUNCTION", "IMPORT TEMP FUNCTION <name> FROM '<runtime::entrypoint>' [WITH (<args>)]", "read-only"),
        ("DROP TEMP CONNECTOR", "DROP TEMP CONNECTOR <name> [FOR PLATFORM '<platform>']", "read-only"),
        ("DROP TEMP FUNCTION", "DROP TEMP FUNCTION <name>", "read-only"),
        ("RENAME TEMP CONNECTOR", "RENAME TEMP CONNECTOR <old> TO <new>", "read-only"),
        ("RENAME TEMP FUNCTION", "RENAME TEMP FUNCTION <old> TO <new>", "read-only"),
        ("EXPLAIN", "EXPLAIN [ANALYZE] [VERBOSE] [FORMAT <format>] [<sql>]", "read-only"),
        ("SET CONFIG", "SET CONFIG <key> = '<value>' FOR '<scope>'", "read-only"),
        ("SHOW DETAILS", "SHOW DETAILS", "read-only"),
        ("SHOW HISTORY", "SHOW HISTORY", "read-only"),
        ("SHOW STATUS", "SHOW STATUS", "read-only"),
        ("SHOW VIEWS", "SHOW VIEWS", "read-only"),
        ("SHOW INDEXES", "SHOW INDEXES", "read-only"),
        ("SHOW PACKS", "SHOW PACKS", "read-only"),
        ("SHOW BLOCKS", "SHOW BLOCKS", "read-only"),
        ("SHOW CONFIG", "SHOW CONFIG", "read-only"),
        ("SHOW COMMANDS", "SHOW COMMANDS", "read-only"),
        ("SHOW CONNECTORS", "SHOW CONNECTORS", "read-only"),
        ("SHOW FUNCTIONS", "SHOW FUNCTIONS", "read-only"),
        ("SHOW COLUMNS", "SHOW COLUMNS", "read-only"),
        ("SHOW COUNT", "SHOW COUNT", "read-only"),
        ("SYNTAX", "SYNTAX [<command>]", "read-only"),
    ];
    entries.sort_by_key(|(name, _, _)| name.to_string());
    entries
}
