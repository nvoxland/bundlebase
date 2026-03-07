package com.bundlebase.sdk;

import org.apache.arrow.vector.VectorSchemaRoot;

import java.util.List;
import java.util.Map;

/**
 * Interface for implementing a custom Bundlebase connector.
 *
 * <p>Implement {@link #discover} and {@link #data}. Optionally override
 * {@link #stableUrl} if your source has stable URLs for data locations.
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
     * Return data for the given location as an Arrow VectorSchemaRoot.
     *
     * @param location the location to fetch data for
     * @param args extra arguments from the source configuration
     * @return data as a VectorSchemaRoot, or null for no data
     */
    VectorSchemaRoot data(Location location, Map<String, String> args);

    /**
     * Return a stable URL for the given location, if available.
     * Default returns null.
     */
    default StableUrl stableUrl(Location location, Map<String, String> args) {
        return null;
    }
}
