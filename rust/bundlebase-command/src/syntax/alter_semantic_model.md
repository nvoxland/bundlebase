Edit the description or AI context for the currently opened semantic model.

```sql
ALTER SEMANTIC MODEL SET DESCRIPTION 'Customer orders';
ALTER SEMANTIC MODEL DROP DESCRIPTION;
ALTER SEMANTIC MODEL SET AI CONTEXT '{"purpose":"Sales reporting"}';
ALTER SEMANTIC MODEL SET AI CONTEXT MEMBER audience 'Finance analysts';
ALTER SEMANTIC MODEL DROP AI CONTEXT MEMBER audience;
ALTER SEMANTIC MODEL DROP AI CONTEXT;
```

Quoted values accept JSON or plain text. `MEMBER` edits a single top-level member.
Whole-context updates replace the current context; member updates preserve other members.
