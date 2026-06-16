package com.bundlebase.sdk;

import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * A discovered data location returned from discover().
 *
 * <p>{@code numRows} is the row count for this location, or {@code null} when
 * unknown. Set it when the connector can determine the count cheaply (e.g.
 * Parquet metadata, a row-count manifest) — bundlebase uses it to report
 * expected row deltas without reading the data. The wire protocol requires
 * the {@code num_rows} key on every location; the SDK serializes
 * {@code null} as explicit JSON null.
 */
public record Location(
    String location,
    @JsonProperty("must_copy") boolean mustCopy,
    String format,
    String version,
    @JsonProperty("num_rows") Long numRows
) {
    public Location(String location) {
        this(location, true, "parquet", "", null);
    }

    public Location(String location, boolean mustCopy, String format, String version) {
        this(location, mustCopy, format, version, null);
    }

    public Location(String location, boolean mustCopy, String format, String version, Long numRows) {
        this.location = location;
        this.mustCopy = mustCopy;
        this.format = format != null ? format : "parquet";
        this.version = version != null ? version : "";
        this.numRows = numRows;
    }
}
