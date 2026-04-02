Test a connector without creating a source. Calls discover() and data() to validate the integration, showing discovered locations, schema, and sample data.

### Syntax

    TEST CONNECTOR <name> [WITH (<key> = '<value>', ...)]
    TEST TEMP CONNECTOR '<runtime>::<entrypoint>' [WITH (<key> = '<value>', ...)]

The first form tests an already-imported connector. The second tests inline without importing.

### Examples

    TEST CONNECTOR http WITH (url = 'https://example.com/data.csv')
    TEST CONNECTOR acme.weather WITH (region = 'us-east')
    TEST TEMP CONNECTOR 'ipc::./my-connector' WITH (path = '/data')
