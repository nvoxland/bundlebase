package com.bundlebase.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ObjectNode;
import org.apache.arrow.memory.BufferAllocator;
import org.apache.arrow.vector.*;
import org.apache.arrow.vector.ipc.ArrowStreamWriter;
import org.apache.arrow.vector.types.pojo.ArrowType;
import org.apache.arrow.vector.types.pojo.Field;
import org.apache.arrow.vector.types.pojo.Schema;

import java.io.*;
import java.nio.ByteBuffer;
import java.nio.channels.Channels;
import java.util.*;

/**
 * Internal JSON-RPC 2.0 and Arrow IPC protocol handling.
 */
class Protocol {
    private static final ObjectMapper MAPPER = new ObjectMapper();

    static JsonNode readRequest(BufferedReader reader) throws IOException {
        String line = reader.readLine();
        if (line == null || line.isBlank()) {
            return null;
        }
        return MAPPER.readTree(line);
    }

    static void writeResponse(OutputStream out, JsonNode id, Object result) throws IOException {
        ObjectNode resp = MAPPER.createObjectNode();
        resp.put("jsonrpc", "2.0");
        resp.set("id", id);
        resp.set("result", MAPPER.valueToTree(result));
        out.write((MAPPER.writeValueAsString(resp) + "\n").getBytes());
        out.flush();
    }

    static void writeError(OutputStream out, JsonNode id, int code, String message) throws IOException {
        ObjectNode resp = MAPPER.createObjectNode();
        resp.put("jsonrpc", "2.0");
        resp.set("id", id);
        ObjectNode error = MAPPER.createObjectNode();
        error.put("code", code);
        error.put("message", message);
        resp.set("error", error);
        out.write((MAPPER.writeValueAsString(resp) + "\n").getBytes());
        out.flush();
    }

    static void writeArrowIPC(OutputStream out, VectorSchemaRoot root) throws IOException {
        if (root == null || root.getRowCount() == 0) {
            // Zero-length frame
            out.write(ByteBuffer.allocate(4).putInt(0).array());
            out.flush();
            return;
        }

        ByteArrayOutputStream buf = new ByteArrayOutputStream();
        ArrowStreamWriter writer = new ArrowStreamWriter(root, null, Channels.newChannel(buf));
        writer.start();
        writer.writeBatch();
        writer.end();
        writer.close();

        byte[] data = buf.toByteArray();
        out.write(ByteBuffer.allocate(4).putInt(data.length).array());
        out.write(data);
        out.flush();
    }

    static List<String> parseStringList(JsonNode node) {
        if (node == null || !node.isArray()) {
            return Collections.emptyList();
        }
        List<String> result = new ArrayList<>();
        for (JsonNode item : node) {
            if (item.isTextual()) {
                result.add(item.asText());
            }
        }
        return result;
    }

    static Map<String, String> parseStringMap(JsonNode params, String... exclude) {
        Set<String> excl = new HashSet<>(Arrays.asList(exclude));
        Map<String, String> result = new HashMap<>();
        if (params != null && params.isObject()) {
            var it = params.fields();
            while (it.hasNext()) {
                var entry = it.next();
                if (!excl.contains(entry.getKey()) && entry.getValue().isTextual()) {
                    result.put(entry.getKey(), entry.getValue().asText());
                }
            }
        }
        return result;
    }

    static Location parseLocation(JsonNode node) {
        if (node == null || !node.isObject()) {
            return new Location("");
        }
        return new Location(
            node.has("location") ? node.get("location").asText() : "",
            node.has("must_copy") ? node.get("must_copy").asBoolean() : true,
            node.has("format") ? node.get("format").asText() : "parquet",
            node.has("version") ? node.get("version").asText() : ""
        );
    }

    /**
     * Normalize data returned from {@link Connector#data} into a VectorSchemaRoot.
     *
     * Supports VectorSchemaRoot (pass-through), List of Maps (row-oriented),
     * and Map of Lists (column-oriented).
     */
    @SuppressWarnings("unchecked")
    static VectorSchemaRoot normalizeToRoot(
            Object data, Map<String, String> schema, BufferAllocator allocator) {
        if (data == null) {
            return null;
        }
        if (data instanceof VectorSchemaRoot root) {
            return root;
        }
        if (data instanceof List<?> list) {
            if (list.isEmpty()) {
                return null;
            }
            if (list.getFirst() instanceof Map<?, ?>) {
                return rowDictsToRoot((List<Map<String, Object>>) list, schema, allocator);
            }
            throw new IllegalArgumentException(
                "Unsupported list element type: " + list.getFirst().getClass().getName());
        }
        if (data instanceof Map<?, ?> map) {
            return columnDictToRoot((Map<String, List<?>>) map, schema, allocator);
        }
        throw new IllegalArgumentException("Unsupported data return type: " + data.getClass().getName());
    }

    private static VectorSchemaRoot columnDictToRoot(
            Map<String, List<?>> data, Map<String, String> schema, BufferAllocator allocator) {
        if (data.isEmpty()) {
            return null;
        }

        Schema arrowSchema = buildSchema(data.keySet(), schema);
        VectorSchemaRoot root = VectorSchemaRoot.create(arrowSchema, allocator);
        root.allocateNew();

        int rowCount = data.values().iterator().next().size();
        for (Field field : arrowSchema.getFields()) {
            List<?> values = data.get(field.getName());
            if (values == null) continue;
            FieldVector vector = root.getVector(field.getName());
            populateVector(vector, values);
        }

        root.setRowCount(rowCount);
        return root;
    }

    private static VectorSchemaRoot rowDictsToRoot(
            List<Map<String, Object>> rows, Map<String, String> schema, BufferAllocator allocator) {
        if (rows.isEmpty()) {
            return null;
        }

        // Collect column names preserving order from the first row
        Set<String> columnNames = new LinkedHashSet<>(rows.getFirst().keySet());
        for (Map<String, Object> row : rows) {
            columnNames.addAll(row.keySet());
        }

        Schema arrowSchema = buildSchema(columnNames, schema);
        VectorSchemaRoot root = VectorSchemaRoot.create(arrowSchema, allocator);
        root.allocateNew();

        for (Field field : arrowSchema.getFields()) {
            FieldVector vector = root.getVector(field.getName());
            List<Object> values = new ArrayList<>();
            for (Map<String, Object> row : rows) {
                values.add(row.get(field.getName()));
            }
            populateVector(vector, values);
        }

        root.setRowCount(rows.size());
        return root;
    }

    private static Schema buildSchema(Collection<String> columnNames, Map<String, String> schema) {
        if (schema != null) {
            return TypeMap.schemaToArrow(schema);
        }
        throw new IllegalArgumentException(
            "schema() is required when returning dict data. " +
            "Define a schema() method on your Connector.");
    }

    private static void populateVector(FieldVector vector, List<?> values) {
        for (int i = 0; i < values.size(); i++) {
            Object val = values.get(i);
            if (val == null) {
                continue; // leave as null
            }
            switch (vector) {
                case VarCharVector v -> v.setSafe(i, val.toString().getBytes());
                case BigIntVector v -> v.setSafe(i, ((Number) val).longValue());
                case IntVector v -> v.setSafe(i, ((Number) val).intValue());
                case SmallIntVector v -> v.setSafe(i, (short) ((Number) val).intValue());
                case TinyIntVector v -> v.setSafe(i, (byte) ((Number) val).intValue());
                case UInt1Vector v -> v.setSafe(i, (byte) ((Number) val).intValue());
                case UInt2Vector v -> v.setSafe(i, (char) ((Number) val).intValue());
                case UInt4Vector v -> v.setSafe(i, ((Number) val).intValue());
                case UInt8Vector v -> v.setSafe(i, ((Number) val).longValue());
                case Float4Vector v -> v.setSafe(i, ((Number) val).floatValue());
                case Float8Vector v -> v.setSafe(i, ((Number) val).doubleValue());
                case BitVector v -> {
                    boolean b = val instanceof Boolean ? (Boolean) val : Boolean.parseBoolean(val.toString());
                    v.setSafe(i, b ? 1 : 0);
                }
                case VarBinaryVector v -> {
                    if (val instanceof byte[] bytes) {
                        v.setSafe(i, bytes);
                    } else {
                        v.setSafe(i, val.toString().getBytes());
                    }
                }
                default -> throw new IllegalArgumentException(
                    "Unsupported vector type: " + vector.getClass().getName());
            }
        }
    }
}
