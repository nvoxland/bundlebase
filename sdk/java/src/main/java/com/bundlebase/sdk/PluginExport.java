package com.bundlebase.sdk;

import com.fasterxml.jackson.core.type.TypeReference;
import com.fasterxml.jackson.databind.ObjectMapper;
import org.apache.arrow.c.ArrowArrayStream;
import org.apache.arrow.c.Data;
import org.apache.arrow.memory.BufferAllocator;
import org.apache.arrow.memory.RootAllocator;
import org.apache.arrow.vector.VectorSchemaRoot;
import org.apache.arrow.vector.ipc.ArrowReader;

import java.io.IOException;
import java.lang.foreign.*;
import java.lang.invoke.MethodHandle;
import java.lang.invoke.MethodHandles;
import java.lang.invoke.MethodType;
import java.nio.charset.StandardCharsets;
import java.util.*;

/**
 * Panama-based bridge for plugin (shared library) source export.
 *
 * <p>Uses Java's Foreign Function &amp; Memory API (Project Panama, Java 22+)
 * to register upcall stubs for the Bundlebase C ABI. The C bootstrap
 * starts the JVM once, then all subsequent {@code bundlebase_discover},
 * {@code bundlebase_data}, and {@code bundlebase_stable_url} calls route
 * through Panama upcalls — no JNI method dispatch on the hot path.
 *
 * <h2>Usage</h2>
 * <pre>
 * // Register your source once at startup
 * PluginExport.register(new MySource());
 * </pre>
 *
 * <p>Then build the shared library and use from Bundlebase:
 * <pre>
 * bundle.create_source("plugin", {"call": "lib:./my_source.so"})
 * </pre>
 */
public class PluginExport {

    private static final ObjectMapper MAPPER = new ObjectMapper();
    private static final BufferAllocator ALLOCATOR = new RootAllocator();
    private static final Linker LINKER = Linker.nativeLinker();
    private static volatile Connector source;

    // Native memory helpers — allocate via C malloc so bundlebase_free (C free) works
    private static final MethodHandle MALLOC;
    private static final MethodHandle C_FREE;

    static {
        try {
            MALLOC = LINKER.downcallHandle(
                    LINKER.defaultLookup().find("malloc").orElseThrow(),
                    FunctionDescriptor.of(ValueLayout.ADDRESS, ValueLayout.JAVA_LONG));
            C_FREE = LINKER.downcallHandle(
                    LINKER.defaultLookup().find("free").orElseThrow(),
                    FunctionDescriptor.ofVoid(ValueLayout.ADDRESS));
        } catch (Throwable e) {
            throw new ExceptionInInitializerError(e);
        }
    }

    // Prevent GC of upcall stubs (must survive for process lifetime)
    @SuppressWarnings("unused")
    private static MemorySegment discoverStub;
    @SuppressWarnings("unused")
    private static MemorySegment dataStub;
    @SuppressWarnings("unused")
    private static MemorySegment stableUrlStub;

    /**
     * Register the connector for plugin export.
     * Call this in a static initializer before the library is loaded.
     */
    public static void register(Connector src) {
        source = src;
    }

    /**
     * Called once from the C bootstrap to set up Panama upcall stubs.
     *
     * <p>The C shim passes the address of its {@code bundlebase_register_callbacks}
     * function. Java creates upcall stubs and registers them via a Panama downcall.
     * After this, all C ABI calls route through Panama — no JNI method lookups.
     *
     * @param registerAddr address of the C {@code bundlebase_register_callbacks} function
     */
    public static void initialize(long registerAddr) throws Throwable {
        var lookup = MethodHandles.lookup();

        // Create upcall stubs for each ABI function
        // int32_t discover(const char* args_json, char** out_json)
        discoverStub = LINKER.upcallStub(
                lookup.findStatic(PluginExport.class, "nativeDiscover",
                        MethodType.methodType(int.class, MemorySegment.class, MemorySegment.class)),
                FunctionDescriptor.of(ValueLayout.JAVA_INT, ValueLayout.ADDRESS, ValueLayout.ADDRESS),
                Arena.global());

        // int32_t data(const char* location_json, const char* args_json, ArrowArrayStream* out)
        dataStub = LINKER.upcallStub(
                lookup.findStatic(PluginExport.class, "nativeData",
                        MethodType.methodType(int.class,
                                MemorySegment.class, MemorySegment.class, MemorySegment.class)),
                FunctionDescriptor.of(ValueLayout.JAVA_INT,
                        ValueLayout.ADDRESS, ValueLayout.ADDRESS, ValueLayout.ADDRESS),
                Arena.global());

        // int32_t stable_url(const char* location_json, const char* args_json, char** out_json)
        stableUrlStub = LINKER.upcallStub(
                lookup.findStatic(PluginExport.class, "nativeStableUrl",
                        MethodType.methodType(int.class,
                                MemorySegment.class, MemorySegment.class, MemorySegment.class)),
                FunctionDescriptor.of(ValueLayout.JAVA_INT,
                        ValueLayout.ADDRESS, ValueLayout.ADDRESS, ValueLayout.ADDRESS),
                Arena.global());

        // Register callbacks with C shim via Panama downcall
        var registerHandle = LINKER.downcallHandle(
                MemorySegment.ofAddress(registerAddr),
                FunctionDescriptor.ofVoid(
                        ValueLayout.ADDRESS, ValueLayout.ADDRESS, ValueLayout.ADDRESS));
        registerHandle.invoke(discoverStub, dataStub, stableUrlStub);
    }

    // ---- Panama upcall targets (called from C via function pointers) ----

    /**
     * Upcall target for {@code bundlebase_discover}.
     * Signature: {@code int32_t(const char* args_json, char** out_json)}
     */
    public static int nativeDiscover(MemorySegment argsPtr, MemorySegment outPtr) {
        try {
            String argsJson = readCString(argsPtr);
            String result = doDiscover(argsJson);
            writeCStringResult(outPtr, result);
            return 0;
        } catch (Throwable e) {
            writeCStringResultSafe(outPtr,
                    e.getMessage() != null ? e.getMessage() : "Unknown error in discover()");
            return -1;
        }
    }

    /**
     * Upcall target for {@code bundlebase_data}.
     * Signature: {@code int32_t(const char* location_json, const char* args_json, ArrowArrayStream* out)}
     */
    public static int nativeData(MemorySegment locPtr, MemorySegment argsPtr,
                                 MemorySegment streamPtr) {
        try {
            String locationJson = readCString(locPtr);
            String argsJson = readCString(argsPtr);
            doData(locationJson, argsJson, streamPtr.address());
            return 0;
        } catch (Throwable e) {
            return -1;
        }
    }

    /**
     * Upcall target for {@code bundlebase_stable_url}.
     * Signature: {@code int32_t(const char* location_json, const char* args_json, char** out_json)}
     */
    public static int nativeStableUrl(MemorySegment locPtr, MemorySegment argsPtr,
                                      MemorySegment outPtr) {
        try {
            String locationJson = readCString(locPtr);
            String argsJson = readCString(argsPtr);
            String result = doStableUrl(locationJson, argsJson);
            if (result != null) {
                writeCStringResult(outPtr, result);
            }
            return 0;
        } catch (Throwable e) {
            return -1;
        }
    }

    // ---- Business logic ----

    private static String doDiscover(String argsJson) throws Exception {
        if (source == null) {
            throw new IllegalStateException("No source registered. Call PluginExport.register() first.");
        }

        Map<String, Object> raw = MAPPER.readValue(argsJson, new TypeReference<>() {});

        List<String> attached = new ArrayList<>();
        Object attachedObj = raw.get("attached_locations");
        if (attachedObj instanceof List<?> list) {
            for (Object item : list) {
                if (item instanceof String s) {
                    attached.add(s);
                }
            }
        }

        Map<String, String> args = new LinkedHashMap<>();
        for (Map.Entry<String, Object> entry : raw.entrySet()) {
            if ("attached_locations".equals(entry.getKey())) continue;
            if (entry.getValue() instanceof String s) {
                args.put(entry.getKey(), s);
            }
        }

        List<Location> locations = source.discover(attached, args);
        return MAPPER.writeValueAsString(Map.of("locations", locations));
    }

    private static void doData(String locationJson, String argsJson, long streamAddress)
            throws Exception {
        if (source == null) {
            throw new IllegalStateException("No source registered.");
        }

        Location loc = MAPPER.readValue(locationJson, Location.class);

        Map<String, Object> raw = MAPPER.readValue(argsJson, new TypeReference<>() {});
        Map<String, String> args = new LinkedHashMap<>();
        for (Map.Entry<String, Object> entry : raw.entrySet()) {
            if (entry.getValue() instanceof String s) {
                args.put(entry.getKey(), s);
            }
        }

        Object data = source.data(loc, args);
        VectorSchemaRoot root = Protocol.normalizeToRoot(data, source.schema(), ALLOCATOR);
        if (root == null) {
            return; // No data — stream left empty
        }

        try (ArrowArrayStream stream = ArrowArrayStream.wrap(streamAddress)) {
            Data.exportArrayStream(ALLOCATOR, new VectorSchemaRootReader(root), stream);
        }
    }

    private static String doStableUrl(String locationJson, String argsJson) throws Exception {
        if (source == null) {
            return null;
        }

        Location loc = MAPPER.readValue(locationJson, Location.class);

        Map<String, Object> raw = MAPPER.readValue(argsJson, new TypeReference<>() {});
        Map<String, String> args = new LinkedHashMap<>();
        for (Map.Entry<String, Object> entry : raw.entrySet()) {
            if (entry.getValue() instanceof String s) {
                args.put(entry.getKey(), s);
            }
        }

        StableUrl result = source.stableUrl(loc, args);
        if (result == null) {
            return null;
        }

        return MAPPER.writeValueAsString(Map.of("url", result.url()));
    }

    // ---- Panama memory helpers ----

    /** Read a null-terminated C string from native memory. */
    private static String readCString(MemorySegment ptr) {
        return ptr.reinterpret(Long.MAX_VALUE).getString(0);
    }

    /**
     * Allocate a C string via malloc and write its address to a {@code char**} out parameter.
     * The caller (Bundlebase) frees via {@code bundlebase_free} which calls C {@code free()}.
     */
    private static void writeCStringResult(MemorySegment outPtr, String value) throws Throwable {
        byte[] bytes = value.getBytes(StandardCharsets.UTF_8);
        MemorySegment cstr = (MemorySegment) MALLOC.invoke((long) (bytes.length + 1));
        cstr = cstr.reinterpret(bytes.length + 1);
        MemorySegment.copy(bytes, 0, cstr, ValueLayout.JAVA_BYTE, 0, bytes.length);
        cstr.set(ValueLayout.JAVA_BYTE, bytes.length, (byte) 0); // null terminator

        outPtr.reinterpret(ValueLayout.ADDRESS.byteSize())
                .set(ValueLayout.ADDRESS, 0, cstr);
    }

    /** Best-effort write — swallows exceptions from malloc failure. */
    private static void writeCStringResultSafe(MemorySegment outPtr, String value) {
        try {
            writeCStringResult(outPtr, value);
        } catch (Throwable ignored) {
            // Cannot allocate error message — caller sees return code -1
        }
    }

    /**
     * Simple ArrowReader wrapper for a single VectorSchemaRoot.
     */
    private static class VectorSchemaRootReader extends ArrowReader {
        private final VectorSchemaRoot root;
        private boolean consumed;

        VectorSchemaRootReader(VectorSchemaRoot root) {
            super(ALLOCATOR);
            this.root = root;
        }

        @Override
        public boolean loadNextBatch() throws IOException {
            if (consumed) return false;
            consumed = true;
            return true;
        }

        @Override
        public long bytesRead() {
            return 0;
        }

        @Override
        protected org.apache.arrow.vector.types.pojo.Schema readSchema() throws IOException {
            return root.getSchema();
        }

        @Override
        protected void closeReadSource() throws IOException {
            root.close();
        }
    }
}
