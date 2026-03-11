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
//
// If the first command-line argument is --bundlebase-functions,
// prints the function manifest as JSON and exits.
func ServeFunction(provider FunctionProvider) {
	if len(os.Args) > 1 && os.Args[1] == "--bundlebase-functions" {
		manifest := provider.Metadata()
		data, err := json.Marshal(manifest)
		if err != nil {
			fmt.Fprintf(os.Stderr, "Failed to serialize manifest: %v\n", err)
			os.Exit(1)
		}
		os.Stdout.Write(data)
		os.Stdout.Write([]byte("\n"))
		return
	}
	ServeFunctionIO(provider, os.Stdin, os.Stdout)
}

// ServeFunctionIO runs the function provider on the given reader/writer (for testing).
func ServeFunctionIO(provider FunctionProvider, r io.Reader, w io.Writer) {
	br := bufio.NewReaderSize(r, 16*1024*1024)
	store := &stateStore{
		states:     make(map[string]interface{}),
		lastAccess: make(map[string]time.Time),
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
	mu         sync.Mutex
	states     map[string]interface{}
	lastAccess map[string]time.Time
	nextID     uint64
}

func (s *stateStore) add(state interface{}) string {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.nextID++
	id := fmt.Sprintf("state_%d", s.nextID)
	s.states[id] = state
	if s.lastAccess == nil {
		s.lastAccess = make(map[string]time.Time)
	}
	s.lastAccess[id] = time.Now()
	return id
}

func (s *stateStore) get(id string) (interface{}, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	v, ok := s.states[id]
	if ok {
		// Update last-access time for TTL
		s.lastAccess[id] = time.Now()
	}
	return v, ok
}

func (s *stateStore) set(id string, state interface{}) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.states[id] = state
	// Update last-access time for TTL
	s.lastAccess[id] = time.Now()
}

func (s *stateStore) remove(id string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	delete(s.states, id)
	delete(s.lastAccess, id)
}

func (s *stateStore) cleanup(ttl time.Duration) {
	s.mu.Lock()
	defer s.mu.Unlock()
	now := time.Now()
	for id, accessed := range s.lastAccess {
		if now.Sub(accessed) > ttl {
			delete(s.states, id)
			delete(s.lastAccess, id)
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
	ref, ok := functions[funcName]
	if !ok {
		writeError(w, req.ID, -32000, fmt.Sprintf("Function not found: %s", funcName))
		return
	}

	if ref.Scalar == nil {
		writeError(w, req.ID, -32000, fmt.Sprintf("Function '%s' is not a scalar function", funcName))
		return
	}

	// Send ack before reading Arrow IPC (host blocks on ack before sending data)
	writeResponse(w, req.ID, map[string]bool{"ok": true})

	// Read Arrow IPC input
	records, err := readArrowIPC(r)
	if err != nil {
		// After ack, errors must be written as empty Arrow IPC (host is reading IPC, not JSON)
		var emptyBuf bytes.Buffer
		writeArrowIPC(&emptyBuf, nil)
		w.Write(emptyBuf.Bytes())
		return
	}

	// Pass the first record batch directly to the function
	var inputBatch arrow.Record
	if len(records) > 0 {
		inputBatch = records[0]
	}

	result, err := ref.Scalar.Invoke(inputBatch)
	if err != nil {
		var emptyBuf bytes.Buffer
		writeArrowIPC(&emptyBuf, nil)
		w.Write(emptyBuf.Bytes())
		return
	}

	// Build a single-column record batch from the result
	field := arrow.Field{Name: "result", Type: result.DataType()}
	schema := arrow.NewSchema([]arrow.Field{field}, nil)
	rec := array.NewRecord(schema, []arrow.Array{result}, int64(result.Len()))

	// Write Arrow IPC output
	var arrowBuf bytes.Buffer
	if err := writeArrowIPC(&arrowBuf, []arrow.Record{rec}); err != nil {
		return
	}

	w.Write(arrowBuf.Bytes())
}

func handleCreateState(provider FunctionProvider, store *stateStore, req jsonRpcRequest, w io.Writer) {
	funcName, _ := req.Params["function"].(string)
	functions := provider.Functions()
	ref, ok := functions[funcName]
	if !ok {
		writeError(w, req.ID, -32000, fmt.Sprintf("Function not found: %s", funcName))
		return
	}

	if ref.Aggregate == nil {
		writeError(w, req.ID, -32000, fmt.Sprintf("Function '%s' is not an aggregate function", funcName))
		return
	}

	state, err := ref.Aggregate.CreateState()
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
	ref, ok := functions[funcName]
	if !ok {
		writeError(w, req.ID, -32000, fmt.Sprintf("Function not found: %s", funcName))
		return
	}

	if ref.Aggregate == nil {
		writeError(w, req.ID, -32000, fmt.Sprintf("Function '%s' is not an aggregate function", funcName))
		return
	}

	state, ok := store.get(stateID)
	if !ok {
		writeError(w, req.ID, -32000, fmt.Sprintf("State not found: %s", stateID))
		return
	}

	// Send ack before reading Arrow IPC (host blocks on ack before sending data)
	writeResponse(w, req.ID, map[string]bool{"ok": true})

	// Read Arrow IPC input
	records, err := readArrowIPC(r)
	if err != nil {
		return
	}

	// Pass the first record batch directly to the function
	var inputBatch arrow.Record
	if len(records) > 0 {
		inputBatch = records[0]
	}

	newState, err := ref.Aggregate.Accumulate(state, inputBatch)
	if err != nil {
		return
	}

	store.set(stateID, newState)
}

func handleMerge(provider FunctionProvider, store *stateStore, req jsonRpcRequest, w io.Writer) {
	funcName, _ := req.Params["function"].(string)
	stateIDA, _ := req.Params["state_id1"].(string)
	stateIDB, _ := req.Params["state_id2"].(string)

	functions := provider.Functions()
	ref, ok := functions[funcName]
	if !ok {
		writeError(w, req.ID, -32000, fmt.Sprintf("Function not found: %s", funcName))
		return
	}

	if ref.Aggregate == nil {
		writeError(w, req.ID, -32000, fmt.Sprintf("Function '%s' is not an aggregate function", funcName))
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

	merged, err := ref.Aggregate.Merge(stateA, stateB)
	if err != nil {
		writeError(w, req.ID, -32000, err.Error())
		return
	}

	store.set(stateIDA, merged)
	store.remove(stateIDB)
	writeResponse(w, req.ID, map[string]string{"state_id": stateIDA})
}

func handleEvaluate(provider FunctionProvider, store *stateStore, req jsonRpcRequest, w io.Writer) {
	funcName, _ := req.Params["function"].(string)
	stateID, _ := req.Params["state_id"].(string)

	functions := provider.Functions()
	ref, ok := functions[funcName]
	if !ok {
		writeError(w, req.ID, -32000, fmt.Sprintf("Function not found: %s", funcName))
		return
	}

	if ref.Aggregate == nil {
		writeError(w, req.ID, -32000, fmt.Sprintf("Function '%s' is not an aggregate function", funcName))
		return
	}

	state, ok := store.get(stateID)
	if !ok {
		writeError(w, req.ID, -32000, fmt.Sprintf("State not found: %s", stateID))
		return
	}

	result, err := ref.Aggregate.Evaluate(state)
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
