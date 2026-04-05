# SQL Server

The Bundlebase CLI can run as an [SQL](https://arrow.apache.org/docs/format/FlightSql.html) server, providing remote access to bundles over the network. Any JDBC-compatible client (such as DBeaver) or ADBC client can connect and run queries.

## Starting the Server

```bash
bundlebase server --bundle <path> [options]
```

### Flags

| Flag | Default | Description |
|---|---|---|
| `--bundle <path>` | *(required)* | Path or URL to the bundle |
| `--read-only` | `false` | Only allow SELECT and EXPLAIN commands |
| `--host <addr>` | `0.0.0.0` | Host address to bind to |
| `--port <port>` | `50051` | Port to listen on |
| `--log-level <level>` | `ui` | Logging level |

### Examples

```bash
# Start with defaults (0.0.0.0:50051)
bundlebase server --bundle ./my-bundle

# Custom host and port
bundlebase server --bundle ./my-bundle --host 127.0.0.1 --port 8080

# Read-only server
bundlebase server --bundle ./my-bundle --read-only
```

To serve a new bundle, create it first with `bundlebase create`.

## Authentication

The server uses HTTP Basic authentication.

| Setting | Default |
|---|---|
| Username | `admin` |
| Password | `password` |

!!! warning
    The default credentials are intended for local development only. The server logs a warning at startup when default credentials are in use.

## Session Management

Each authenticated client gets its own session:

- Sessions are created on first connection (handshake)
- Each session maintains its own bundle state and prepared statements
- Sessions expire after **30 minutes** of inactivity
- Expired sessions are cleaned up automatically every 5 minutes
- If an expired token is reused, the server re-creates the session automatically

## Supported Operations

The SQL server supports the full set of Bundlebase SQL commands:

- **Queries**: `SELECT`, `EXPLAIN`
- **Data modification**: `ATTACH`, `DETACH`, `REPLACE`, `FILTER`
- **Schema changes**: `JOIN`, `DROP JOIN`, `DROP COLUMN`, `RENAME COLUMN`, `CREATE VIEW`, etc.
- **Version control**: `COMMIT`, `RESET`, `UNDO`, `VERIFY DATA`
- **Metadata**: `SET NAME`, `SET DESCRIPTION`, `SAVE CONFIG`, `SET CONFIG`
- **Introspection**: `SHOW HISTORY`, `SHOW STATUS`, `SHOW CONFIG`, etc.
- **Help**: `SYNTAX` to list all commands, `SYNTAX <command>` for detailed syntax
- **Prepared statements**: Create, execute, and close prepared statements

In read-only mode (`--read-only`), only `SELECT`, `EXPLAIN`, and `SET CONFIG` are allowed.

See the [SQL Reference](../sql-reference/index.md) for the full command syntax.

## Connecting from DBeaver

[DBeaver](https://dbeaver.io/) can connect via the SQL JDBC driver.

### Setup

1. Download the [Arrow Flight SQL JDBC driver](https://central.sonatype.com/artifact/org.apache.arrow/flight-sql-jdbc-driver) JAR
2. In DBeaver, go to **Database > Driver Manager > New** and add the JAR
3. Set the driver class to `org.apache.arrow.driver.jdbc.ArrowFlightJdbcDriver`
4. Create a new connection with URL:

    ```
    jdbc:arrow-flight-sql://localhost:50051
    ```

5. Enter credentials: username `admin`, password `password`

### Querying

Once connected, you can run SQL in the DBeaver SQL editor:

```sql
SELECT * FROM bundle
```

All Bundlebase commands work through the JDBC connection:

```sql
ATTACH 'new_data.parquet'
COMMIT 'Added new data via DBeaver'
```

## Connecting from TypeScript

Use the [Apache SQL client](https://www.npmjs.com/package/apache-arrow) to connect from TypeScript/Node.js via gRPC.

### Install

```bash
npm install @grpc/grpc-js apache-arrow
```

### Example

```typescript
import * as grpc from "@grpc/grpc-js";
import { FlightSqlClient } from "apache-arrow/flight";

const client = new FlightSqlClient("localhost:50051", {
  credentials: grpc.credentials.createInsecure(),
  username: "admin",
  password: "password",
});

// Authenticate
await client.handshake();

// Run a query
const info = await client.execute("SELECT * FROM bundle");
const reader = await client.doGet(info.endpoints[0].ticket);

for await (const batch of reader) {
  console.log(batch.toArray());
}

await client.close();
```

## Read-Only Mode

When started with `--read-only`, the server restricts operations:

- `SELECT` and `EXPLAIN` queries execute normally
- All modification commands (`ATTACH`, `FILTER`, `COMMIT`, etc.) return an error
- Useful for serving bundles as a read-only data source
