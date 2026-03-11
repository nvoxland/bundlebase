package com.bundlebase.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import org.apache.arrow.memory.BufferAllocator;
import org.apache.arrow.memory.RootAllocator;
import org.apache.arrow.vector.*;
import org.apache.arrow.vector.ipc.ArrowStreamReader;
import org.apache.arrow.vector.ipc.ArrowStreamWriter;
import org.apache.arrow.vector.types.pojo.ArrowType;
import org.apache.arrow.vector.types.pojo.Field;
import org.apache.arrow.vector.types.pojo.Schema;

import java.io.*;
import java.nio.ByteBuffer;
import java.nio.channels.Channels;
import java.util.*;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicLong;

/**
 * Entry point for running a function provider as a JSON-RPC subprocess.
 */
public class FunctionServe {

    private static final ObjectMapper MAPPER = new ObjectMapper();
    private static final BufferAllocator ALLOCATOR = new RootAllocator();

    /** State TTL: 300 seconds. */
    private static final long STATE_TTL_MS = 300_000;
    /** Cleanup interval: 60 seconds. */
    private static final long CLEANUP_INTERVAL_MS = 60_000;

    /**
     * Run the function provider on stdin/stdout.
     *
     * <p>If the first command-line argument is {@code --bundlebase-functions},
     * prints the function manifest as JSON and exits.
     */
    public static void run(Function.FunctionProvider provider, String[] args) {
        if (args != null && args.length > 0 && "--bundlebase-functions".equals(args[0])) {
            try {
                String json = MAPPER.writeValueAsString(provider.metadata());
                System.out.println(json);
            } catch (Exception e) {
                System.err.println("Failed to serialize manifest: " + e.getMessage());
                System.exit(1);
            }
            return;
        }
        run(provider, System.in, System.out);
    }

    /**
     * Run the function provider on the given streams (for testing).
     */
    public static void run(Function.FunctionProvider provider, InputStream in, OutputStream out) {
        BufferedReader reader = new BufferedReader(new InputStreamReader(in));
        StateStore store = new StateStore();
        long lastCleanup = System.currentTimeMillis();

        try {
            String line;
            while ((line = reader.readLine()) != null) {
                if (line.isBlank()) continue;

                // Periodic cleanup of expired aggregate state
                long now = System.currentTimeMillis();
                if (now - lastCleanup >= CLEANUP_INTERVAL_MS) {
                    store.cleanup(STATE_TTL_MS);
                    lastCleanup = now;
                }

                JsonNode req;
                try {
                    req = MAPPER.readTree(line);
                } catch (Exception e) {
                    Protocol.writeError(out, null, -32700, "Parse error: " + e.getMessage());
                    continue;
                }

                String method = req.has("method") ? req.get("method").asText() : "";
                JsonNode id = req.get("id");
                JsonNode params = req.has("params") ? req.get("params") : null;

                try {
                    boolean shouldStop = handleRequest(provider, store, method, id, params, in, out);
                    if (shouldStop) {
                        break;
                    }
                } catch (Exception e) {
                    Protocol.writeError(out, id, -32000, e.getMessage());
                }
            }
        } catch (IOException e) {
            System.err.println("Error reading from stdin: " + e.getMessage());
        }
    }

    private static boolean handleRequest(
            Function.FunctionProvider provider, StateStore store,
            String method, JsonNode id, JsonNode params,
            InputStream in, OutputStream out) throws IOException {

        switch (method) {
            case "handshake" -> Protocol.writeResponse(out, id, Map.of("protocol_version", "1"));
            case "ping" -> Protocol.writeResponse(out, id, "pong");
            case "manifest" -> Protocol.writeResponse(out, id, provider.metadata());
            case "invoke" -> handleInvoke(provider, id, params, in, out);
            case "create_state" -> handleCreateState(provider, store, id, params, out);
            case "accumulate" -> handleAccumulate(provider, store, id, params, in, out);
            case "merge" -> handleMerge(provider, store, id, params, out);
            case "evaluate" -> handleEvaluate(provider, store, id, params, out);
            case "shutdown" -> {
                Protocol.writeResponse(out, id, Map.of("ok", true));
                return true;
            }
            default -> Protocol.writeError(out, id, -32601, "Method not found: " + method);
        }
        return false;
    }

    private static void handleInvoke(
            Function.FunctionProvider provider, JsonNode id, JsonNode params,
            InputStream in, OutputStream out) throws IOException {
        String funcName = params != null && params.has("function") ? params.get("function").asText() : "";
        Object fn = provider.functions().get(funcName);
        if (fn == null) {
            Protocol.writeError(out, id, -32000, "Function not found: " + funcName);
            return;
        }
        if (!(fn instanceof Function.ScalarFunction scalar)) {
            Protocol.writeError(out, id, -32000,
                    "Function '" + funcName + "' is not a scalar function (actual type: " + fn.getClass().getName() + ")");
            return;
        }

        // Read Arrow IPC input
        VectorSchemaRoot inputRoot = readArrowIPC(in);
        List<FieldVector> args = new ArrayList<>();
        if (inputRoot != null) {
            args.addAll(inputRoot.getFieldVectors());
        }

        FieldVector result;
        try {
            result = scalar.invoke(args);
        } catch (Exception e) {
            Protocol.writeError(out, id, -32000, e.getMessage());
            if (inputRoot != null) inputRoot.close();
            return;
        }

        // Build a single-column VectorSchemaRoot from the result
        Schema schema = new Schema(List.of(result.getField()));
        VectorSchemaRoot resultRoot = new VectorSchemaRoot(List.of(result.getField()), List.of(result), result.getValueCount());

        // Buffer Arrow IPC before sending ack
        ByteArrayOutputStream arrowBuf = new ByteArrayOutputStream();
        try {
            Protocol.writeArrowIPC(arrowBuf, resultRoot);
        } catch (IOException e) {
            Protocol.writeError(out, id, -32000, "Failed to serialize Arrow IPC result: " + e.getMessage());
            resultRoot.close();
            if (inputRoot != null) inputRoot.close();
            return;
        }

        Protocol.writeResponse(out, id, Map.of("ok", true));
        out.write(arrowBuf.toByteArray());
        out.flush();

        if (inputRoot != null) inputRoot.close();
    }

    @SuppressWarnings("unchecked")
    private static void handleCreateState(
            Function.FunctionProvider provider, StateStore store,
            JsonNode id, JsonNode params, OutputStream out) throws IOException {
        String funcName = params != null && params.has("function") ? params.get("function").asText() : "";
        Object fn = provider.functions().get(funcName);
        if (fn == null) {
            Protocol.writeError(out, id, -32000, "Function not found: " + funcName);
            return;
        }
        if (!(fn instanceof Function.AggregateFunction<?> agg)) {
            Protocol.writeError(out, id, -32000,
                    "Function '" + funcName + "' is not an aggregate function (actual type: " + fn.getClass().getName() + ")");
            return;
        }

        Object state;
        try {
            state = agg.createState();
        } catch (Exception e) {
            Protocol.writeError(out, id, -32000, e.getMessage());
            return;
        }

        String stateId = store.add(state);
        Protocol.writeResponse(out, id, Map.of("state_id", stateId));
    }

    @SuppressWarnings("unchecked")
    private static void handleAccumulate(
            Function.FunctionProvider provider, StateStore store,
            JsonNode id, JsonNode params, InputStream in, OutputStream out) throws IOException {
        String funcName = params != null && params.has("function") ? params.get("function").asText() : "";
        String stateId = params != null && params.has("state_id") ? params.get("state_id").asText() : "";

        Object fn = provider.functions().get(funcName);
        if (fn == null) {
            Protocol.writeError(out, id, -32000, "Function not found: " + funcName);
            return;
        }
        if (!(fn instanceof Function.AggregateFunction agg)) {
            Protocol.writeError(out, id, -32000,
                    "Function '" + funcName + "' is not an aggregate function (actual type: " + fn.getClass().getName() + ")");
            return;
        }

        Object state = store.get(stateId);
        if (state == null) {
            Protocol.writeError(out, id, -32000, "State not found: " + stateId);
            return;
        }

        // Read Arrow IPC input
        VectorSchemaRoot inputRoot = readArrowIPC(in);
        List<FieldVector> args = new ArrayList<>();
        if (inputRoot != null) {
            args.addAll(inputRoot.getFieldVectors());
        }

        Object newState;
        try {
            newState = agg.accumulate(state, args);
        } catch (Exception e) {
            Protocol.writeError(out, id, -32000, e.getMessage());
            if (inputRoot != null) inputRoot.close();
            return;
        }

        store.set(stateId, newState);
        Protocol.writeResponse(out, id, Map.of("ok", true));
        if (inputRoot != null) inputRoot.close();
    }

    @SuppressWarnings("unchecked")
    private static void handleMerge(
            Function.FunctionProvider provider, StateStore store,
            JsonNode id, JsonNode params, OutputStream out) throws IOException {
        String funcName = params != null && params.has("function") ? params.get("function").asText() : "";
        String stateIdA = params != null && params.has("state_id_a") ? params.get("state_id_a").asText() : "";
        String stateIdB = params != null && params.has("state_id_b") ? params.get("state_id_b").asText() : "";

        Object fn = provider.functions().get(funcName);
        if (fn == null) {
            Protocol.writeError(out, id, -32000, "Function not found: " + funcName);
            return;
        }
        if (!(fn instanceof Function.AggregateFunction agg)) {
            Protocol.writeError(out, id, -32000,
                    "Function '" + funcName + "' is not an aggregate function (actual type: " + fn.getClass().getName() + ")");
            return;
        }

        Object stateA = store.get(stateIdA);
        if (stateA == null) {
            Protocol.writeError(out, id, -32000, "State not found: " + stateIdA);
            return;
        }
        Object stateB = store.get(stateIdB);
        if (stateB == null) {
            Protocol.writeError(out, id, -32000, "State not found: " + stateIdB);
            return;
        }

        Object merged;
        try {
            merged = agg.merge(stateA, stateB);
        } catch (Exception e) {
            Protocol.writeError(out, id, -32000, e.getMessage());
            return;
        }

        store.set(stateIdA, merged);
        store.remove(stateIdB);
        Protocol.writeResponse(out, id, Map.of("ok", true));
    }

    @SuppressWarnings("unchecked")
    private static void handleEvaluate(
            Function.FunctionProvider provider, StateStore store,
            JsonNode id, JsonNode params, OutputStream out) throws IOException {
        String funcName = params != null && params.has("function") ? params.get("function").asText() : "";
        String stateId = params != null && params.has("state_id") ? params.get("state_id").asText() : "";

        Object fn = provider.functions().get(funcName);
        if (fn == null) {
            Protocol.writeError(out, id, -32000, "Function not found: " + funcName);
            return;
        }
        if (!(fn instanceof Function.AggregateFunction agg)) {
            Protocol.writeError(out, id, -32000,
                    "Function '" + funcName + "' is not an aggregate function (actual type: " + fn.getClass().getName() + ")");
            return;
        }

        Object state = store.get(stateId);
        if (state == null) {
            Protocol.writeError(out, id, -32000, "State not found: " + stateId);
            return;
        }

        Object result;
        try {
            result = agg.evaluate(state);
        } catch (Exception e) {
            Protocol.writeError(out, id, -32000, e.getMessage());
            return;
        }

        // Convert the scalar result to a single-element Arrow vector
        VectorSchemaRoot resultRoot = scalarToRoot(result);
        if (resultRoot == null) {
            Protocol.writeError(out, id, -32000, "Unsupported evaluate result type: " + result.getClass().getName());
            return;
        }

        // Buffer Arrow IPC before sending ack
        ByteArrayOutputStream arrowBuf = new ByteArrayOutputStream();
        try {
            Protocol.writeArrowIPC(arrowBuf, resultRoot);
        } catch (IOException e) {
            Protocol.writeError(out, id, -32000, "Failed to serialize Arrow IPC result: " + e.getMessage());
            resultRoot.close();
            return;
        }

        Protocol.writeResponse(out, id, Map.of("ok", true));
        out.write(arrowBuf.toByteArray());
        out.flush();

        resultRoot.close();
        store.remove(stateId);
    }

    /**
     * Read a length-prefixed Arrow IPC stream from the input.
     */
    private static VectorSchemaRoot readArrowIPC(InputStream in) throws IOException {
        byte[] lengthBuf = new byte[4];
        int read = in.read(lengthBuf);
        if (read < 4) {
            return null;
        }
        int length = ByteBuffer.wrap(lengthBuf).getInt();
        if (length == 0) {
            return null;
        }

        byte[] data = in.readNBytes(length);
        if (data.length < length) {
            throw new IOException("Unexpected end of Arrow IPC data");
        }

        BufferAllocator allocator = new RootAllocator();
        ArrowStreamReader reader = new ArrowStreamReader(new ByteArrayInputStream(data), allocator);
        if (!reader.loadNextBatch()) {
            reader.close();
            return null;
        }

        // Transfer ownership of vectors so we can close the reader
        VectorSchemaRoot root = reader.getVectorSchemaRoot();
        // We need to keep the root alive, so we don't close the reader here.
        // The caller is responsible for closing the root.
        return root;
    }

    /**
     * Convert a scalar value to a single-row VectorSchemaRoot.
     */
    private static VectorSchemaRoot scalarToRoot(Object value) {
        BufferAllocator allocator = new RootAllocator();

        if (value instanceof Long l) {
            Schema schema = new Schema(List.of(Field.nullable("result", new ArrowType.Int(64, true))));
            VectorSchemaRoot root = VectorSchemaRoot.create(schema, allocator);
            root.allocateNew();
            ((BigIntVector) root.getVector("result")).setSafe(0, l);
            root.setRowCount(1);
            return root;
        } else if (value instanceof Integer i) {
            Schema schema = new Schema(List.of(Field.nullable("result", new ArrowType.Int(64, true))));
            VectorSchemaRoot root = VectorSchemaRoot.create(schema, allocator);
            root.allocateNew();
            ((BigIntVector) root.getVector("result")).setSafe(0, i.longValue());
            root.setRowCount(1);
            return root;
        } else if (value instanceof Double d) {
            Schema schema = new Schema(List.of(Field.nullable("result", new ArrowType.FloatingPoint(
                    org.apache.arrow.vector.types.FloatingPointPrecision.DOUBLE))));
            VectorSchemaRoot root = VectorSchemaRoot.create(schema, allocator);
            root.allocateNew();
            ((Float8Vector) root.getVector("result")).setSafe(0, d);
            root.setRowCount(1);
            return root;
        } else if (value instanceof String s) {
            Schema schema = new Schema(List.of(Field.nullable("result", new ArrowType.Utf8())));
            VectorSchemaRoot root = VectorSchemaRoot.create(schema, allocator);
            root.allocateNew();
            ((VarCharVector) root.getVector("result")).setSafe(0, s.getBytes());
            root.setRowCount(1);
            return root;
        } else if (value instanceof Boolean b) {
            Schema schema = new Schema(List.of(Field.nullable("result", new ArrowType.Bool())));
            VectorSchemaRoot root = VectorSchemaRoot.create(schema, allocator);
            root.allocateNew();
            ((BitVector) root.getVector("result")).setSafe(0, b ? 1 : 0);
            root.setRowCount(1);
            return root;
        }

        return null;
    }

    /**
     * Thread-safe state store for aggregate function accumulators.
     */
    static class StateStore {
        private final ConcurrentHashMap<String, Object> states = new ConcurrentHashMap<>();
        private final ConcurrentHashMap<String, Long> createdAt = new ConcurrentHashMap<>();
        private final AtomicLong nextId = new AtomicLong(0);

        String add(Object state) {
            long id = nextId.incrementAndGet();
            String key = "state_" + id;
            states.put(key, state);
            createdAt.put(key, System.currentTimeMillis());
            return key;
        }

        Object get(String id) {
            return states.get(id);
        }

        void set(String id, Object state) {
            states.put(id, state);
        }

        void remove(String id) {
            states.remove(id);
            createdAt.remove(id);
        }

        void cleanup(long ttlMs) {
            long now = System.currentTimeMillis();
            for (Map.Entry<String, Long> entry : createdAt.entrySet()) {
                if (now - entry.getValue() > ttlMs) {
                    states.remove(entry.getKey());
                    createdAt.remove(entry.getKey());
                }
            }
        }
    }
}
