package bundlebasesdk

import (
	"bytes"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"io"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/array"
	"github.com/apache/arrow-go/v18/arrow/float16"
	"github.com/apache/arrow-go/v18/arrow/ipc"
	"github.com/apache/arrow-go/v18/arrow/memory"
)

// jsonRpcRequest represents an incoming JSON-RPC 2.0 request.
type jsonRpcRequest struct {
	JSONRPC string                 `json:"jsonrpc"`
	ID      json.RawMessage        `json:"id"`
	Method  string                 `json:"method"`
	Params  map[string]interface{} `json:"params"`
}

// jsonRpcResponse represents an outgoing JSON-RPC 2.0 response.
type jsonRpcResponse struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      json.RawMessage `json:"id"`
	Result  interface{}     `json:"result,omitempty"`
	Error   *jsonRpcError   `json:"error,omitempty"`
}

// jsonRpcError represents a JSON-RPC error.
type jsonRpcError struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
}

// writeResponse writes a JSON-RPC response line to the writer.
func writeResponse(w io.Writer, id json.RawMessage, result interface{}) error {
	resp := jsonRpcResponse{JSONRPC: "2.0", ID: id, Result: result}
	data, err := json.Marshal(resp)
	if err != nil {
		return fmt.Errorf("failed to marshal response: %w", err)
	}
	data = append(data, '\n')
	_, err = w.Write(data)
	return err
}

// writeError writes a JSON-RPC error response line to the writer.
func writeError(w io.Writer, id json.RawMessage, code int, message string) error {
	resp := jsonRpcResponse{
		JSONRPC: "2.0",
		ID:      id,
		Error:   &jsonRpcError{Code: code, Message: message},
	}
	data, err := json.Marshal(resp)
	if err != nil {
		return fmt.Errorf("failed to marshal error response: %w", err)
	}
	data = append(data, '\n')
	_, err = w.Write(data)
	return err
}

// writeArrowIPC writes length-prefixed Arrow IPC stream bytes to the writer.
// An empty slice writes a zero-length frame.
func writeArrowIPC(w io.Writer, records []arrow.Record) error {
	if len(records) == 0 {
		return binary.Write(w, binary.BigEndian, uint32(0))
	}

	var buf bytes.Buffer
	writer := ipc.NewWriter(&buf, ipc.WithSchema(records[0].Schema()))
	for _, rec := range records {
		if err := writer.Write(rec); err != nil {
			return fmt.Errorf("failed to write Arrow record: %w", err)
		}
	}
	if err := writer.Close(); err != nil {
		return fmt.Errorf("failed to close Arrow writer: %w", err)
	}

	data := buf.Bytes()
	if err := binary.Write(w, binary.BigEndian, uint32(len(data))); err != nil {
		return fmt.Errorf("failed to write length prefix: %w", err)
	}
	_, err := w.Write(data)
	return err
}

// parseStringSlice extracts a []string from a params value.
func parseStringSlice(v interface{}) []string {
	if v == nil {
		return nil
	}
	arr, ok := v.([]interface{})
	if !ok {
		return nil
	}
	result := make([]string, 0, len(arr))
	for _, item := range arr {
		if s, ok := item.(string); ok {
			result = append(result, s)
		}
	}
	return result
}

// parseStringMap extracts string-only key-value pairs from params,
// excluding the specified keys.
func parseStringMap(params map[string]interface{}, exclude ...string) map[string]string {
	excl := make(map[string]bool, len(exclude))
	for _, k := range exclude {
		excl[k] = true
	}
	result := make(map[string]string)
	for k, v := range params {
		if excl[k] {
			continue
		}
		if s, ok := v.(string); ok {
			result[k] = s
		}
	}
	return result
}

// readArrowIPC reads a length-prefixed Arrow IPC stream from the reader.
func readArrowIPC(r io.Reader) ([]arrow.Record, error) {
	var length uint32
	if err := binary.Read(r, binary.BigEndian, &length); err != nil {
		return nil, fmt.Errorf("failed to read IPC length prefix: %w", err)
	}
	if length == 0 {
		return nil, nil
	}

	data := make([]byte, length)
	if _, err := io.ReadFull(r, data); err != nil {
		return nil, fmt.Errorf("failed to read IPC data: %w", err)
	}

	reader, err := ipc.NewReader(bytes.NewReader(data))
	if err != nil {
		return nil, fmt.Errorf("failed to create Arrow reader: %w", err)
	}
	defer reader.Release()

	var records []arrow.Record
	for reader.Next() {
		rec := reader.Record()
		rec.Retain()
		records = append(records, rec)
	}
	return records, nil
}

// NormalizeToRecords converts Go data structures into Arrow Record batches using
// the provided schema map (column name -> type name string).
//
// Supported input types:
//   - []arrow.Record: passed through as-is (schema is ignored).
//   - []map[string]interface{}: row-oriented data; each map is one row.
//   - map[string][]interface{}: column-oriented data; each key is a column.
func NormalizeToRecords(data interface{}, schema map[string]string) ([]arrow.Record, error) {
	switch v := data.(type) {
	case []arrow.Record:
		return v, nil

	case []map[string]interface{}:
		return rowMapsToRecords(v, schema)

	case map[string][]interface{}:
		return columnMapToRecords(v, schema)

	default:
		return nil, fmt.Errorf("unsupported data type %T; expected []arrow.Record, []map[string]interface{}, or map[string][]interface{}", data)
	}
}

// rowMapsToRecords converts row-oriented maps to a single Arrow Record.
func rowMapsToRecords(rows []map[string]interface{}, schema map[string]string) ([]arrow.Record, error) {
	if len(rows) == 0 {
		return nil, nil
	}

	arrowSchema, err := SchemaFromTypes(schema)
	if err != nil {
		return nil, err
	}

	alloc := memory.DefaultAllocator
	bldr := array.NewRecordBuilder(alloc, arrowSchema)
	defer bldr.Release()

	for _, row := range rows {
		for i, field := range arrowSchema.Fields() {
			val := row[field.Name]
			if err := appendValue(bldr.Field(i), field.Type, val); err != nil {
				return nil, fmt.Errorf("column %q row value: %w", field.Name, err)
			}
		}
	}

	rec := bldr.NewRecord()
	return []arrow.Record{rec}, nil
}

// columnMapToRecords converts column-oriented maps to a single Arrow Record.
func columnMapToRecords(cols map[string][]interface{}, schema map[string]string) ([]arrow.Record, error) {
	if len(cols) == 0 {
		return nil, nil
	}

	arrowSchema, err := SchemaFromTypes(schema)
	if err != nil {
		return nil, err
	}

	alloc := memory.DefaultAllocator
	bldr := array.NewRecordBuilder(alloc, arrowSchema)
	defer bldr.Release()

	// Determine row count from first column.
	var nRows int
	for _, field := range arrowSchema.Fields() {
		if colData, ok := cols[field.Name]; ok {
			nRows = len(colData)
			break
		}
	}

	for i, field := range arrowSchema.Fields() {
		colData := cols[field.Name]
		for j := 0; j < nRows; j++ {
			var val interface{}
			if j < len(colData) {
				val = colData[j]
			}
			if err := appendValue(bldr.Field(i), field.Type, val); err != nil {
				return nil, fmt.Errorf("column %q index %d: %w", field.Name, j, err)
			}
		}
	}

	rec := bldr.NewRecord()
	return []arrow.Record{rec}, nil
}

// appendValue appends a single Go value to the appropriate Arrow array builder.
func appendValue(builder array.Builder, dt arrow.DataType, val interface{}) error {
	if val == nil {
		builder.AppendNull()
		return nil
	}

	switch dt.ID() {
	case arrow.BOOL:
		b, ok := val.(bool)
		if !ok {
			return fmt.Errorf("expected bool, got %T", val)
		}
		builder.(*array.BooleanBuilder).Append(b)

	case arrow.INT8:
		n, err := toInt64(val)
		if err != nil {
			return err
		}
		builder.(*array.Int8Builder).Append(int8(n))

	case arrow.INT16:
		n, err := toInt64(val)
		if err != nil {
			return err
		}
		builder.(*array.Int16Builder).Append(int16(n))

	case arrow.INT32:
		n, err := toInt64(val)
		if err != nil {
			return err
		}
		builder.(*array.Int32Builder).Append(int32(n))

	case arrow.INT64:
		n, err := toInt64(val)
		if err != nil {
			return err
		}
		builder.(*array.Int64Builder).Append(n)

	case arrow.UINT8:
		n, err := toUint64(val)
		if err != nil {
			return err
		}
		builder.(*array.Uint8Builder).Append(uint8(n))

	case arrow.UINT16:
		n, err := toUint64(val)
		if err != nil {
			return err
		}
		builder.(*array.Uint16Builder).Append(uint16(n))

	case arrow.UINT32:
		n, err := toUint64(val)
		if err != nil {
			return err
		}
		builder.(*array.Uint32Builder).Append(uint32(n))

	case arrow.UINT64:
		n, err := toUint64(val)
		if err != nil {
			return err
		}
		builder.(*array.Uint64Builder).Append(n)

	case arrow.FLOAT16:
		f, err := toFloat64(val)
		if err != nil {
			return err
		}
		builder.(*array.Float16Builder).Append(float16.New(float32(f)))

	case arrow.FLOAT32:
		f, err := toFloat64(val)
		if err != nil {
			return err
		}
		builder.(*array.Float32Builder).Append(float32(f))

	case arrow.FLOAT64:
		f, err := toFloat64(val)
		if err != nil {
			return err
		}
		builder.(*array.Float64Builder).Append(f)

	case arrow.STRING:
		s, ok := val.(string)
		if !ok {
			return fmt.Errorf("expected string, got %T", val)
		}
		builder.(*array.StringBuilder).Append(s)

	case arrow.LARGE_STRING:
		s, ok := val.(string)
		if !ok {
			return fmt.Errorf("expected string, got %T", val)
		}
		builder.(*array.LargeStringBuilder).Append(s)

	case arrow.BINARY:
		switch b := val.(type) {
		case []byte:
			builder.(*array.BinaryBuilder).Append(b)
		default:
			return fmt.Errorf("expected []byte, got %T", val)
		}

	case arrow.LARGE_BINARY:
		switch b := val.(type) {
		case []byte:
			builder.(*array.LargeBinaryBuilder).Append(b)
		default:
			return fmt.Errorf("expected []byte, got %T", val)
		}

	case arrow.DATE32:
		n, err := toInt64(val)
		if err != nil {
			return err
		}
		builder.(*array.Date32Builder).Append(arrow.Date32(n))

	case arrow.DATE64:
		n, err := toInt64(val)
		if err != nil {
			return err
		}
		builder.(*array.Date64Builder).Append(arrow.Date64(n))

	case arrow.TIMESTAMP:
		n, err := toInt64(val)
		if err != nil {
			return err
		}
		builder.(*array.TimestampBuilder).Append(arrow.Timestamp(n))

	default:
		return fmt.Errorf("unsupported Arrow type %s", dt)
	}

	return nil
}

// toInt64 converts numeric Go values to int64.
func toInt64(val interface{}) (int64, error) {
	switch n := val.(type) {
	case int:
		return int64(n), nil
	case int8:
		return int64(n), nil
	case int16:
		return int64(n), nil
	case int32:
		return int64(n), nil
	case int64:
		return n, nil
	case float32:
		return int64(n), nil
	case float64:
		return int64(n), nil
	case json.Number:
		return n.Int64()
	default:
		return 0, fmt.Errorf("expected numeric type, got %T", val)
	}
}

// toUint64 converts numeric Go values to uint64.
func toUint64(val interface{}) (uint64, error) {
	switch n := val.(type) {
	case uint:
		return uint64(n), nil
	case uint8:
		return uint64(n), nil
	case uint16:
		return uint64(n), nil
	case uint32:
		return uint64(n), nil
	case uint64:
		return n, nil
	case int:
		return uint64(n), nil
	case int64:
		return uint64(n), nil
	case float64:
		return uint64(n), nil
	case json.Number:
		i, err := n.Int64()
		return uint64(i), err
	default:
		return 0, fmt.Errorf("expected numeric type, got %T", val)
	}
}

// toFloat64 converts numeric Go values to float64.
func toFloat64(val interface{}) (float64, error) {
	switch n := val.(type) {
	case float32:
		return float64(n), nil
	case float64:
		return n, nil
	case int:
		return float64(n), nil
	case int64:
		return float64(n), nil
	case json.Number:
		return n.Float64()
	default:
		return 0, fmt.Errorf("expected numeric type, got %T", val)
	}
}

// parseLocation extracts a Location from a params["location"] value.
func parseLocation(v interface{}) Location {
	loc := Location{Format: "parquet"}
	m, ok := v.(map[string]interface{})
	if !ok {
		return loc
	}
	if s, ok := m["location"].(string); ok {
		loc.Location = s
	}
	if b, ok := m["must_copy"].(bool); ok {
		loc.MustCopy = b
	}
	if s, ok := m["format"].(string); ok {
		loc.Format = s
	}
	if s, ok := m["version"].(string); ok {
		loc.Version = s
	}
	return loc
}
