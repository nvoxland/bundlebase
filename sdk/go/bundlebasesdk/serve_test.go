package bundlebasesdk

import (
	"bytes"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"strings"
	"testing"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/array"
	"github.com/apache/arrow-go/v18/arrow/ipc"
	"github.com/apache/arrow-go/v18/arrow/memory"
)

// testSource is a minimal source for testing.
type testSource struct{}

func (s *testSource) Discover(attached []string, args map[string]string) ([]Location, error) {
	return []Location{
		{Location: "file1.parquet", MustCopy: true, Format: "parquet", Version: "v1"},
		{Location: "file2.parquet", MustCopy: true, Format: "parquet", Version: "v1"},
	}, nil
}

func (s *testSource) Data(location Location, args map[string]string) ([]arrow.Record, error) {
	alloc := memory.NewGoAllocator()
	schema := arrow.NewSchema([]arrow.Field{
		{Name: "id", Type: arrow.PrimitiveTypes.Int64},
		{Name: "name", Type: arrow.BinaryTypes.String},
	}, nil)

	if location.Location == "file1.parquet" {
		b1 := array.NewRecordBuilder(alloc, schema)
		defer b1.Release()
		b1.Field(0).(*array.Int64Builder).AppendValues([]int64{1, 2}, nil)
		b1.Field(1).(*array.StringBuilder).AppendValues([]string{"alice", "bob"}, nil)
		rec1 := b1.NewRecord()

		b2 := array.NewRecordBuilder(alloc, schema)
		defer b2.Release()
		b2.Field(0).(*array.Int64Builder).AppendValues([]int64{3}, nil)
		b2.Field(1).(*array.StringBuilder).AppendValues([]string{"charlie"}, nil)
		rec2 := b2.NewRecord()

		return []arrow.Record{rec1, rec2}, nil
	} else if location.Location == "file2.parquet" {
		b := array.NewRecordBuilder(alloc, schema)
		defer b.Release()
		b.Field(0).(*array.Int64Builder).AppendValues([]int64{4, 5}, nil)
		b.Field(1).(*array.StringBuilder).AppendValues([]string{"dave", "eve"}, nil)
		return []arrow.Record{b.NewRecord()}, nil
	}
	return nil, nil
}

// errorSource returns errors.
type errorSource struct{}

func (s *errorSource) Discover(attached []string, args map[string]string) ([]Location, error) {
	return nil, fmt.Errorf("discover exploded")
}

func (s *errorSource) Data(location Location, args map[string]string) ([]arrow.Record, error) {
	return nil, nil
}

func makeRequest(method string, params map[string]interface{}, id int) string {
	req := map[string]interface{}{
		"jsonrpc": "2.0",
		"id":      id,
		"method":  method,
		"params":  params,
	}
	b, _ := json.Marshal(req)
	return string(b) + "\n"
}

func readResponse(t *testing.T, data []byte, offset int) (map[string]interface{}, int) {
	t.Helper()
	end := bytes.IndexByte(data[offset:], '\n')
	if end < 0 {
		t.Fatal("no newline found in response")
	}
	line := data[offset : offset+end]
	var resp map[string]interface{}
	if err := json.Unmarshal(line, &resp); err != nil {
		t.Fatalf("failed to parse response: %v", err)
	}
	return resp, offset + end + 1
}

func readArrowFrame(t *testing.T, data []byte, offset int) (int64, int) {
	t.Helper()
	if offset+4 > len(data) {
		t.Fatal("not enough data for length prefix")
	}
	length := binary.BigEndian.Uint32(data[offset : offset+4])
	offset += 4
	if length == 0 {
		return 0, offset
	}

	ipcData := data[offset : offset+int(length)]
	reader, err := ipc.NewReader(bytes.NewReader(ipcData))
	if err != nil {
		t.Fatalf("failed to create Arrow reader: %v", err)
	}
	defer reader.Release()

	var totalRows int64
	for reader.Next() {
		rec := reader.Record()
		totalRows += rec.NumRows()
	}
	return totalRows, offset + int(length)
}

func TestDiscover(t *testing.T) {
	input := makeRequest("discover", map[string]interface{}{"attached_locations": []string{}}, 1) +
		makeRequest("shutdown", nil, 2)

	var out bytes.Buffer
	ServeIO(&testSource{}, strings.NewReader(input), &out)

	resp, _ := readResponse(t, out.Bytes(), 0)
	result := resp["result"].(map[string]interface{})
	locations := result["locations"].([]interface{})
	if len(locations) != 2 {
		t.Fatalf("expected 2 locations, got %d", len(locations))
	}
	loc0 := locations[0].(map[string]interface{})
	if loc0["location"] != "file1.parquet" {
		t.Errorf("expected file1.parquet, got %s", loc0["location"])
	}
}

func TestDataReturnsArrow(t *testing.T) {
	input := makeRequest("data", map[string]interface{}{
		"location": map[string]interface{}{
			"location":  "file1.parquet",
			"must_copy": true,
			"format":    "parquet",
			"version":   "v1",
		},
	}, 1) + makeRequest("shutdown", nil, 2)

	var out bytes.Buffer
	ServeIO(&testSource{}, strings.NewReader(input), &out)

	data := out.Bytes()
	resp, offset := readResponse(t, data, 0)
	result := resp["result"].(map[string]interface{})
	if result["ok"] != true {
		t.Error("expected ok:true")
	}

	totalRows, _ := readArrowFrame(t, data, offset)
	if totalRows != 3 {
		t.Errorf("expected 3 rows, got %d", totalRows)
	}
}

func TestDataNone(t *testing.T) {
	input := makeRequest("data", map[string]interface{}{
		"location": map[string]interface{}{"location": "nonexistent"},
	}, 1) + makeRequest("shutdown", nil, 2)

	var out bytes.Buffer
	ServeIO(&testSource{}, strings.NewReader(input), &out)

	data := out.Bytes()
	_, offset := readResponse(t, data, 0)

	// Should be zero-length frame
	length := binary.BigEndian.Uint32(data[offset : offset+4])
	if length != 0 {
		t.Errorf("expected zero-length frame, got %d", length)
	}
}

func TestUnknownMethod(t *testing.T) {
	input := makeRequest("bogus", nil, 1) +
		makeRequest("shutdown", nil, 2)

	var out bytes.Buffer
	ServeIO(&testSource{}, strings.NewReader(input), &out)

	resp, _ := readResponse(t, out.Bytes(), 0)
	errObj := resp["error"].(map[string]interface{})
	if int(errObj["code"].(float64)) != -32601 {
		t.Errorf("expected error code -32601, got %v", errObj["code"])
	}
}

func TestMalformedJsonReturnsParseError(t *testing.T) {
	input := "this is not json\n" +
		makeRequest("shutdown", nil, 2)

	var out bytes.Buffer
	ServeIO(&testSource{}, strings.NewReader(input), &out)

	resp, _ := readResponse(t, out.Bytes(), 0)
	errObj := resp["error"].(map[string]interface{})
	if int(errObj["code"].(float64)) != -32700 {
		t.Errorf("expected error code -32700, got %v", errObj["code"])
	}
	if !strings.Contains(errObj["message"].(string), "Parse error") {
		t.Errorf("expected 'Parse error' in message, got %s", errObj["message"])
	}
}

func TestUserErrorWrapped(t *testing.T) {
	input := makeRequest("discover", map[string]interface{}{"attached_locations": []string{}}, 1) +
		makeRequest("shutdown", nil, 2)

	var out bytes.Buffer
	ServeIO(&errorSource{}, strings.NewReader(input), &out)

	resp, _ := readResponse(t, out.Bytes(), 0)
	errObj := resp["error"].(map[string]interface{})
	if int(errObj["code"].(float64)) != -32000 {
		t.Errorf("expected error code -32000, got %v", errObj["code"])
	}
	if !strings.Contains(errObj["message"].(string), "discover exploded") {
		t.Errorf("expected error message to contain 'discover exploded'")
	}
}
