package bundlebasesdk

import (
	"bufio"
	"encoding/json"
	"fmt"
	"io"
	"os"
)

// Serve runs the source function as a JSON-RPC subprocess on stdin/stdout.
func Serve(source SourceFunction) {
	ServeIO(source, os.Stdin, os.Stdout)
}

// ServeIO runs the source function on the given reader/writer (for testing).
func ServeIO(source SourceFunction, r io.Reader, w io.Writer) {
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
			continue
		}

		shouldStop := handleRequest(source, req, w)
		if shouldStop {
			return
		}
	}
}

func handleRequest(source SourceFunction, req jsonRpcRequest, w io.Writer) bool {
	switch req.Method {
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

func handleDiscover(source SourceFunction, req jsonRpcRequest, w io.Writer) {
	attached := parseStringSlice(req.Params["attached_locations"])
	args := parseStringMap(req.Params, "attached_locations")

	locations, err := source.Discover(attached, args)
	if err != nil {
		writeError(w, req.ID, -32000, err.Error())
		return
	}

	writeResponse(w, req.ID, map[string]interface{}{"locations": locations})
}

func handleData(source SourceFunction, req jsonRpcRequest, w io.Writer) {
	location := parseLocation(req.Params["location"])
	args := parseStringMap(req.Params, "location")

	records, err := source.Data(location, args)
	if err != nil {
		writeError(w, req.ID, -32000, err.Error())
		return
	}

	writeResponse(w, req.ID, map[string]bool{"ok": true})

	if err := writeArrowIPC(w, records); err != nil {
		// Can't send JSON-RPC error after we've already sent the ack
		fmt.Fprintf(os.Stderr, "failed to write Arrow IPC data: %v\n", err)
	}
}

func handleStableUrl(source SourceFunction, req jsonRpcRequest, w io.Writer) {
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
