package com.bundlebase.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ObjectNode;
import org.apache.arrow.vector.VectorSchemaRoot;
import org.apache.arrow.vector.ipc.ArrowStreamWriter;

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
}
