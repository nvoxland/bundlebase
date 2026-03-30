Rename all columns to clean lowercase+underscore identifiers.

Converts spaces, dots, dashes, and other special characters to underscores. Collapses consecutive underscores. Strips leading/trailing underscores. Handles duplicates by appending `_2`, `_3`, etc.

### Examples

    NORMALIZE COLUMN NAMES

Before: `First Name`, `Last Name`, `Phone #`, `DOB`
After: `first_name`, `last_name`, `phone`, `dob`
