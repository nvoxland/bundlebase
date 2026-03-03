package com.bundlebase.sdk;

/**
 * A discovered data location returned from discover().
 */
public record Location(
    String location,
    boolean mustCopy,
    String format,
    String version
) {
    public Location(String location) {
        this(location, true, "parquet", "");
    }

    public Location(String location, boolean mustCopy, String format, String version) {
        this.location = location;
        this.mustCopy = mustCopy;
        this.format = format != null ? format : "parquet";
        this.version = version != null ? version : "";
    }
}
