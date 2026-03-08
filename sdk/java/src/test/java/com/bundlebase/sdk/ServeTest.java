package com.bundlebase.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import org.apache.arrow.memory.BufferAllocator;
import org.apache.arrow.memory.RootAllocator;
import org.apache.arrow.vector.BigIntVector;
import org.apache.arrow.vector.VectorSchemaRoot;
import org.apache.arrow.vector.ipc.ArrowStreamReader;
import org.apache.arrow.vector.types.FloatingPointPrecision;
import org.apache.arrow.vector.types.pojo.ArrowType;
import org.apache.arrow.vector.types.pojo.Field;
import org.apache.arrow.vector.types.pojo.Schema;
import org.junit.Test;

import java.io.*;
import java.nio.ByteBuffer;
import java.util.*;

import static org.junit.Assert.*;

public class ServeTest {
    private static final ObjectMapper MAPPER = new ObjectMapper();

    private static byte[] makeRequest(String method, Map<String, Object> params, int id) throws Exception {
        Map<String, Object> req = new LinkedHashMap<>();
        req.put("jsonrpc", "2.0");
        req.put("id", id);
        req.put("method", method);
        req.put("params", params != null ? params : Map.of());
        return (MAPPER.writeValueAsString(req) + "\n").getBytes();
    }

    private static Connector simpleSource() {
        return new Connector() {
            private final BufferAllocator allocator = new RootAllocator();

            @Override
            public List<Location> discover(List<String> attached, Map<String, String> args) {
                return List.of(
                    new Location("f1.csv", true, "csv", "v1"),
                    new Location("f2.csv", false, "csv", "v2")
                );
            }

            @Override
            public VectorSchemaRoot data(Location location, Map<String, String> args) {
                if ("f1.csv".equals(location.location())) {
                    Schema schema = new Schema(List.of(
                        Field.nullable("id", new ArrowType.Int(64, true))
                    ));
                    VectorSchemaRoot root = VectorSchemaRoot.create(schema, allocator);
                    root.allocateNew();
                    ((BigIntVector) root.getVector("id")).setSafe(0, 1);
                    ((BigIntVector) root.getVector("id")).setSafe(1, 2);
                    root.setRowCount(2);
                    return root;
                }
                return null;
            }
        };
    }

    @Test
    public void testDiscover() throws Exception {
        ByteArrayOutputStream input = new ByteArrayOutputStream();
        input.write(makeRequest("discover", Map.of("attached_locations", List.of()), 1));
        input.write(makeRequest("shutdown", null, 2));

        ByteArrayOutputStream output = new ByteArrayOutputStream();
        Serve.run(simpleSource(), new ByteArrayInputStream(input.toByteArray()), output);

        String[] lines = output.toString().split("\n");
        JsonNode resp = MAPPER.readTree(lines[0]);
        assertEquals(1, resp.get("id").asInt());
        JsonNode locations = resp.get("result").get("locations");
        assertEquals(2, locations.size());
        assertEquals("f1.csv", locations.get(0).get("location").asText());
    }

    @Test
    public void testDataReturnsArrow() throws Exception {
        ByteArrayOutputStream input = new ByteArrayOutputStream();
        input.write(makeRequest("data", Map.of(
            "location", Map.of("location", "f1.csv")
        ), 1));
        input.write(makeRequest("shutdown", null, 2));

        ByteArrayOutputStream output = new ByteArrayOutputStream();
        Serve.run(simpleSource(), new ByteArrayInputStream(input.toByteArray()), output);

        byte[] out = output.toByteArray();
        // Find the end of the first JSON line
        int newlineIdx = 0;
        while (newlineIdx < out.length && out[newlineIdx] != '\n') newlineIdx++;
        newlineIdx++; // skip newline

        // Read length prefix
        int length = ByteBuffer.wrap(out, newlineIdx, 4).getInt();
        assertTrue("Expected non-zero Arrow IPC data", length > 0);

        // Verify Arrow IPC
        BufferAllocator allocator = new RootAllocator();
        byte[] ipcData = Arrays.copyOfRange(out, newlineIdx + 4, newlineIdx + 4 + length);
        ArrowStreamReader reader = new ArrowStreamReader(new ByteArrayInputStream(ipcData), allocator);
        assertTrue(reader.loadNextBatch());
        assertEquals(2, reader.getVectorSchemaRoot().getRowCount());
        reader.close();
        allocator.close();
    }

    @Test
    public void testDataNone() throws Exception {
        ByteArrayOutputStream input = new ByteArrayOutputStream();
        input.write(makeRequest("data", Map.of(
            "location", Map.of("location", "nonexistent")
        ), 1));
        input.write(makeRequest("shutdown", null, 2));

        ByteArrayOutputStream output = new ByteArrayOutputStream();
        Serve.run(simpleSource(), new ByteArrayInputStream(input.toByteArray()), output);

        byte[] out = output.toByteArray();
        int newlineIdx = 0;
        while (newlineIdx < out.length && out[newlineIdx] != '\n') newlineIdx++;
        newlineIdx++;

        int length = ByteBuffer.wrap(out, newlineIdx, 4).getInt();
        assertEquals("Expected zero-length frame for no data", 0, length);
    }

    // -- Schema-driven connector tests --

    private static Connector schemaSource() {
        return new Connector() {
            @Override
            public Map<String, String> schema() {
                // Use LinkedHashMap to preserve column order
                Map<String, String> s = new LinkedHashMap<>();
                s.put("name", "string");
                s.put("score", "float32");
                return s;
            }

            @Override
            public List<Location> discover(List<String> attached, Map<String, String> args) {
                return List.of(new Location("col_dict"), new Location("row_dicts"));
            }

            @Override
            public Object data(Location location, Map<String, String> args) {
                if ("col_dict".equals(location.location())) {
                    Map<String, List<?>> cols = new LinkedHashMap<>();
                    cols.put("name", List.of("alice", "bob"));
                    cols.put("score", List.of(9.5, 8.0));
                    return cols;
                } else if ("row_dicts".equals(location.location())) {
                    return List.of(
                        Map.of("name", "charlie", "score", 7.5)
                    );
                }
                return null;
            }
        };
    }

    @Test
    public void testColumnDictWithSchema() throws Exception {
        ByteArrayOutputStream input = new ByteArrayOutputStream();
        input.write(makeRequest("data", Map.of("location", Map.of("location", "col_dict")), 1));
        input.write(makeRequest("shutdown", null, 2));

        ByteArrayOutputStream output = new ByteArrayOutputStream();
        Serve.run(schemaSource(), new ByteArrayInputStream(input.toByteArray()), output);

        byte[] out = output.toByteArray();
        int newlineIdx = 0;
        while (newlineIdx < out.length && out[newlineIdx] != '\n') newlineIdx++;
        newlineIdx++;

        int length = ByteBuffer.wrap(out, newlineIdx, 4).getInt();
        assertTrue("Expected non-zero Arrow IPC data", length > 0);

        BufferAllocator allocator = new RootAllocator();
        byte[] ipcData = Arrays.copyOfRange(out, newlineIdx + 4, newlineIdx + 4 + length);
        ArrowStreamReader reader = new ArrowStreamReader(new ByteArrayInputStream(ipcData), allocator);
        assertTrue(reader.loadNextBatch());

        VectorSchemaRoot root = reader.getVectorSchemaRoot();
        assertEquals(2, root.getRowCount());

        // Verify schema types
        assertEquals(new ArrowType.Utf8(), root.getVector("name").getField().getType());
        assertEquals(
            new ArrowType.FloatingPoint(FloatingPointPrecision.SINGLE),
            root.getVector("score").getField().getType());

        reader.close();
        allocator.close();
    }

    @Test
    public void testRowDictsWithSchema() throws Exception {
        ByteArrayOutputStream input = new ByteArrayOutputStream();
        input.write(makeRequest("data", Map.of("location", Map.of("location", "row_dicts")), 1));
        input.write(makeRequest("shutdown", null, 2));

        ByteArrayOutputStream output = new ByteArrayOutputStream();
        Serve.run(schemaSource(), new ByteArrayInputStream(input.toByteArray()), output);

        byte[] out = output.toByteArray();
        int newlineIdx = 0;
        while (newlineIdx < out.length && out[newlineIdx] != '\n') newlineIdx++;
        newlineIdx++;

        int length = ByteBuffer.wrap(out, newlineIdx, 4).getInt();
        assertTrue("Expected non-zero Arrow IPC data", length > 0);

        BufferAllocator allocator = new RootAllocator();
        byte[] ipcData = Arrays.copyOfRange(out, newlineIdx + 4, newlineIdx + 4 + length);
        ArrowStreamReader reader = new ArrowStreamReader(new ByteArrayInputStream(ipcData), allocator);
        assertTrue(reader.loadNextBatch());

        VectorSchemaRoot root = reader.getVectorSchemaRoot();
        assertEquals(1, root.getRowCount());
        assertEquals(
            new ArrowType.FloatingPoint(FloatingPointPrecision.SINGLE),
            root.getVector("score").getField().getType());

        reader.close();
        allocator.close();
    }

    @Test
    public void testSchemaToArrowUnknownType() {
        try {
            TypeMap.schemaToArrow(Map.of("col", "bigint"));
            fail("Expected IllegalArgumentException");
        } catch (IllegalArgumentException e) {
            assertTrue(e.getMessage().contains("Unknown type 'bigint'"));
        }
    }

    @Test
    public void testDictWithoutSchemaRaises() throws Exception {
        Connector noSchemaSource = new Connector() {
            @Override
            public List<Location> discover(List<String> attached, Map<String, String> args) {
                return List.of(new Location("test"));
            }

            @Override
            public Object data(Location location, Map<String, String> args) {
                return List.of(Map.of("name", "alice"));
            }
        };

        ByteArrayOutputStream input = new ByteArrayOutputStream();
        input.write(makeRequest("data", Map.of("location", Map.of("location", "test")), 1));
        input.write(makeRequest("shutdown", null, 2));

        ByteArrayOutputStream output = new ByteArrayOutputStream();
        Serve.run(noSchemaSource, new ByteArrayInputStream(input.toByteArray()), output);

        // The error is caught by Serve and returned as a JSON-RPC error
        String[] lines = output.toString().split("\n");
        JsonNode resp = MAPPER.readTree(lines[0]);
        assertEquals(-32000, resp.get("error").get("code").asInt());
        assertTrue(resp.get("error").get("message").asText().contains("schema() is required"));
    }

    @Test
    public void testUnknownMethod() throws Exception {
        ByteArrayOutputStream input = new ByteArrayOutputStream();
        input.write(makeRequest("bogus", null, 1));
        input.write(makeRequest("shutdown", null, 2));

        ByteArrayOutputStream output = new ByteArrayOutputStream();
        Serve.run(simpleSource(), new ByteArrayInputStream(input.toByteArray()), output);

        String[] lines = output.toString().split("\n");
        JsonNode resp = MAPPER.readTree(lines[0]);
        assertEquals(-32601, resp.get("error").get("code").asInt());
        assertTrue(resp.get("error").get("message").asText().contains("Method not found"));
    }
}
