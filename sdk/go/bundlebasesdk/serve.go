package bundlebasesdk

import (
	"bufio"
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"os"
)

// Serve runs the connector as a JSON-RPC subprocess on stdin/stdout.
func Serve(source Connector) {
	ServeIO(source, os.Stdin, os.Stdout)
}

// ServeIO runs the connector on the given reader/writer (for testing).
func ServeIO(source Connector, r io.Reader, w io.Writer) {
	scanner := bufio.NewScanner(r)
	// Allow large lines (up to 16MB)
	scanner.Buffer(make([]byte, 0, 64*1024), 16*1024*1024)

	for scanner.Scan() {
		line := scanner.Bytes()
		if len(line) == 0 {
			continue
		}

		var req jsonRpcRequest
		if err := json.Unmarshal(line, &req); err != nil {
			writeError(w, nil, -32700, fmt.Sprintf("Parse error: %v", err))
			continue
		}

		shouldStop := handleRequest(source, req, w)
		if shouldStop {
			return
		}
	}
}

func handleRequest(source Connector, req jsonRpcRequest, w io.Writer) bool {
	switch req.Method {
	case "handshake":
		writeResponse(w, req.ID, map[string]string{"protocol_version": "1"})
	case "ping":
		writeResponse(w, req.ID, "pong")
	case "discover":
		handleDiscover(source, req, w)
	case "data":
		handleData(source, req, w)
	case "stable_url":
		handleStableUrl(source, req, w)
	case "shutdown":
		writeResponse(w, req.ID, map[string]bool{"ok": true})
		return true
	default:
		writeError(w, req.ID, -32601, fmt.Sprintf("Method not found: %s", req.Method))
	}
	return false
}

func handleDiscover(source Connector, req jsonRpcRequest, w io.Writer) {
	attached := parseStringSlice(req.Params["attached_locations"])
	args := parseStringMap(req.Params, "attached_locations")

	locations, err := source.Discover(attached, args)
	if err != nil {
		writeError(w, req.ID, -32000, err.Error())
		return
	}

	writeResponse(w, req.ID, map[string]interface{}{"locations": locations})
}

func handleData(source Connector, req jsonRpcRequest, w io.Writer) {
	location := parseLocation(req.Params["location"])
	args := parseStringMap(req.Params, "location")

	rawData, err := source.Data(location, args)
	if err != nil {
		writeError(w, req.ID, -32000, err.Error())
		return
	}

	// If the source implements MapConnector, normalize the data through the
	// type-mapping layer so connectors can return plain Go maps/slices.
	records := rawData
	if mc, ok := source.(MapConnector); ok && records != nil {
		records, err = NormalizeToRecords(rawData, mc.Schema())
		if err != nil {
			writeError(w, req.ID, -32000, fmt.Sprintf("failed to normalize data: %v", err))
			return
		}
	}

	// Buffer the Arrow IPC data first so we can send an error instead of an ack
	// if serialization fails.
	var arrowBuf bytes.Buffer
	if err := writeArrowIPC(&arrowBuf, records); err != nil {
		writeError(w, req.ID, -32000, fmt.Sprintf("failed to serialize Arrow IPC data: %v", err))
		return
	}

	writeResponse(w, req.ID, map[string]bool{"ok": true})
	w.Write(arrowBuf.Bytes())
}

func handleStableUrl(source Connector, req jsonRpcRequest, w io.Writer) {
	provider, ok := source.(StableUrlProvider)
	if !ok {
		writeResponse(w, req.ID, nil)
		return
	}

	location := parseLocation(req.Params["location"])
	args := parseStringMap(req.Params, "location")

	result, err := provider.StableUrl(location, args)
	if err != nil {
		writeError(w, req.ID, -32000, err.Error())
		return
	}

	if result == nil {
		writeResponse(w, req.ID, nil)
	} else {
		writeResponse(w, req.ID, map[string]string{"url": result.URL})
	}
}
