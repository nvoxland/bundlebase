package com.bundlebase.sdk;

import java.util.List;
import java.util.Map;

/**
 * Interface for implementing a custom Bundlebase connector.
 *
 * <p>Implement {@link #discover} and {@link #data}. Optionally override
 * {@link #schema} and {@link #stableUrl}.
 */
public interface Connector {

    /**
     * Discover available data locations.
     *
     * @param attachedLocations locations already attached to the bundle
     * @param args extra arguments from the source configuration
     * @return list of discovered locations
     */
    List<Location> discover(List<String> attachedLocations, Map<String, String> args);

    /**
     * Return data for the given location.
     *
     * <p>Supported return types:
     * <ul>
     *   <li>{@code VectorSchemaRoot} — Arrow data directly</li>
     *   <li>{@code List<Map<String, Object>>} — row-oriented dicts (auto-converted to Arrow)</li>
     *   <li>{@code Map<String, List<?>>} — column-oriented dict (auto-converted to Arrow)</li>
     *   <li>{@code null} — no data for this location</li>
     * </ul>
     *
     * <p>When returning row-oriented or column-oriented data, a {@link #schema()}
     * is required. Returning dict data without a schema throws
     * {@link IllegalArgumentException}.
     *
     * @param location the location to fetch data for
     * @param args extra arguments from the source configuration
     * @return data in one of the supported types, or null
     */
    Object data(Location location, Map<String, String> args);

    /**
     * Schema for automatic dict-to-Arrow conversion.
     *
     * <p>Return a map of column names to type strings. Required when
     * {@link #data} returns {@code List<Map>} or {@code Map<String, List>};
     * those conversions throw {@link IllegalArgumentException} without it.
     *
     * <p>Supported types: Utf8, Int8, Int16, Int32, Int64, UInt8, UInt16, UInt32, UInt64,
     * Float16, Float32, Float64, Boolean, Date32, Date64, Timestamp, Binary.
     *
     * <p>Example: {@code Map.of("name", "Utf8", "age", "Int32", "score", "Float64")}
     *
     * @return schema map, or null when {@link #data} returns
     *         {@code VectorSchemaRoot} directly
     */
    default Map<String, String> schema() {
        return null;
    }

    /**
     * Return a stable URL for the given location, if available.
     * Default returns null.
     */
    default StableUrl stableUrl(Location location, Map<String, String> args) {
        return null;
    }
}
