# Java SDK

Build custom Bundlebase connectors in Java.

## Installation

Add the SDK to your Maven `pom.xml`:

```xml
<dependency>
    <groupId>com.bundlebase</groupId>
    <artifactId>bundlebase-sdk</artifactId>
    <version>0.1.0</version>
</dependency>
```

Requires Apache Arrow Java (`org.apache.arrow:arrow-vector` and `org.apache.arrow:arrow-memory-netty`).

## Quick Start

```java
import com.bundlebase.sdk.*;
import org.apache.arrow.memory.RootAllocator;
import org.apache.arrow.vector.*;
import org.apache.arrow.vector.types.pojo.*;
import java.util.*;

public class ExampleConnector implements Connector {
    private final RootAllocator allocator = new RootAllocator();

    public List<Location> discover(List<String> attached, Map<String, String> args) {
        return List.of(new Location("data.parquet", true, "parquet", "v1"));
    }

    public VectorSchemaRoot data(Location loc, Map<String, String> args) {
        Schema schema = new Schema(List.of(
            Field.nullable("id", new ArrowType.Int(64, true)),
            Field.nullable("value", new ArrowType.Utf8())
        ));
        VectorSchemaRoot root = VectorSchemaRoot.create(schema, allocator);
        root.allocateNew();
        ((BigIntVector) root.getVector("id")).setSafe(0, 1);
        ((VarCharVector) root.getVector("value")).setSafe(0, "a".getBytes());
        root.setRowCount(1);
        return root;
    }

    public static void main(String[] args) { Serve.run(new ExampleConnector()); }
}
```

Build a JAR and use with:

```python
bundle.import_connector('example.connector', 'java::target/example-connector.jar')
bundle.create_source('example.connector')
```

## API Reference

### Connector Interface

```java
public interface Connector {
    List<Location> discover(List<String> attachedLocations, Map<String, String> args);
    Object data(Location location, Map<String, String> args);
    default Map<String, String> schema() { return null; }
    default StableUrl stableUrl(Location location, Map<String, String> args) { return null; }
}
```

#### `discover(attachedLocations, args)`

Return the available data locations.

| Parameter | Type | Description |
|-----------|------|-------------|
| `attachedLocations` | `List<String>` | Locations already attached to the bundle |
| `args` | `Map<String, String>` | Extra arguments from the source configuration |

**Returns:** `List<Location>`

#### `data(location, args)`

Return data for the given location. Return `null` for no data.

| Parameter | Type | Description |
|-----------|------|-------------|
| `location` | `Location` | The location to fetch data for |
| `args` | `Map<String, String>` | Extra arguments from the source configuration |

**Returns:** One of the supported [data return types](#data-return-types).

#### `schema()`

Optional schema for dict-to-Arrow conversion. Required when `data()` returns `List<Map>` or `Map<String, List>`. Return `null` (default) when `data()` returns `VectorSchemaRoot`.

**Returns:** `Map<String, String>` mapping column names to type strings, or `null`

#### `stableUrl(location, args)`

Return a stable URL for the given location. Default returns `null`.

| Parameter | Type | Description |
|-----------|------|-------------|
| `location` | `Location` | The location to get a URL for |
| `args` | `Map<String, String>` | Extra arguments from the source configuration |

**Returns:** `StableUrl` or `null`

### Location

```java
public record Location(String location, boolean mustCopy, String format, String version) {
    public Location(String location) { this(location, true, "parquet", ""); }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `location` | `String` | *(required)* | Identifier for this data file |
| `mustCopy` | `boolean` | `true` | Whether the data must be copied into the bundle |
| `format` | `String` | `"parquet"` | File format |
| `version` | `String` | `""` | Version string for change detection |

The single-argument constructor `new Location("path")` defaults to `mustCopy=true`, `format="parquet"`, `version=""`.

### StableUrl

```java
public record StableUrl(String url) {}
```

| Field | Type | Description |
|-------|------|-------------|
| `url` | `String` | The stable URL string |

### Serve

```java
public class Serve {
    public static void run(Connector source);
    public static void run(Connector source, InputStream in, OutputStream out);
}
```

`run(source)` reads from stdin and writes to stdout. The two-argument overload accepts explicit streams for testing.

## Data Return Types

The `data()` method supports several return types:

| Return Type | Description |
|------------|-------------|
| `VectorSchemaRoot` | Arrow data directly (most control) |
| `List<Map<String, Object>>` | Row-oriented dicts (requires `schema()`) |
| `Map<String, List<?>>` | Column-oriented dict (requires `schema()`) |
| `null` | No data for this location |

A `schema()` method is required when returning dict data (`List<Map>` or `Map<String, List>`). Returning dict data without a schema throws an `IllegalArgumentException`.

## Schema-Driven Connectors

Instead of constructing `VectorSchemaRoot` manually, you can define a `schema()` method with simple type strings and return plain Java collections. The SDK handles Arrow conversion automatically.

### Defining a Schema

Override `schema()` to return a map of column names to type strings:

```java
@Override
public Map<String, String> schema() {
    Map<String, String> s = new LinkedHashMap<>();
    s.put("name", "Utf8");
    s.put("age", "Int32");
    s.put("score", "Float64");
    return s;
}
```

Supported type strings:

| Type String | Arrow Type | Aliases |
|------------|------------|---------|
| `Utf8` | `Utf8` | `string` |
| `Int64` | `Int(64, signed)` | `int` |
| `Int8`, `Int16`, `Int32` | `Int(8/16/32, signed)` | |
| `UInt8`, `UInt16`, `UInt32`, `UInt64` | `Int(8-64, unsigned)` | |
| `Float64` | `FloatingPoint(DOUBLE)` | `float`, `double` |
| `Float16`, `Float32` | `FloatingPoint(HALF/SINGLE)` | |
| `Boolean` | `Bool` | `bool` |
| `Date32` | `Date(DAY)` | `date` |
| `Date64` | `Date(MILLISECOND)` | |
| `Timestamp` | `Timestamp(MICROSECOND)` | |
| `Binary` | `Binary` | `bytes` |

### Returning Column-Oriented Data

With a schema defined, return data as `Map<String, List<?>>`:

```java
@Override
public Object data(Location location, Map<String, String> args) {
    Map<String, List<?>> cols = new LinkedHashMap<>();
    cols.put("name", List.of("alice", "bob"));
    cols.put("age", List.of(30, 25));
    return cols;
}
```

### Returning Row-Oriented Data

Return `List<Map<String, Object>>`:

```java
@Override
public Object data(Location location, Map<String, String> args) {
    return List.of(
        Map.of("name", "alice", "age", 30),
        Map.of("name", "bob", "age", 25)
    );
}
```

### Full Schema Example

```java
import com.bundlebase.sdk.*;
import java.util.*;

public class SensorConnector implements Connector {
    @Override
    public Map<String, String> schema() {
        Map<String, String> s = new LinkedHashMap<>();
        s.put("sensor_id", "Utf8");
        s.put("temperature", "Float32");
        s.put("reading_count", "Int32");
        return s;
    }

    @Override
    public List<Location> discover(List<String> attached, Map<String, String> args) {
        return List.of(new Location("readings"));
    }

    @Override
    public Object data(Location location, Map<String, String> args) {
        Map<String, List<?>> cols = new LinkedHashMap<>();
        cols.put("sensor_id", List.of("s1", "s2", "s3"));
        cols.put("temperature", List.of(22.5, 19.8, 25.1));
        cols.put("reading_count", List.of(100, 85, 120));
        return cols;
    }

    public static void main(String[] args) { Serve.run(new SensorConnector()); }
}
```

## Complete Example

A source with multiple locations, stable URL support, and extra argument handling:

```java
import com.bundlebase.sdk.*;
import org.apache.arrow.memory.BufferAllocator;
import org.apache.arrow.memory.RootAllocator;
import org.apache.arrow.vector.*;
import org.apache.arrow.vector.types.pojo.*;
import java.util.*;

public class DatabaseSource implements Connector {
    private final BufferAllocator allocator = new RootAllocator();

    @Override
    public List<Location> discover(List<String> attachedLocations, Map<String, String> args) {
        return List.of(
            new Location("users.parquet", true, "parquet", "v2"),
            new Location("orders.parquet", true, "parquet", "v1")
        );
    }

    @Override
    public VectorSchemaRoot data(Location location, Map<String, String> args) {
        switch (location.location()) {
            case "users.parquet" -> {
                Schema schema = new Schema(Arrays.asList(
                    Field.nullable("id", new ArrowType.Int(64, true)),
                    Field.nullable("name", new ArrowType.Utf8())
                ));
                VectorSchemaRoot root = VectorSchemaRoot.create(schema, allocator);
                root.allocateNew();
                BigIntVector idVec = (BigIntVector) root.getVector("id");
                VarCharVector nameVec = (VarCharVector) root.getVector("name");
                idVec.setSafe(0, 1); nameVec.setSafe(0, "alice".getBytes());
                idVec.setSafe(1, 2); nameVec.setSafe(1, "bob".getBytes());
                idVec.setSafe(2, 3); nameVec.setSafe(2, "charlie".getBytes());
                root.setRowCount(3);
                return root;
            }
            case "orders.parquet" -> {
                Schema schema = new Schema(Arrays.asList(
                    Field.nullable("order_id", new ArrowType.Int(64, true)),
                    Field.nullable("user_id", new ArrowType.Int(64, true)),
                    Field.nullable("amount", new ArrowType.FloatingPoint(org.apache.arrow.vector.types.FloatingPointPrecision.DOUBLE))
                ));
                VectorSchemaRoot root = VectorSchemaRoot.create(schema, allocator);
                root.allocateNew();
                ((BigIntVector) root.getVector("order_id")).setSafe(0, 101);
                ((BigIntVector) root.getVector("user_id")).setSafe(0, 1);
                ((Float8Vector) root.getVector("amount")).setSafe(0, 29.99);
                root.setRowCount(1);
                return root;
            }
            default -> { return null; }
        }
    }

    @Override
    public StableUrl stableUrl(Location location, Map<String, String> args) {
        if ("users.parquet".equals(location.location())) {
            return new StableUrl("https://db.example.com/exports/users-v2.parquet");
        }
        return null;
    }

    public static void main(String[] args) { Serve.run(new DatabaseSource()); }
}
```

## Testing

The `Serve.run()` method accepts explicit `InputStream`/`OutputStream` for testing without launching a subprocess:

```java
import com.bundlebase.sdk.*;
import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import org.junit.Test;
import java.io.*;
import java.util.*;
import static org.junit.Assert.*;

public class ExampleConnectorTest {
    private static final ObjectMapper MAPPER = new ObjectMapper();

    private static byte[] makeRequest(String method, Map<String, Object> params, int id)
            throws Exception {
        Map<String, Object> req = new LinkedHashMap<>();
        req.put("jsonrpc", "2.0");
        req.put("id", id);
        req.put("method", method);
        req.put("params", params != null ? params : Map.of());
        return (MAPPER.writeValueAsString(req) + "\n").getBytes();
    }

    @Test
    public void testDiscover() throws Exception {
        ByteArrayOutputStream input = new ByteArrayOutputStream();
        input.write(makeRequest("discover", Map.of("attached_locations", List.of()), 1));
        input.write(makeRequest("shutdown", null, 2));

        ByteArrayOutputStream output = new ByteArrayOutputStream();
        Serve.run(new ExampleConnector(), new ByteArrayInputStream(input.toByteArray()), output);

        String[] lines = output.toString().split("\n");
        JsonNode resp = MAPPER.readTree(lines[0]);
        assertEquals(1, resp.get("id").asInt());
        JsonNode locations = resp.get("result").get("locations");
        assertEquals(1, locations.size());
        assertEquals("data.parquet", locations.get(0).get("location").asText());
    }

    @Test
    public void testDataReturnsArrow() throws Exception {
        ByteArrayOutputStream input = new ByteArrayOutputStream();
        input.write(makeRequest("data", Map.of(
            "location", Map.of("location", "data.parquet")
        ), 1));
        input.write(makeRequest("shutdown", null, 2));

        ByteArrayOutputStream output = new ByteArrayOutputStream();
        Serve.run(new ExampleConnector(), new ByteArrayInputStream(input.toByteArray()), output);

        byte[] out = output.toByteArray();
        int newlineIdx = 0;
        while (newlineIdx < out.length && out[newlineIdx] != '\n') newlineIdx++;
        newlineIdx++;

        int length = java.nio.ByteBuffer.wrap(out, newlineIdx, 4).getInt();
        assertTrue("Expected non-zero Arrow IPC data", length > 0);
    }
}
```

## Native Mode

Build your connector as a shared library using Project Panama (Java 22+) for zero-copy in-process loading.

### Requirements

- **Java 22+** — uses the Foreign Function & Memory API (JEP 454)
- GCC or Clang for compiling the thin C bootstrap

### Setup

Register your connector with `PluginExport`:

```java
import com.bundlebase.sdk.*;

public class ExampleNativeConnector implements Connector {
    // ... implement discover() and data() as usual ...

    static {
        PluginExport.register(new ExampleNativeConnector());
    }
}
```

### Build

Build the Panama bridge and your Java code into a shared library:

```bash
# Build Java classes
mvn compile

# Build native shim (from sdk/java directory)
mvn compile -Dnative
```

This produces `libbundlebase_plugin.so` in `target/`.

### Use

```python
bundle.import_connector('example.connector', 'ffi::./libbundlebase_plugin.so')
bundle.create_source('example.connector')
```

### How It Works

The native bridge uses Project Panama's Foreign Function & Memory API for high-performance Java↔C interop:

1. A thin C bootstrap (`bundlebase_plugin.c`) starts the JVM once at library load
2. It calls `PluginExport.initialize()` — a single JNI call that registers Panama upcall stubs
3. All subsequent `bundlebase_discover`, `bundlebase_data`, and `bundlebase_stable_url` calls route through Panama function pointers — no JNI method dispatch on the hot path
4. Strings are allocated via `malloc()` in Java (using Panama downcalls) and freed by Bundlebase via `bundlebase_free()`
5. Arrow data is transferred via the Arrow C Data Interface (`ArrowArrayStream`)

This architecture means JNI is used only for the one-time JVM bootstrap. All data-path calls use Panama upcalls, which avoids JNI overhead (method ID lookups, string conversions in C, etc.).

The same `Connector` interface works for both native and IPC — switch between them by changing only the entry point (`PluginExport.register()` + native build vs `Serve.run()` + JAR).

See [Native Mode](native.md) for the full C ABI reference.

## Error Handling

Exceptions thrown in your `discover()`, `data()`, or `stableUrl()` methods are caught by the SDK and returned as JSON-RPC error responses with code `-32000`. The exception message is included:

```java
public VectorSchemaRoot data(Location location, Map<String, String> args) {
    throw new RuntimeException("Database connection failed");
    // Bundlebase receives: {"error": {"code": -32000, "message": "Database connection failed"}}
}
```

The SDK automatically closes `VectorSchemaRoot` instances after sending the Arrow IPC data.

Bundlebase surfaces these errors to the user as connector failures during `FETCH`.
