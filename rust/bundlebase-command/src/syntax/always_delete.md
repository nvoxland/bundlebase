Register a persistent delete rule that automatically applies to all future ATTACH operations. Also immediately deletes matching rows from current data.

Multiple rules can be added and they accumulate. Use DROP ALWAYS DELETE to remove rules.

### Examples

    ALWAYS DELETE FROM bundle WHERE salary < 0
    ALWAYS DELETE FROM bundle WHERE status = 'inactive' AND last_login < '2020-01-01'

### Removing Rules

    DROP ALWAYS DELETE WHERE salary < 0
    DROP ALWAYS DELETE
