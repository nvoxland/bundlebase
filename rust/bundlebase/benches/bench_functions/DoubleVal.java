// Benchmark IPC function server for Java. Implements double_val (scalar) and int_sum (aggregate).

import com.bundlebase.sdk.Function;
import com.bundlebase.sdk.FunctionServe;
import org.apache.arrow.memory.RootAllocator;
import org.apache.arrow.vector.BigIntVector;
import org.apache.arrow.vector.FieldVector;
import org.apache.arrow.vector.VectorSchemaRoot;

import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

public class DoubleVal {

    static class DoubleValFn implements Function.ScalarFunction {
        @Override
        public FieldVector invoke(VectorSchemaRoot input) {
            BigIntVector col = (BigIntVector) input.getVector(0);
            BigIntVector result = new BigIntVector("result", new RootAllocator());
            result.allocateNew(col.getValueCount());
            for (int i = 0; i < col.getValueCount(); i++) {
                result.setSafe(i, col.get(i) * 2);
            }
            result.setValueCount(col.getValueCount());
            return result;
        }
    }

    static class IntSumFn implements Function.AggregateFunction<Long> {
        @Override
        public Long createState() {
            return 0L;
        }

        @Override
        public Long accumulate(Long state, VectorSchemaRoot input) {
            BigIntVector col = (BigIntVector) input.getVector(0);
            long sum = state;
            for (int i = 0; i < col.getValueCount(); i++) {
                sum += col.get(i);
            }
            return sum;
        }

        @Override
        public Long merge(Long stateA, Long stateB) {
            return stateA + stateB;
        }

        @Override
        public Object evaluate(Long state) {
            return state;
        }
    }

    static class BenchProvider implements Function.FunctionProvider {
        @Override
        public Map<String, Object> functions() {
            Map<String, Object> fns = new LinkedHashMap<>();
            fns.put("double_val", new DoubleValFn());
            fns.put("int_sum", new IntSumFn());
            return fns;
        }

        @Override
        public Function.FunctionManifest metadata() {
            return new Function.FunctionManifest(List.of(
                new Function.FunctionMeta("double_val", List.of("Int64"), "Int64", "scalar"),
                new Function.FunctionMeta("int_sum", List.of("Int64"), "Int64", "aggregate")
            ));
        }
    }

    public static void main(String[] args) {
        FunctionServe.run(new BenchProvider(), args);
    }
}
