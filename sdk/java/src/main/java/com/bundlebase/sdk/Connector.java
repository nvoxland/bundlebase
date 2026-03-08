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
     * <p>When returning row-oriented or column-oriented data, define a {@link #schema()}
     * for explicit type control. Without a schema, types are inferred as strings.
     *
     * @param location the location to fetch data for
     * @param args extra arguments from the source configuration
     * @return data in one of the supported types, or null
     */
    Object data(Location location, Map<String, String> args);

    /**
     * Optional schema for automatic dict-to-Arrow conversion.
     *
     * <p>Return a map of column names to type strings. When {@link #data} returns
     * {@code List<Map>} or {@code Map<String, List>}, this schema controls
     * the Arrow types used.
     *
     * <p>Supported types: string, int8, int16, int32, int64, uint8, uint16, uint32, uint64,
     * float16, float32, float64, float, double, int, bool, boolean, date32, date64, date,
     * timestamp, binary, bytes.
     *
     * <p>Example: {@code Map.of("name", "string", "age", "int32", "score", "float64")}
     *
     * @return schema map, or null to use type inference
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
