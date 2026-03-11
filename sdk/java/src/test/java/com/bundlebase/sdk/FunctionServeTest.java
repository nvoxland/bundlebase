package com.bundlebase.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import org.apache.arrow.memory.BufferAllocator;
import org.apache.arrow.memory.RootAllocator;
import org.apache.arrow.vector.BigIntVector;
import org.apache.arrow.vector.FieldVector;
import org.apache.arrow.vector.VectorSchemaRoot;
import org.apache.arrow.vector.ipc.ArrowStreamReader;
import org.apache.arrow.vector.ipc.ArrowStreamWriter;
import org.apache.arrow.vector.types.pojo.ArrowType;
import org.apache.arrow.vector.types.pojo.Field;
import org.apache.arrow.vector.types.pojo.Schema;
import org.junit.Test;

import java.io.*;
import java.nio.ByteBuffer;
import java.nio.channels.Channels;
import java.util.*;

import static org.junit.Assert.*;

public class FunctionServeTest {

    private static final ObjectMapper MAPPER = new ObjectMapper();
    private static final BufferAllocator ALLOCATOR = new RootAllocator();

    // -- Test scalar function: doubles int64 values --

    static class DoubleVal implements Function.ScalarFunction {
        @Override
        public FieldVector invoke(VectorSchemaRoot input) {
            BigIntVector col = (BigIntVector) input.getVector(0);
            BigIntVector result = new BigIntVector("result", new RootAllocator());
            result.allocateNew(col.getValueCount());
            for (int i = 0; i < col.getValueCount(); i++) {
                result.setSafe(i, col.get(i) * 2);
            }
            result.setValueCount(col.getValueCount());
            return result;
        }
    }

    // -- Test aggregate function: sums int64 values --

    static class SumAgg implements Function.AggregateFunction<Long> {
        @Override
        public Long createState() {
            return 0L;
        }

        @Override
        public Long accumulate(Long state, VectorSchemaRoot input) {
            BigIntVector col = (BigIntVector) input.getVector(0);
            long sum = state;
            for (int i = 0; i < col.getValueCount(); i++) {
                sum += col.get(i);
            }
            return sum;
        }

        @Override
        public Long merge(Long stateA, Long stateB) {
            return stateA + stateB;
        }

        @Override
        public Object evaluate(Long state) {
            return state;
        }
    }

    // -- Test provider --

    static class TestProvider implements Function.FunctionProvider {
        @Override
        public Map<String, Object> functions() {
            Map<String, Object> fns = new LinkedHashMap<>();
            fns.put("double_val", new DoubleVal());
            fns.put("my_sum", new SumAgg());
            return fns;
        }

        @Override
        public Function.FunctionManifest metadata() {
            return new Function.FunctionManifest(List.of(
                    new Function.FunctionMeta("double_val", List.of("Int64"), "Int64", "scalar"),
                    new Function.FunctionMeta("my_sum", List.of("Int64"), "Int64", "aggregate")
            ));
        }
    }

    // -- Helpers --

    private static byte[] makeRequest(String method, Map<String, Object> params, int id) throws Exception {
        Map<String, Object> req = new LinkedHashMap<>();
        req.put("jsonrpc", "2.0");
        req.put("id", id);
        req.put("method", method);
        req.put("params", params != null ? params : Map.of());
        return (MAPPER.writeValueAsString(req) + "\n").getBytes();
    }

    private static byte[] buildArrowIPC(long[] values) throws Exception {
        BufferAllocator allocator = new RootAllocator();
        Schema schema = new Schema(List.of(
                Field.nullable("col0", new ArrowType.Int(64, true))
        ));
        VectorSchemaRoot root = VectorSchemaRoot.create(schema, allocator);
        root.allocateNew();
        BigIntVector vec = (BigIntVector) root.getVector("col0");
        for (int i = 0; i < values.length; i++) {
            vec.setSafe(i, values[i]);
        }
        root.setRowCount(values.length);

        ByteArrayOutputStream ipcBuf = new ByteArrayOutputStream();
        ArrowStreamWriter writer = new ArrowStreamWriter(root, null, Channels.newChannel(ipcBuf));
        writer.start();
        writer.writeBatch();
        writer.end();
        writer.close();
        root.close();

        byte[] ipcBytes = ipcBuf.toByteArray();
        ByteArrayOutputStream result = new ByteArrayOutputStream();
        result.write(ByteBuffer.allocate(4).putInt(ipcBytes.length).array());
        result.write(ipcBytes);
        return result.toByteArray();
    }

    // -- Tests --

    @Test
    public void testManifest() throws Exception {
        ByteArrayOutputStream input = new ByteArrayOutputStream();
        input.write(makeRequest("manifest", null, 1));
        input.write(makeRequest("shutdown", null, 2));

        ByteArrayOutputStream output = new ByteArrayOutputStream();
        FunctionServe.run(new TestProvider(), new ByteArrayInputStream(input.toByteArray()), output);

        String[] lines = output.toString().split("\n");
        JsonNode resp = MAPPER.readTree(lines[0]);
        assertEquals(1, resp.get("id").asInt());
        JsonNode result = resp.get("result");
        JsonNode funcs = result.get("functions");
        assertEquals(2, funcs.size());
        assertEquals("double_val", funcs.get(0).get("name").asText());
        assertEquals("my_sum", funcs.get(1).get("name").asText());
    }

    @Test
    public void testInvokeScalar() throws Exception {
        byte[] ipcData = buildArrowIPC(new long[]{1, 2, 3});

        ByteArrayOutputStream input = new ByteArrayOutputStream();
        input.write(makeRequest("invoke", Map.of("function", "double_val"), 1));
        input.write(ipcData);
        input.write(makeRequest("shutdown", null, 2));

        ByteArrayOutputStream output = new ByteArrayOutputStream();
        FunctionServe.run(new TestProvider(), new ByteArrayInputStream(input.toByteArray()), output);

        byte[] out = output.toByteArray();
        // Find the end of the first JSON line
        int newlineIdx = 0;
        while (newlineIdx < out.length && out[newlineIdx] != '\n') newlineIdx++;
        newlineIdx++;

        JsonNode resp = MAPPER.readTree(new String(out, 0, newlineIdx - 1));
        assertTrue(resp.get("result").get("ok").asBoolean());

        // Read length prefix
        int length = ByteBuffer.wrap(out, newlineIdx, 4).getInt();
        assertTrue("Expected non-zero Arrow IPC data", length > 0);

        // Verify Arrow IPC result
        byte[] resultIpc = Arrays.copyOfRange(out, newlineIdx + 4, newlineIdx + 4 + length);
        BufferAllocator allocator = new RootAllocator();
        ArrowStreamReader reader = new ArrowStreamReader(new ByteArrayInputStream(resultIpc), allocator);
        assertTrue(reader.loadNextBatch());
        VectorSchemaRoot root = reader.getVectorSchemaRoot();
        assertEquals(3, root.getRowCount());

        BigIntVector resultVec = (BigIntVector) root.getVector("result");
        assertEquals(2, resultVec.get(0));
        assertEquals(4, resultVec.get(1));
        assertEquals(6, resultVec.get(2));

        reader.close();
        allocator.close();
    }

    @Test
    public void testAggregateWorkflow() throws Exception {
        byte[] ipcData = buildArrowIPC(new long[]{10, 20, 30});

        ByteArrayOutputStream input = new ByteArrayOutputStream();
        // 1. Create state
        input.write(makeRequest("create_state", Map.of("function", "my_sum"), 1));
        // 2. Accumulate
        input.write(makeRequest("accumulate", Map.of("function", "my_sum", "state_id", "state_1"), 2));
        input.write(ipcData);
        // 3. Evaluate
        input.write(makeRequest("evaluate", Map.of("function", "my_sum", "state_id", "state_1"), 3));
        input.write(makeRequest("shutdown", null, 4));

        ByteArrayOutputStream output = new ByteArrayOutputStream();
        FunctionServe.run(new TestProvider(), new ByteArrayInputStream(input.toByteArray()), output);

        byte[] out = output.toByteArray();
        String[] lines = output.toString().split("\n");

        // Response 1: create_state
        JsonNode resp1 = MAPPER.readTree(lines[0]);
        String stateId = resp1.get("result").get("state_id").asText();
        assertNotNull(stateId);
        assertFalse(stateId.isEmpty());

        // Response 2: accumulate
        JsonNode resp2 = MAPPER.readTree(lines[1]);
        assertTrue(resp2.get("result").get("ok").asBoolean());

        // Response 3: evaluate
        JsonNode resp3 = MAPPER.readTree(lines[2]);
        assertTrue(resp3.get("result").get("ok").asBoolean());

        // Read the Arrow IPC result after the third JSON line
        // Find the byte offset after the third newline
        int offset = 0;
        int newlineCount = 0;
        while (offset < out.length && newlineCount < 3) {
            if (out[offset] == '\n') newlineCount++;
            offset++;
        }

        int length = ByteBuffer.wrap(out, offset, 4).getInt();
        assertTrue("Expected non-zero Arrow IPC data for evaluate", length > 0);

        byte[] resultIpc = Arrays.copyOfRange(out, offset + 4, offset + 4 + length);
        BufferAllocator allocator = new RootAllocator();
        ArrowStreamReader reader = new ArrowStreamReader(new ByteArrayInputStream(resultIpc), allocator);
        assertTrue(reader.loadNextBatch());
        VectorSchemaRoot root = reader.getVectorSchemaRoot();
        assertEquals(1, root.getRowCount());

        BigIntVector resultVec = (BigIntVector) root.getVector("result");
        assertEquals(60, resultVec.get(0));  // 10 + 20 + 30

        reader.close();
        allocator.close();
    }

    @Test
    public void testMerge() throws Exception {
        byte[] ipcData1 = buildArrowIPC(new long[]{10, 20});
        byte[] ipcData2 = buildArrowIPC(new long[]{30, 40});

        ByteArrayOutputStream input = new ByteArrayOutputStream();
        // Create two states
        input.write(makeRequest("create_state", Map.of("function", "my_sum"), 1));
        input.write(makeRequest("create_state", Map.of("function", "my_sum"), 2));
        // Accumulate into each
        input.write(makeRequest("accumulate", Map.of("function", "my_sum", "state_id", "state_1"), 3));
        input.write(ipcData1);
        input.write(makeRequest("accumulate", Map.of("function", "my_sum", "state_id", "state_2"), 4));
        input.write(ipcData2);
        // Merge state_2 into state_1
        input.write(makeRequest("merge", Map.of("function", "my_sum", "state_id1", "state_1", "state_id2", "state_2"), 5));
        // Evaluate merged state
        input.write(makeRequest("evaluate", Map.of("function", "my_sum", "state_id", "state_1"), 6));
        input.write(makeRequest("shutdown", null, 7));

        ByteArrayOutputStream output = new ByteArrayOutputStream();
        FunctionServe.run(new TestProvider(), new ByteArrayInputStream(input.toByteArray()), output);

        byte[] out = output.toByteArray();
        String[] lines = output.toString().split("\n");

        // Responses 1-4 should all succeed
        for (int i = 0; i < 4; i++) {
            JsonNode resp = MAPPER.readTree(lines[i]);
            assertNull("Response " + i + " should not have error: " + lines[i], resp.get("error"));
        }

        // Response 5: merge should return state_id
        JsonNode mergeResp = MAPPER.readTree(lines[4]);
        assertNull("Merge should not have error: " + lines[4], mergeResp.get("error"));
        assertEquals("state_1", mergeResp.get("result").get("state_id").asText());

        // Response 6: evaluate (after merge)
        JsonNode evalResp = MAPPER.readTree(lines[5]);
        assertTrue(evalResp.get("result").get("ok").asBoolean());

        // Read the Arrow IPC result
        int offset = 0;
        int newlineCount = 0;
        while (offset < out.length && newlineCount < 6) {
            if (out[offset] == '\n') newlineCount++;
            offset++;
        }

        int length = ByteBuffer.wrap(out, offset, 4).getInt();
        assertTrue(length > 0);

        byte[] resultIpc = Arrays.copyOfRange(out, offset + 4, offset + 4 + length);
        BufferAllocator allocator = new RootAllocator();
        ArrowStreamReader reader = new ArrowStreamReader(new ByteArrayInputStream(resultIpc), allocator);
        assertTrue(reader.loadNextBatch());

        BigIntVector resultVec = (BigIntVector) reader.getVectorSchemaRoot().getVector("result");
        assertEquals(100, resultVec.get(0));  // 10 + 20 + 30 + 40

        reader.close();
        allocator.close();
    }

    @Test
    public void testHandshake() throws Exception {
        ByteArrayOutputStream input = new ByteArrayOutputStream();
        input.write(makeRequest("handshake", null, 1));
        input.write(makeRequest("shutdown", null, 2));

        ByteArrayOutputStream output = new ByteArrayOutputStream();
        FunctionServe.run(new TestProvider(), new ByteArrayInputStream(input.toByteArray()), output);

        String[] lines = output.toString().split("\n");
        JsonNode resp = MAPPER.readTree(lines[0]);
        assertEquals("1", resp.get("result").get("protocol_version").asText());
    }

    @Test
    public void testUnknownMethod() throws Exception {
        ByteArrayOutputStream input = new ByteArrayOutputStream();
        input.write(makeRequest("bogus", null, 1));
        input.write(makeRequest("shutdown", null, 2));

        ByteArrayOutputStream output = new ByteArrayOutputStream();
        FunctionServe.run(new TestProvider(), new ByteArrayInputStream(input.toByteArray()), output);

        String[] lines = output.toString().split("\n");
        JsonNode resp = MAPPER.readTree(lines[0]);
        assertEquals(-32601, resp.get("error").get("code").asInt());
        assertTrue(resp.get("error").get("message").asText().contains("Method not found"));
    }

    @Test
    public void testFunctionNotFound() throws Exception {
        ByteArrayOutputStream input = new ByteArrayOutputStream();
        input.write(makeRequest("invoke", Map.of("function", "nonexistent"), 1));
        input.write(makeRequest("shutdown", null, 2));

        ByteArrayOutputStream output = new ByteArrayOutputStream();
        FunctionServe.run(new TestProvider(), new ByteArrayInputStream(input.toByteArray()), output);

        String[] lines = output.toString().split("\n");
        JsonNode resp = MAPPER.readTree(lines[0]);
        assertEquals(-32000, resp.get("error").get("code").asInt());
        assertTrue(resp.get("error").get("message").asText().contains("Function not found"));
    }

    @Test
    public void testStateNotFound() throws Exception {
        ByteArrayOutputStream input = new ByteArrayOutputStream();
        input.write(makeRequest("evaluate", Map.of("function", "my_sum", "state_id", "nonexistent"), 1));
        input.write(makeRequest("shutdown", null, 2));

        ByteArrayOutputStream output = new ByteArrayOutputStream();
        FunctionServe.run(new TestProvider(), new ByteArrayInputStream(input.toByteArray()), output);

        String[] lines = output.toString().split("\n");
        JsonNode resp = MAPPER.readTree(lines[0]);
        assertEquals(-32000, resp.get("error").get("code").asInt());
        assertTrue(resp.get("error").get("message").asText().contains("State not found"));
    }

    @Test
    public void testStateStoreCleanup() throws Exception {
        FunctionServe.StateStore store = new FunctionServe.StateStore();
        String id = store.add("test_state");
        assertNotNull(store.get(id));

        // Wait a moment so the TTL check can expire with a very short TTL
        Thread.sleep(10);
        // Cleanup with 1ms TTL should remove entries older than 1ms
        store.cleanup(1);
        assertNull(store.get(id));
    }
}
