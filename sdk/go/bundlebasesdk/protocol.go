package bundlebasesdk

import (
	"bytes"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"io"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/ipc"
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
