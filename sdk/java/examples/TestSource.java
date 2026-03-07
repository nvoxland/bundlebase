package com.bundlebase.sdk.examples;

import com.bundlebase.sdk.*;
import org.apache.arrow.memory.BufferAllocator;
import org.apache.arrow.memory.RootAllocator;
import org.apache.arrow.vector.BigIntVector;
import org.apache.arrow.vector.VarCharVector;
import org.apache.arrow.vector.VectorSchemaRoot;
import org.apache.arrow.vector.types.pojo.ArrowType;
import org.apache.arrow.vector.types.pojo.Field;
import org.apache.arrow.vector.types.pojo.Schema;

import java.util.Arrays;
import java.util.List;
import java.util.Map;

/**
 * Test source function used by Rust integration tests.
 */
public class TestSource implements Connector {
    private final BufferAllocator allocator = new RootAllocator();

    @Override
    public List<Location> discover(List<String> attachedLocations, Map<String, String> args) {
        return List.of(
            new Location("test_file_1.parquet", true, "parquet", "v1"),
            new Location("test_file_2.parquet", true, "parquet", "v1")
        );
    }

    @Override
    public VectorSchemaRoot data(Location location, Map<String, String> args) {
        Schema schema = new Schema(Arrays.asList(
            Field.nullable("id", new ArrowType.Int(64, true)),
            Field.nullable("name", new ArrowType.Utf8())
        ));

        VectorSchemaRoot root = VectorSchemaRoot.create(schema, allocator);

        switch (location.location()) {
            case "test_file_1.parquet" -> {
                root.allocateNew();
                BigIntVector idVec = (BigIntVector) root.getVector("id");
                VarCharVector nameVec = (VarCharVector) root.getVector("name");
                idVec.setSafe(0, 1); nameVec.setSafe(0, "alice".getBytes());
                idVec.setSafe(1, 2); nameVec.setSafe(1, "bob".getBytes());
                idVec.setSafe(2, 3); nameVec.setSafe(2, "charlie".getBytes());
                root.setRowCount(3);
            }
            case "test_file_2.parquet" -> {
                root.allocateNew();
                BigIntVector idVec = (BigIntVector) root.getVector("id");
                VarCharVector nameVec = (VarCharVector) root.getVector("name");
                idVec.setSafe(0, 4); nameVec.setSafe(0, "dave".getBytes());
                idVec.setSafe(1, 5); nameVec.setSafe(1, "eve".getBytes());
                root.setRowCount(2);
            }
            default -> {
                root.close();
                return null;
            }
        }
        return root;
    }

    public static void main(String[] args) {
        Serve.run(new TestSource());
    }
}
