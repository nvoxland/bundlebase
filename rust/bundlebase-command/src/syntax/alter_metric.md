Update one property of a named metric. Updating its expression preserves its description
and AI context. Column and function references follow stable identities across renames.

```sql
ALTER METRIC revenue SET EXPRESSION 'SUM(amount)';
ALTER METRIC revenue SET DESCRIPTION 'Gross revenue before discounts';
ALTER METRIC revenue SET AI CONTEXT '{"unit":"USD"}';
ALTER METRIC revenue SET AI CONTEXT MEMBER usage 'Compare within the same currency';
ALTER METRIC revenue DROP DESCRIPTION;
ALTER METRIC revenue DROP AI CONTEXT MEMBER usage;
ALTER METRIC revenue DROP AI CONTEXT;
```

AI context values accept quoted JSON or plain text. Only AI context supports `MEMBER`.
A metric expression cannot be cleared; use `DROP METRIC <name>` to remove a metric.
