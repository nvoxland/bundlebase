Create a named view from a SQL query. Views act as saved queries that can be referenced by name.

### Examples

    CREATE VIEW active_users AS SELECT * FROM bundle WHERE status = 'active'
    CREATE VIEW summary AS SELECT category, COUNT(*) as cnt FROM bundle GROUP BY category
