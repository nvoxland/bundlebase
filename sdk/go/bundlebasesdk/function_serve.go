package bundlebasesdk

import (
	"bufio"
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"sync"
	"time"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/array"
	"github.com/apache/arrow-go/v18/arrow/memory"
)

// ServeFunction runs the function provider as a JSON-RPC subprocess on stdin/stdout.
func ServeFunction(provider FunctionProvider) {
	ServeFunctionIO(provider, os.Stdin, os.Stdout)
}

// ServeFunctionIO runs the function provider on the given reader/writer (for testing).
func ServeFunctionIO(provider FunctionProvider, r io.Reader, w io.Writer) {
	br := bufio.NewReaderSize(r, 16*1024*1024)
	store := &stateStore{
		states:    make(map[string]interface{}),
		createdAt: make(map[string]time.Time),
	}
	lastCleanup := time.Now()

	for {
		// Periodically clean up expired aggregate state
		if now := time.Now(); now.Sub(lastCleanup) >= 60*time.Second {
			store.cleanup(5 * time.Minute)
			lastCleanup = now
		}

		line, err := br.ReadBytes('\n')
		if len(line) > 0 {
			// Trim the trailing newline
			trimmed := line
			if trimmed[len(trimmed)-1] == '\n' {
				trimmed = trimmed[:len(trimmed)-1]
			}
			if len(trimmed) == 0 {
				continue
			}

			var req jsonRpcRequest
			if jsonErr := json.Unmarshal(trimmed, &req); jsonErr != nil {
				writeError(w, nil, -32700, fmt.Sprintf("Parse error: %v", jsonErr))
				continue
			}

			shouldStop := handleFunctionRequest(provider, store, req, br, w)
			if shouldStop {
				return
			}
		}
		if err != nil {
			return
		}
	}
}

type stateStore struct {
	mu        sync.Mutex
	states    map[string]interface{}
	createdAt map[string]time.Time
	nextID    uint64
}

func (s *stateStore) add(state interface{}) string {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.nextID++
	id := fmt.Sprintf("state_%d", s.nextID)
	s.states[id] = state
	if s.createdAt == nil {
		s.createdAt = make(map[string]time.Time)
	}
	s.createdAt[id] = time.Now()
	return id
}

func (s *stateStore) get(id string) (interface{}, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	v, ok := s.states[id]
	return v, ok
}

func (s *stateStore) set(id string, state interface{}) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.states[id] = state
}

func (s *stateStore) remove(id string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	delete(s.states, id)
	delete(s.createdAt, id)
}

func (s *stateStore) cleanup(ttl time.Duration) {
	s.mu.Lock()
	defer s.mu.Unlock()
	now := time.Now()
	for id, created := range s.createdAt {
		if now.Sub(created) > ttl {
			delete(s.states, id)
			delete(s.createdAt, id)
		}
	}
}

func handleFunctionRequest(provider FunctionProvider, store *stateStore, req jsonRpcRequest, r io.Reader, w io.Writer) bool {
	switch req.Method {
	case "handshake":
		writeResponse(w, req.ID, map[string]string{"protocol_version": "1"})
	case "ping":
		writeResponse(w, req.ID, "pong")
	case "manifest":
		manifest := provider.Metadata()
		writeResponse(w, req.ID, manifest)
	case "invoke":
		handleInvoke(provider, req, r, w)
	case "create_state":
		handleCreateState(provider, store, req, w)
	case "accumulate":
		handleAccumulate(provider, store, req, r, w)
	case "merge":
		handleMerge(provider, store, req, w)
	case "evaluate":
		handleEvaluate(provider, store, req, w)
	case "shutdown":
		writeResponse(w, req.ID, map[string]bool{"ok": true})
		return true
	default:
		writeError(w, req.ID, -32601, fmt.Sprintf("Method not found: %s", req.Method))
	}
	return false
}

func handleInvoke(provider FunctionProvider, req jsonRpcRequest, r io.Reader, w io.Writer) {
	funcName, _ := req.Params["function"].(string)
	functions := provider.Functions()
	fn, ok := functions[funcName]
	if !ok {
		writeError(w, req.ID, -32000, fmt.Sprintf("Function not found: %s", funcName))
		return
	}

	scalar, ok := fn.(ScalarFunction)
	if !ok {
		writeError(w, req.ID, -32000, fmt.Sprintf("Function '%s' is not a scalar function (actual type: %T)", funcName, fn))
		return
	}

	// Read Arrow IPC input
	records, err := readArrowIPC(r)
	if err != nil {
		writeError(w, req.ID, -32000, fmt.Sprintf("Failed to read input: %v", err))
		return
	}

	// Extract columns as args (from the first record batch)
	var args []arrow.Array
	if len(records) > 0 {
		rec := records[0]
		for i := 0; i < int(rec.NumCols()); i++ {
			args = append(args, rec.Column(i))
		}
	}

	result, err := scalar.Invoke(args)
	if err != nil {
		writeError(w, req.ID, -32000, err.Error())
		return
	}

	// Build a single-column record batch from the result
	field := arrow.Field{Name: "result", Type: result.DataType()}
	schema := arrow.NewSchema([]arrow.Field{field}, nil)
	rec := array.NewRecord(schema, []arrow.Array{result}, int64(result.Len()))

	// Buffer Arrow IPC before sending ack
	var arrowBuf bytes.Buffer
	if err := writeArrowIPC(&arrowBuf, []arrow.Record{rec}); err != nil {
		writeError(w, req.ID, -32000, fmt.Sprintf("failed to serialize Arrow IPC result: %v", err))
		return
	}

	writeResponse(w, req.ID, map[string]bool{"ok": true})
	w.Write(arrowBuf.Bytes())
}

func handleCreateState(provider FunctionProvider, store *stateStore, req jsonRpcRequest, w io.Writer) {
	funcName, _ := req.Params["function"].(string)
	functions := provider.Functions()
	fn, ok := functions[funcName]
	if !ok {
		writeError(w, req.ID, -32000, fmt.Sprintf("Function not found: %s", funcName))
		return
	}

	agg, ok := fn.(AggregateFunction)
	if !ok {
		writeError(w, req.ID, -32000, fmt.Sprintf("Function '%s' is not an aggregate function (actual type: %T)", funcName, fn))
		return
	}

	state, err := agg.CreateState()
	if err != nil {
		writeError(w, req.ID, -32000, err.Error())
		return
	}

	id := store.add(state)
	writeResponse(w, req.ID, map[string]string{"state_id": id})
}

func handleAccumulate(provider FunctionProvider, store *stateStore, req jsonRpcRequest, r io.Reader, w io.Writer) {
	funcName, _ := req.Params["function"].(string)
	stateID, _ := req.Params["state_id"].(string)

	functions := provider.Functions()
	fn, ok := functions[funcName]
	if !ok {
		writeError(w, req.ID, -32000, fmt.Sprintf("Function not found: %s", funcName))
		return
	}

	agg, ok := fn.(AggregateFunction)
	if !ok {
		writeError(w, req.ID, -32000, fmt.Sprintf("Function '%s' is not an aggregate function (actual type: %T)", funcName, fn))
		return
	}

	state, ok := store.get(stateID)
	if !ok {
		writeError(w, req.ID, -32000, fmt.Sprintf("State not found: %s", stateID))
		return
	}

	// Read Arrow IPC input
	records, err := readArrowIPC(r)
	if err != nil {
		writeError(w, req.ID, -32000, fmt.Sprintf("Failed to read input: %v", err))
		return
	}

	var args []arrow.Array
	if len(records) > 0 {
		rec := records[0]
		for i := 0; i < int(rec.NumCols()); i++ {
			args = append(args, rec.Column(i))
		}
	}

	newState, err := agg.Accumulate(state, args)
	if err != nil {
		writeError(w, req.ID, -32000, err.Error())
		return
	}

	store.set(stateID, newState)
	writeResponse(w, req.ID, map[string]bool{"ok": true})
}

func handleMerge(provider FunctionProvider, store *stateStore, req jsonRpcRequest, w io.Writer) {
	funcName, _ := req.Params["function"].(string)
	stateIDA, _ := req.Params["state_id_a"].(string)
	stateIDB, _ := req.Params["state_id_b"].(string)

	functions := provider.Functions()
	fn, ok := functions[funcName]
	if !ok {
		writeError(w, req.ID, -32000, fmt.Sprintf("Function not found: %s", funcName))
		return
	}

	agg, ok := fn.(AggregateFunction)
	if !ok {
		writeError(w, req.ID, -32000, fmt.Sprintf("Function '%s' is not an aggregate function (actual type: %T)", funcName, fn))
		return
	}

	stateA, ok := store.get(stateIDA)
	if !ok {
		writeError(w, req.ID, -32000, fmt.Sprintf("State not found: %s", stateIDA))
		return
	}

	stateB, ok := store.get(stateIDB)
	if !ok {
		writeError(w, req.ID, -32000, fmt.Sprintf("State not found: %s", stateIDB))
		return
	}

	merged, err := agg.Merge(stateA, stateB)
	if err != nil {
		writeError(w, req.ID, -32000, err.Error())
		return
	}

	store.set(stateIDA, merged)
	store.remove(stateIDB)
	writeResponse(w, req.ID, map[string]bool{"ok": true})
}

func handleEvaluate(provider FunctionProvider, store *stateStore, req jsonRpcRequest, w io.Writer) {
	funcName, _ := req.Params["function"].(string)
	stateID, _ := req.Params["state_id"].(string)

	functions := provider.Functions()
	fn, ok := functions[funcName]
	if !ok {
		writeError(w, req.ID, -32000, fmt.Sprintf("Function not found: %s", funcName))
		return
	}

	agg, ok := fn.(AggregateFunction)
	if !ok {
		writeError(w, req.ID, -32000, fmt.Sprintf("Function '%s' is not an aggregate function (actual type: %T)", funcName, fn))
		return
	}

	state, ok := store.get(stateID)
	if !ok {
		writeError(w, req.ID, -32000, fmt.Sprintf("State not found: %s", stateID))
		return
	}

	result, err := agg.Evaluate(state)
	if err != nil {
		writeError(w, req.ID, -32000, err.Error())
		return
	}

	// Convert the result to an Arrow array and write as IPC
	alloc := memory.NewGoAllocator()
	var resultArr arrow.Array

	switch v := result.(type) {
	case int64:
		b := array.NewInt64Builder(alloc)
		b.Append(v)
		resultArr = b.NewArray()
	case float64:
		b := array.NewFloat64Builder(alloc)
		b.Append(v)
		resultArr = b.NewArray()
	case string:
		b := array.NewStringBuilder(alloc)
		b.Append(v)
		resultArr = b.NewArray()
	case bool:
		b := array.NewBooleanBuilder(alloc)
		b.Append(v)
		resultArr = b.NewArray()
	default:
		writeError(w, req.ID, -32000, fmt.Sprintf("Unsupported evaluate result type: %T", result))
		return
	}

	field := arrow.Field{Name: "result", Type: resultArr.DataType()}
	schema := arrow.NewSchema([]arrow.Field{field}, nil)
	rec := array.NewRecord(schema, []arrow.Array{resultArr}, 1)

	// Buffer Arrow IPC before sending ack
	var arrowBuf bytes.Buffer
	if err := writeArrowIPC(&arrowBuf, []arrow.Record{rec}); err != nil {
		writeError(w, req.ID, -32000, fmt.Sprintf("failed to serialize Arrow IPC result: %v", err))
		return
	}

	writeResponse(w, req.ID, map[string]bool{"ok": true})
	w.Write(arrowBuf.Bytes())

	store.remove(stateID)
}
