Inspect the current semantic model directly, or exchange it as an optional document.

```sql
SHOW SEMANTIC MODEL;
EXPORT SEMANTIC MODEL TO 'sales.yaml';
IMPORT SEMANTIC MODEL FROM 'sales.yaml';
IMPORT SEMANTIC MODEL FROM 'sales.yaml' MERGE;
IMPORT SEMANTIC MODEL FROM 'sales.yaml' REPLACE;
```

`SHOW` returns the resolved semantic model. Reads and exports work on read-only bundles.
Import requires a writable bundle. Merge is the default: supplied properties are updated,
and omitted properties remain unchanged. Explicit replacement clears omitted editable
metadata in the document's supported scope. Imports record ordinary property operations
and can be undone before committing.
