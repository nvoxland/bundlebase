package bundlebasesdk

/*
#include <stdlib.h>
#include <stdint.h>

// Arrow C Data Interface — ArrowArrayStream
struct ArrowSchema {
	const char* format;
	const char* name;
	const char* metadata;
	int64_t flags;
	int64_t n_children;
	struct ArrowSchema** children;
	struct ArrowSchema* dictionary;
	void (*release)(struct ArrowSchema*);
	void* private_data;
};

struct ArrowArray {
	int64_t length;
	int64_t null_count;
	int64_t offset;
	int64_t n_buffers;
	int64_t n_children;
	const void** buffers;
	struct ArrowArray** children;
	struct ArrowArray* dictionary;
	void (*release)(struct ArrowArray*);
	void* private_data;
};

struct ArrowArrayStream {
	int (*get_schema)(struct ArrowArrayStream*, struct ArrowSchema* out);
	int (*get_next)(struct ArrowArrayStream*, struct ArrowArray* out);
	const char* (*get_last_error)(struct ArrowArrayStream*);
	void (*release)(struct ArrowArrayStream*);
	void* private_data;
};
*/
import "C"

import (
	"encoding/json"
	"fmt"
	"sync"
	"unsafe"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/array"
	"github.com/apache/arrow-go/v18/arrow/cdata"
)

var (
	exportedConnector Connector
	exportMu       sync.Mutex
)

// newRecordReader creates an array.RecordReader from a slice of records.
// Returns nil if the slice is empty.
func newRecordReader(records []arrow.Record) array.RecordReader {
	if len(records) == 0 {
		return nil
	}
	// Convert []arrow.Record to []arrow.RecordBatch (same underlying type)
	batches := make([]arrow.RecordBatch, len(records))
	for i, r := range records {
		batches[i] = r
	}
	reader, err := array.NewRecordReader(records[0].Schema(), batches)
	if err != nil {
		return nil
	}
	return reader
}

// ExportConnector registers a Connector for use as a plugin shared library.
//
// Call this from your main() or init() before the library is used.
// The source is stored globally and used by the exported C functions.
//
// Example:
//
//	func init() {
//	    bundlebasesdk.ExportConnector(&MySource{})
//	}
func ExportConnector(source Connector) {
	exportMu.Lock()
	defer exportMu.Unlock()
	exportedConnector = source
}

func getExportedConnector() Connector {
	exportMu.Lock()
	defer exportMu.Unlock()
	return exportedConnector
}

// parseExportArgs parses the JSON args string into attached locations and args map.
func parseExportArgs(argsJSON string) ([]string, map[string]string, error) {
	var raw map[string]interface{}
	if err := json.Unmarshal([]byte(argsJSON), &raw); err != nil {
		return nil, nil, fmt.Errorf("invalid JSON: %w", err)
	}

	var attached []string
	if v, ok := raw["attached_locations"]; ok {
		if arr, ok := v.([]interface{}); ok {
			for _, item := range arr {
				if s, ok := item.(string); ok {
					attached = append(attached, s)
				}
			}
		}
	}

	args := make(map[string]string)
	for k, v := range raw {
		if k == "attached_locations" {
			continue
		}
		if s, ok := v.(string); ok {
			args[k] = s
		}
	}

	return attached, args, nil
}

// parseExportLocation parses a JSON location string into a Location.
func parseExportLocation(locationJSON string) (Location, error) {
	var loc Location
	loc.Format = "parquet"
	if err := json.Unmarshal([]byte(locationJSON), &loc); err != nil {
		return loc, fmt.Errorf("invalid location JSON: %w", err)
	}
	return loc, nil
}

// parseSimpleArgs parses JSON into a string map (no attached_locations).
func parseSimpleArgs(argsJSON string) (map[string]string, error) {
	var raw map[string]interface{}
	if err := json.Unmarshal([]byte(argsJSON), &raw); err != nil {
		return nil, fmt.Errorf("invalid JSON: %w", err)
	}
	args := make(map[string]string)
	for k, v := range raw {
		if s, ok := v.(string); ok {
			args[k] = s
		}
	}
	return args, nil
}

//export bundlebase_discover
func bundlebase_discover(argsJSON *C.char, outJSON **C.char) C.int32_t {
	source := getExportedConnector()
	if source == nil {
		if outJSON != nil {
			*outJSON = C.CString("No source registered. Call ExportConnector() first.")
		}
		return -1
	}

	attached, args, err := parseExportArgs(C.GoString(argsJSON))
	if err != nil {
		if outJSON != nil {
			*outJSON = C.CString(err.Error())
		}
		return -1
	}

	locations, err := source.Discover(attached, args)
	if err != nil {
		if outJSON != nil {
			*outJSON = C.CString(err.Error())
		}
		return -1
	}

	response := map[string]interface{}{"locations": locations}
	data, err := json.Marshal(response)
	if err != nil {
		if outJSON != nil {
			*outJSON = C.CString(err.Error())
		}
		return -1
	}

	if outJSON != nil {
		*outJSON = C.CString(string(data))
	}
	return 0
}

//export bundlebase_data
func bundlebase_data(locationJSON *C.char, argsJSON *C.char, out *C.struct_ArrowArrayStream) C.int32_t {
	source := getExportedConnector()
	if source == nil {
		return -1
	}

	loc, err := parseExportLocation(C.GoString(locationJSON))
	if err != nil {
		return -1
	}

	args, err := parseSimpleArgs(C.GoString(argsJSON))
	if err != nil {
		return -1
	}

	records, err := source.Data(loc, args)
	if err != nil {
		return -1
	}

	if len(records) == 0 || out == nil {
		return 0
	}

	// Export records via Arrow C Data Interface
	reader := newRecordReader(records)
	if reader == nil {
		return 0
	}
	cdata.ExportRecordReader(
		reader,
		(*cdata.CArrowArrayStream)(unsafe.Pointer(out)),
	)

	return 0
}

//export bundlebase_free
func bundlebase_free(ptr *C.char) {
	if ptr != nil {
		C.free(unsafe.Pointer(ptr))
	}
}

//export bundlebase_stable_url
func bundlebase_stable_url(locationJSON *C.char, argsJSON *C.char, outJSON **C.char) C.int32_t {
	source := getExportedConnector()
	if source == nil {
		return -1
	}

	provider, ok := source.(StableUrlProvider)
	if !ok {
		return 0 // No stable URL support
	}

	loc, err := parseExportLocation(C.GoString(locationJSON))
	if err != nil {
		return -1
	}

	args, err := parseSimpleArgs(C.GoString(argsJSON))
	if err != nil {
		return -1
	}

	result, err := provider.StableUrl(loc, args)
	if err != nil {
		if outJSON != nil {
			*outJSON = C.CString(err.Error())
		}
		return -1
	}

	if result == nil {
		return 0
	}

	data, err := json.Marshal(map[string]string{"url": result.URL})
	if err != nil {
		return -1
	}

	if outJSON != nil {
		*outJSON = C.CString(string(data))
	}
	return 0
}
