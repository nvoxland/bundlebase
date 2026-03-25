Show the query execution plan. Use ANALYZE to run the query and show actual execution statistics. Use VERBOSE for additional detail. FORMAT controls the output format (INDENT, TREE, or GRAPHVIZ).

### Examples

    EXPLAIN
    EXPLAIN ANALYZE
    EXPLAIN VERBOSE
    EXPLAIN ANALYZE VERBOSE FORMAT TREE
    EXPLAIN SELECT * FROM bundle WHERE id > 10
