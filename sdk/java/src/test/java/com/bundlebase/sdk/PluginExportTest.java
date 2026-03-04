package com.bundlebase.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import org.apache.arrow.memory.BufferAllocator;
import org.apache.arrow.memory.RootAllocator;
import org.apache.arrow.vector.BigIntVector;
import org.apache.arrow.vector.VarCharVector;
import org.apache.arrow.vector.VectorSchemaRoot;
import org.apache.arrow.vector.types.pojo.ArrowType;
import org.apache.arrow.vector.types.pojo.Field;
import org.apache.arrow.vector.types.pojo.Schema;
import org.junit.Test;

import java.lang.reflect.Method;
import java.util.*;

import static org.junit.Assert.*;

/**
 * Tests for PluginExport business logic.
 *
 * <p>These test the JSON parsing, source delegation, and error handling
 * that runs inside the Panama upcall targets. We use reflection to call
 * the private doDiscover/doData/doStableUrl methods since they contain
 * the actual logic.
 */
public class PluginExportTest {

    private static final ObjectMapper MAPPER = new ObjectMapper();

    private static SourceFunction testSource() {
        return new SourceFunction() {
            private final BufferAllocator allocator = new RootAllocator();

            @Override
            public List<Location> discover(List<String> attached, Map<String, String> args) {
                List<Location> locs = new ArrayList<>();
                locs.add(new Location("data.parquet", true, "parquet", "v1"));
                if (args.containsKey("extra")) {
                    locs.add(new Location(args.get("extra")));
                }
                return locs;
            }

            @Override
            public VectorSchemaRoot data(Location location, Map<String, String> args) {
                if ("data.parquet".equals(location.location())) {
                    Schema schema = new Schema(List.of(
                            Field.nullable("id", new ArrowType.Int(64, true)),
                            Field.nullable("name", new ArrowType.Utf8())
                    ));
                    VectorSchemaRoot root = VectorSchemaRoot.create(schema, allocator);
                    root.allocateNew();
                    ((BigIntVector) root.getVector("id")).setSafe(0, 1);
                    ((BigIntVector) root.getVector("id")).setSafe(1, 2);
                    ((VarCharVector) root.getVector("name")).setSafe(0, "alice".getBytes());
                    ((VarCharVector) root.getVector("name")).setSafe(1, "bob".getBytes());
                    root.setRowCount(2);
                    return root;
                }
                return null;
            }

            @Override
            public StableUrl stableUrl(Location location, Map<String, String> args) {
                if ("data.parquet".equals(location.location())) {
                    return new StableUrl("https://example.com/data.parquet");
                }
                return null;
            }
        };
    }

    private String callDoDiscover(String argsJson) throws Exception {
        PluginExport.register(testSource());
        Method m = PluginExport.class.getDeclaredMethod("doDiscover", String.class);
        m.setAccessible(true);
        return (String) m.invoke(null, argsJson);
    }

    private String callDoStableUrl(String locationJson, String argsJson) throws Exception {
        PluginExport.register(testSource());
        Method m = PluginExport.class.getDeclaredMethod("doStableUrl", String.class, String.class);
        m.setAccessible(true);
        return (String) m.invoke(null, locationJson, argsJson);
    }

    @Test
    public void testDiscoverReturnsLocations() throws Exception {
        String result = callDoDiscover("{\"attached_locations\": []}");
        JsonNode json = MAPPER.readTree(result);
        JsonNode locations = json.get("locations");
        assertEquals(1, locations.size());
        assertEquals("data.parquet", locations.get(0).get("location").asText());
        assertEquals("v1", locations.get(0).get("version").asText());
    }

    @Test
    public void testDiscoverPassesExtraArgs() throws Exception {
        String result = callDoDiscover("{\"attached_locations\": [], \"extra\": \"bonus.csv\"}");
        JsonNode json = MAPPER.readTree(result);
        JsonNode locations = json.get("locations");
        assertEquals(2, locations.size());
        assertEquals("bonus.csv", locations.get(1).get("location").asText());
    }

    @Test
    public void testDiscoverParsesAttachedLocations() throws Exception {
        // Attached locations are parsed but not passed through to the response;
        // they go to the discover method's attached parameter.
        String result = callDoDiscover("{\"attached_locations\": [\"loc1\", \"loc2\"]}");
        JsonNode json = MAPPER.readTree(result);
        assertNotNull(json.get("locations"));
    }

    @Test(expected = Exception.class)
    public void testDiscoverNoSourceRegistered() throws Exception {
        PluginExport.register(null);
        Method m = PluginExport.class.getDeclaredMethod("doDiscover", String.class);
        m.setAccessible(true);
        m.invoke(null, "{\"attached_locations\": []}");
    }

    @Test
    public void testStableUrlPresent() throws Exception {
        String result = callDoStableUrl(
                "{\"location\": \"data.parquet\", \"must_copy\": true, \"format\": \"parquet\", \"version\": \"v1\"}",
                "{}");
        JsonNode json = MAPPER.readTree(result);
        assertEquals("https://example.com/data.parquet", json.get("url").asText());
    }

    @Test
    public void testStableUrlNone() throws Exception {
        String result = callDoStableUrl(
                "{\"location\": \"other.parquet\"}",
                "{}");
        assertNull(result);
    }

    @Test
    public void testRegisterSource() {
        PluginExport.register(testSource());
        // Should not throw — source is registered
        // We can't easily verify without calling discover, but at minimum
        // register should accept a non-null source
    }

    @Test
    public void testDiscoverExcludesAttachedFromArgs() throws Exception {
        // Verify that "attached_locations" key is not passed as an extra arg
        SourceFunction argTracker = new SourceFunction() {
            @Override
            public List<Location> discover(List<String> attached, Map<String, String> args) {
                // If attached_locations leaked into args, this would show it
                return List.of(new Location(args.containsKey("attached_locations") ? "LEAKED" : "clean"));
            }

            @Override
            public VectorSchemaRoot data(Location location, Map<String, String> args) {
                return null;
            }
        };

        PluginExport.register(argTracker);
        Method m = PluginExport.class.getDeclaredMethod("doDiscover", String.class);
        m.setAccessible(true);
        String result = (String) m.invoke(null,
                "{\"attached_locations\": [\"x\"], \"real_arg\": \"val\"}");

        JsonNode json = MAPPER.readTree(result);
        assertEquals("clean", json.get("locations").get(0).get("location").asText());
    }
}
