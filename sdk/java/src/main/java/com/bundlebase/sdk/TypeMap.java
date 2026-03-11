package com.bundlebase.sdk;

import org.apache.arrow.vector.types.DateUnit;
import org.apache.arrow.vector.types.FloatingPointPrecision;
import org.apache.arrow.vector.types.TimeUnit;
import org.apache.arrow.vector.types.pojo.ArrowType;
import org.apache.arrow.vector.types.pojo.Field;
import org.apache.arrow.vector.types.pojo.Schema;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;

/**
 * Maps type string names to Arrow types for schema-driven conversion.
 */
class TypeMap {
    private static final Map<String, ArrowType> TYPE_MAP = Map.ofEntries(
        Map.entry("Utf8", new ArrowType.Utf8()),
        Map.entry("Int8", new ArrowType.Int(8, true)),
        Map.entry("Int16", new ArrowType.Int(16, true)),
        Map.entry("Int32", new ArrowType.Int(32, true)),
        Map.entry("Int64", new ArrowType.Int(64, true)),
        Map.entry("UInt8", new ArrowType.Int(8, false)),
        Map.entry("UInt16", new ArrowType.Int(16, false)),
        Map.entry("UInt32", new ArrowType.Int(32, false)),
        Map.entry("UInt64", new ArrowType.Int(64, false)),
        Map.entry("Float16", new ArrowType.FloatingPoint(FloatingPointPrecision.HALF)),
        Map.entry("Float32", new ArrowType.FloatingPoint(FloatingPointPrecision.SINGLE)),
        Map.entry("Float64", new ArrowType.FloatingPoint(FloatingPointPrecision.DOUBLE)),
        Map.entry("Boolean", new ArrowType.Bool()),
        Map.entry("Date32", new ArrowType.Date(DateUnit.DAY)),
        Map.entry("Date64", new ArrowType.Date(DateUnit.MILLISECOND)),
        Map.entry("Timestamp", new ArrowType.Timestamp(TimeUnit.MICROSECOND, null)),
        Map.entry("Binary", new ArrowType.Binary())
    );

    /**
     * Convert a schema map of {column_name: type_string} to an Arrow Schema.
     *
     * @throws IllegalArgumentException for unknown type strings
     */
    static Schema schemaToArrow(Map<String, String> schema) {
        List<Field> fields = new ArrayList<>();
        for (var entry : schema.entrySet()) {
            ArrowType type = TYPE_MAP.get(entry.getValue());
            if (type == null) {
                throw new IllegalArgumentException(
                    "Unknown type '" + entry.getValue() + "' for column '" + entry.getKey()
                    + "'. Supported types: " + String.join(", ", TYPE_MAP.keySet().stream().sorted().toList()));
            }
            fields.add(Field.nullable(entry.getKey(), type));
        }
        return new Schema(fields);
    }

    /**
     * Look up the Arrow type for a type string.
     *
     * @throws IllegalArgumentException if the type string is unknown
     */
    static ArrowType resolve(String typeStr) {
        ArrowType type = TYPE_MAP.get(typeStr);
        if (type == null) {
            throw new IllegalArgumentException("Unknown type: " + typeStr);
        }
        return type;
    }
}
