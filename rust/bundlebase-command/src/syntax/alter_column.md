Edit metadata attached to a column's stable identity. Renaming a column keeps its metadata.

```sql
ALTER COLUMN amount SET DESCRIPTION 'Gross sale amount before discounts';
ALTER COLUMN amount SET LABEL 'Gross sales';
ALTER COLUMN amount SET AI CONTEXT '{"unit":"USD","aggregation":"sum"}';
ALTER COLUMN amount SET AI CONTEXT MEMBER unit 'USD';
ALTER COLUMN amount DROP DESCRIPTION;
ALTER COLUMN amount DROP LABEL;
ALTER COLUMN amount DROP AI CONTEXT MEMBER unit;
ALTER COLUMN amount DROP AI CONTEXT;
```

`SET` requires a quoted value. AI context values accept JSON or plain text; `MEMBER`
edits a single top-level AI context member. Other properties do not accept `MEMBER`.
Each command records one property operation and supports normal undo and commit history.
