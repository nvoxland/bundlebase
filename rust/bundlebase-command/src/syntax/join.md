Join the bundle with another data source. Supports INNER, LEFT, RIGHT, FULL OUTER, and OUTER join types. The default is INNER JOIN.

### Examples

    JOIN 'other.csv' AS other ON id = other.id
    LEFT JOIN 'users.parquet' AS users ON user_id = users.id
    FULL OUTER JOIN 'data.json' AS data ON key = data.key
