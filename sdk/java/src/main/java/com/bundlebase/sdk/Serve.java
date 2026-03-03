package com.bundlebase.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import org.apache.arrow.memory.BufferAllocator;
import org.apache.arrow.memory.RootAllocator;
import org.apache.arrow.vector.VectorSchemaRoot;

import java.io.*;
import java.util.*;

/**
 * Entry point for running a source function as a JSON-RPC subprocess.
 */
public class Serve {

    /**
     * Run the source function on stdin/stdout.
     */
    public static void run(SourceFunction source) {
        run(source, System.in, System.out);
    }

    /**
     * Run the source function on the given streams (for testing).
     */
    public static void run(SourceFunction source, InputStream in, OutputStream out) {
        BufferedReader reader = new BufferedReader(new InputStreamReader(in));

        try {
            while (true) {
                JsonNode req = Protocol.readRequest(reader);
                if (req == null) {
                    break;
                }

                String method = req.has("method") ? req.get("method").asText() : "";
                JsonNode id = req.get("id");
                JsonNode params = req.has("params") ? req.get("params") : null;

                try {
                    boolean shouldStop = handleRequest(source, method, id, params, out);
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
            SourceFunction source, String method, JsonNode id, JsonNode params, OutputStream out)
            throws IOException {

        switch (method) {
            case "discover" -> handleDiscover(source, id, params, out);
            case "data" -> handleData(source, id, params, out);
            case "stable_url" -> handleStableUrl(source, id, params, out);
            case "shutdown" -> {
                Protocol.writeResponse(out, id, Map.of("ok", true));
                return true;
            }
            default -> Protocol.writeError(out, id, -32601, "Method not found: " + method);
        }
        return false;
    }

    private static void handleDiscover(
            SourceFunction source, JsonNode id, JsonNode params, OutputStream out)
            throws IOException {
        List<String> attached = Protocol.parseStringList(
                params != null ? params.get("attached_locations") : null);
        Map<String, String> args = Protocol.parseStringMap(params, "attached_locations");

        List<Location> locations = source.discover(attached, args);
        List<Map<String, Object>> locList = new ArrayList<>();
        for (Location loc : locations) {
            Map<String, Object> m = new LinkedHashMap<>();
            m.put("location", loc.location());
            m.put("must_copy", loc.mustCopy());
            m.put("format", loc.format());
            m.put("version", loc.version());
            locList.add(m);
        }

        Protocol.writeResponse(out, id, Map.of("locations", locList));
    }

    private static void handleData(
            SourceFunction source, JsonNode id, JsonNode params, OutputStream out)
            throws IOException {
        Location location = Protocol.parseLocation(params != null ? params.get("location") : null);
        Map<String, String> args = Protocol.parseStringMap(params, "location");

        VectorSchemaRoot root = source.data(location, args);

        Protocol.writeResponse(out, id, Map.of("ok", true));
        Protocol.writeArrowIPC(out, root);

        if (root != null) {
            root.close();
        }
    }

    private static void handleStableUrl(
            SourceFunction source, JsonNode id, JsonNode params, OutputStream out)
            throws IOException {
        Location location = Protocol.parseLocation(params != null ? params.get("location") : null);
        Map<String, String> args = Protocol.parseStringMap(params, "location");

        StableUrl result = source.stableUrl(location, args);
        if (result != null) {
            Protocol.writeResponse(out, id, Map.of("url", result.url()));
        } else {
            Protocol.writeResponse(out, id, null);
        }
    }
}
