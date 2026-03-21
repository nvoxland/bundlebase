Filter rows using a SQL query. The query is passed directly to DataFusion for execution.

### Examples

    FILTER WITH SELECT * FROM bundle WHERE country = 'USA'
    FILTER WITH SELECT * FROM bundle WHERE age > 21 AND status = 'active'
    FILTER WITH SELECT id, name FROM bundle WHERE score >= 90
