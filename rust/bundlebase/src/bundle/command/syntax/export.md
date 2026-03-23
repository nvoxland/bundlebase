# EXPORT

Exports query results to a file. The output format is determined by the file extension.

## Syntax

```sql
EXPORT TO '<path>' <sql>
```

## Supported Formats

| Extension | Format |
|-----------|--------|
| `.csv` | Comma-separated values |
| `.jsonl` | JSON Lines (one JSON object per line) |

## Examples

```sql
-- Export all data to CSV
EXPORT TO 'output.csv' SELECT * FROM bundle

-- Export filtered results to JSON Lines
EXPORT TO '/tmp/active_users.jsonl' SELECT * FROM bundle WHERE active = true

-- Export aggregated results
EXPORT TO 'summary.csv' SELECT department, COUNT(*) as cnt, AVG(salary) as avg_sal FROM bundle GROUP BY department

-- Export with a row limit
EXPORT TO 'sample.csv' SELECT * FROM bundle LIMIT 100
```
