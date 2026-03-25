Add a computed column to the bundle using a SQL expression. The expression can reference existing columns.

### Examples

    ADD COLUMN full_name AS first_name || ' ' || last_name
    ADD COLUMN total AS price * quantity
    ADD COLUMN year AS EXTRACT(YEAR FROM order_date)
