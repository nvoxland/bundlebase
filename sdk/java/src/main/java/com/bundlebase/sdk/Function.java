package com.bundlebase.sdk;

import com.fasterxml.jackson.annotation.JsonProperty;
import org.apache.arrow.vector.FieldVector;
import org.apache.arrow.vector.VectorSchemaRoot;

import java.util.List;
import java.util.Map;

/**
 * Interfaces for implementing custom Bundlebase functions.
 *
 * <p>Functions are either scalar (row-by-row) or aggregate (accumulating state).
 */
public class Function {

    private Function() {} // namespace only

    /**
     * A scalar function that transforms an input record batch into an output column.
     */
    public interface ScalarFunction {
        /**
         * Apply the function to the given input record batch, returning a single output column.
         *
         * @param input input record batch (may be null if no input data)
         * @return the result column
         */
        FieldVector invoke(VectorSchemaRoot input) throws Exception;
    }

    /**
     * An aggregate function that accumulates state across batches and produces a final result.
     *
     * @param <S> the accumulator state type
     */
    public interface AggregateFunction<S> {
        /**
         * Create a new accumulator state.
         */
        S createState() throws Exception;

        /**
         * Add data from an input record batch into the accumulator state.
         *
         * @param state the current accumulator state
         * @param input input record batch (may be null if no input data)
         * @return the updated accumulator state
         */
        S accumulate(S state, VectorSchemaRoot input) throws Exception;

        /**
         * Merge two accumulator states into one.
         *
         * @param stateA the first state (result is stored here)
         * @param stateB the second state (consumed)
         * @return the merged state
         */
        S merge(S stateA, S stateB) throws Exception;

        /**
         * Produce a final scalar result from the accumulator state.
         *
         * @param state the final accumulator state
         * @return the result value (Long, Double, String, or Boolean)
         */
        Object evaluate(S state) throws Exception;
    }

    /**
     * Groups functions together with metadata for discovery.
     */
    public interface FunctionProvider {
        /**
         * Return the available functions. Values must be {@link ScalarFunction}
         * or {@link AggregateFunction}.
         */
        Map<String, Object> functions();

        /**
         * Return function metadata for auto-discovery.
         */
        FunctionManifest metadata();
    }

    /**
     * Describes all functions in a provider for auto-detection.
     */
    public record FunctionManifest(List<FunctionMeta> functions) {}

    /**
     * Describes a single function for auto-detection.
     */
    public record FunctionMeta(
            String name,
            @JsonProperty("input_types") List<String> inputTypes,
            @JsonProperty("return_type") String returnType,
            String kind,
            String symbol
    ) {
        public FunctionMeta(String name, List<String> inputTypes, String returnType, String kind) {
            this(name, inputTypes, returnType, kind, null);
        }
    }
}
