Edit semantic metadata for the current bundle's dataset.

```sql
ALTER DATASET bundle SET DESCRIPTION 'Order transactions';
ALTER DATASET bundle SET AI CONTEXT '{"grain":"one row per order"}';
ALTER DATASET bundle SET AI CONTEXT MEMBER freshness 'Updated daily';
ALTER DATASET bundle DROP DESCRIPTION;
ALTER DATASET bundle DROP AI CONTEXT MEMBER freshness;
ALTER DATASET bundle DROP AI CONTEXT;
```

AI context values accept quoted JSON or plain text. Only AI context supports `MEMBER`.
