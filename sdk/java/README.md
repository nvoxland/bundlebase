# Bundlebase Java SDK

Build custom Bundlebase source functions in Java.

## Installation

Add the SDK to your Maven project's `pom.xml`:

```xml
<dependency>
    <groupId>com.bundlebase</groupId>
    <artifactId>sdk</artifactId>
    <version>1.0.0</version>
</dependency>
```

## Quick Start

Implement the `SourceFunction` interface:

```java
import com.bundlebase.sdk.*;
import org.apache.arrow.memory.BufferAllocator;
import org.apache.arrow.memory.RootAllocator;
import org.apache.arrow.vector.BigIntVector;
import org.apache.arrow.vector.VarCharVector;
import org.apache.arrow.vector.VectorSchemaRoot;
import org.apache.arrow.vector.types.pojo.ArrowType;
import org.apache.arrow.vector.types.pojo.Field;
import org.apache.arrow.vector.types.pojo.Schema;
import java.util.*;

public class MySource implements SourceFunction {
    private final BufferAllocator allocator = new RootAllocator();

    @Override
    public List<Location> discover(List<String> attachedLocations, Map<String, String> args) {
        return List.of(
            new Location("data1.parquet", true, "parquet", "v1"),
            new Location("data2.parquet", true, "parquet", "v1")
        );
    }

    @Override
    public VectorSchemaRoot data(Location location, Map<String, String> args) {
        Schema schema = new Schema(Arrays.asList(
            Field.nullable("id", new ArrowType.Int(64, true)),
            Field.nullable("name", new ArrowType.Utf8())
        ));
        VectorSchemaRoot root = VectorSchemaRoot.create(schema, allocator);
        // Populate root with data...
        return root;
    }

    public static void main(String[] args) {
        Serve.run(new MySource());
    }
}
```

## Implementation

Implement the `SourceFunction` interface:

- **`discover(attachedLocations, args)`** - Return available data locations as a list of `Location` objects
- **`data(location, args)`** - Return data for a location as an Arrow `VectorSchemaRoot`
- **`stableUrl(location, args)`** (optional) - Return a stable URL for a location

Call `Serve.run(instance)` to start the source function server.

## Documentation

For complete documentation, including advanced usage and API details, see [Custom Source Functions](../../docs/guide/custom-sources/).
