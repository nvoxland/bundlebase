# EXPORT DATA

Exports query results to a file. The output format is determined by the file extension.

## Syntax

```sql
EXPORT DATA TO '<path>' <sql>
```

## Supported Formats

| Extension | Format |
|-----------|--------|
| `.csv` | Comma-separated values |
| `.jsonl` | JSON Lines (one JSON object per line) |

## Examples

```sql
-- Export all data to CSV
EXPORT DATA TO 'output.csv' SELECT * FROM bundle

-- Export filtered results to JSON Lines
EXPORT DATA TO '/tmp/active_users.jsonl' SELECT * FROM bundle WHERE active = true

-- Export aggregated results
EXPORT DATA TO 'summary.csv' SELECT department, COUNT(*) as cnt, AVG(salary) as avg_sal FROM bundle GROUP BY department

-- Export with a row limit
EXPORT DATA TO 'sample.csv' SELECT * FROM bundle LIMIT 100
```
