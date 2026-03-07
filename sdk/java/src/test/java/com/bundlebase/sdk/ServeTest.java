package com.bundlebase.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import org.apache.arrow.memory.BufferAllocator;
import org.apache.arrow.memory.RootAllocator;
import org.apache.arrow.vector.BigIntVector;
import org.apache.arrow.vector.VectorSchemaRoot;
import org.apache.arrow.vector.ipc.ArrowStreamReader;
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
