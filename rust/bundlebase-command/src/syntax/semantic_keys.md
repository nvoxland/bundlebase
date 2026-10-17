Declare the columns that identify records in the current bundle's dataset.
Keys are semantic metadata and do not enforce uniqueness or scan the data.

```sql
DECLARE PRIMARY KEY (order_id);
DECLARE UNIQUE KEY (customer_id, order_date);
DROP PRIMARY KEY;
DROP UNIQUE KEY (customer_id, order_date);
```

A dataset has one primary key and may have multiple unique keys. Column references
use stable identities, so declarations follow column renames.
