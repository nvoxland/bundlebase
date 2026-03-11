package bundlebasesdk

import (
	"bytes"
	"encoding/binary"
	"strings"
	"testing"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/array"
	"github.com/apache/arrow-go/v18/arrow/ipc"
	"github.com/apache/arrow-go/v18/arrow/memory"
)

// doubleVal is a scalar function that doubles int64 values.
type doubleVal struct{}

func (d *doubleVal) Invoke(batch arrow.Record) (arrow.Array, error) {
	input := batch.Column(0).(*array.Int64)
	alloc := memory.NewGoAllocator()
	b := array.NewInt64Builder(alloc)
	for i := 0; i < input.Len(); i++ {
		b.Append(input.Value(i) * 2)
	}
	return b.NewArray(), nil
}

// sumAgg is an aggregate function that sums int64 values.
type sumAgg struct{}

func (s *sumAgg) CreateState() (interface{}, error) {
	return int64(0), nil
}

func (s *sumAgg) Accumulate(state interface{}, batch arrow.Record) (interface{}, error) {
	sum := state.(int64)
	input := batch.Column(0).(*array.Int64)
	for i := 0; i < input.Len(); i++ {
		sum += input.Value(i)
	}
	return sum, nil
}

func (s *sumAgg) Merge(stateA interface{}, stateB interface{}) (interface{}, error) {
	return stateA.(int64) + stateB.(int64), nil
}

func (s *sumAgg) Evaluate(state interface{}) (interface{}, error) {
	return state.(int64), nil
}

// testProvider groups the test functions.
type testProvider struct{}

func (p *testProvider) Functions() map[string]FunctionRef {
	return map[string]FunctionRef{
		"double_val": {Scalar: &doubleVal{}},
		"my_sum":     {Aggregate: &sumAgg{}},
	}
}

func (p *testProvider) Metadata() FunctionManifest {
	return FunctionManifest{
		Functions: []FunctionMeta{
			{Name: "double_val", InputTypes: []string{"Int64"}, ReturnType: "Int64", Kind: "scalar"},
			{Name: "my_sum", InputTypes: []string{"Int64"}, ReturnType: "Int64", Kind: "aggregate"},
		},
	}
}

func buildArrowIPC(t *testing.T, values []int64) []byte {
	t.Helper()
	alloc := memory.NewGoAllocator()
	schema := arrow.NewSchema([]arrow.Field{
		{Name: "col0", Type: arrow.PrimitiveTypes.Int64},
	}, nil)
	b := array.NewRecordBuilder(alloc, schema)
	defer b.Release()
	b.Field(0).(*array.Int64Builder).AppendValues(values, nil)
	rec := b.NewRecord()
	defer rec.Release()

	var buf bytes.Buffer
	w := ipc.NewWriter(&buf, ipc.WithSchema(schema))
	w.Write(rec)
	w.Close()

	ipcBytes := buf.Bytes()
	var result bytes.Buffer
	binary.Write(&result, binary.BigEndian, uint32(len(ipcBytes)))
	result.Write(ipcBytes)
	return result.Bytes()
}

func TestManifest(t *testing.T) {
	input := makeRequest("manifest", nil, 1) + makeRequest("shutdown", nil, 2)
	var out bytes.Buffer
	ServeFunctionIO(&testProvider{}, strings.NewReader(input), &out)

	resp, _ := readResponse(t, out.Bytes(), 0)
	result := resp["result"].(map[string]interface{})
	funcs := result["functions"].([]interface{})
	if len(funcs) != 2 {
		t.Fatalf("expected 2 functions, got %d", len(funcs))
	}
}

func TestInvokeScalar(t *testing.T) {
	ipcData := buildArrowIPC(t, []int64{1, 2, 3})
	jsonReq := makeRequest("invoke", map[string]interface{}{"function": "double_val"}, 1)

	var inputBuf bytes.Buffer
	inputBuf.WriteString(jsonReq)
	inputBuf.Write(ipcData)
	inputBuf.WriteString(makeRequest("shutdown", nil, 2))

	var out bytes.Buffer
	ServeFunctionIO(&testProvider{}, &inputBuf, &out)

	data := out.Bytes()
	resp, offset := readResponse(t, data, 0)
	result := resp["result"].(map[string]interface{})
	if result["ok"] != true {
		t.Error("expected ok:true")
	}

	// Read Arrow IPC output
	totalRows, _ := readArrowFrame(t, data, offset)
	if totalRows != 3 {
		t.Errorf("expected 3 rows, got %d", totalRows)
	}
}

func TestAggregateWorkflow(t *testing.T) {
	// 1. Create state
	input := makeRequest("create_state", map[string]interface{}{"function": "my_sum"}, 1)
	// We need to build the full input including accumulate with IPC data
	ipcData := buildArrowIPC(t, []int64{10, 20, 30})

	var inputBuf bytes.Buffer
	inputBuf.WriteString(input)
	// 2. Accumulate
	accReq := makeRequest("accumulate", map[string]interface{}{"function": "my_sum", "state_id": "state_1"}, 2)
	inputBuf.WriteString(accReq)
	inputBuf.Write(ipcData)
	// 3. Evaluate
	evalReq := makeRequest("evaluate", map[string]interface{}{"function": "my_sum", "state_id": "state_1"}, 3)
	inputBuf.WriteString(evalReq)
	inputBuf.WriteString(makeRequest("shutdown", nil, 4))

	var out bytes.Buffer
	ServeFunctionIO(&testProvider{}, &inputBuf, &out)

	data := out.Bytes()

	// Response 1: create_state
	resp, offset := readResponse(t, data, 0)
	result := resp["result"].(map[string]interface{})
	stateID := result["state_id"].(string)
	if stateID == "" {
		t.Error("expected non-empty state_id")
	}

	// Response 2: accumulate
	resp, offset = readResponse(t, data, offset)
	result = resp["result"].(map[string]interface{})
	if result["ok"] != true {
		t.Error("expected ok:true for accumulate")
	}

	// Response 3: evaluate
	resp, offset = readResponse(t, data, offset)
	result = resp["result"].(map[string]interface{})
	if result["ok"] != true {
		t.Error("expected ok:true for evaluate")
	}

	// Read the Arrow IPC result
	totalRows, _ := readArrowFrame(t, data, offset)
	if totalRows != 1 {
		t.Errorf("expected 1 row for evaluate, got %d", totalRows)
	}
}
