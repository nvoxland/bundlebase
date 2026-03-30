Undo uncommitted changes.

`UNDO` reverts the last uncommitted change, keeping earlier uncommitted changes intact. `UNDO LAST N` reverts the last N changes at once. Only works on uncommitted changes — committed operations cannot be undone.

### Examples

    UNDO
    UNDO LAST 3
