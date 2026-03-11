# Bundlebase Go SDK

Build custom Bundlebase connectors in Go.

## Installation

Add the SDK to your Go module:

```bash
go get github.com/nvoxland/bundlebase/sdk/go/bundlebasesdk
```

## Quick Start

Implement the `Connector` interface:

```go
package main

import (
	sdk "github.com/nvoxland/bundlebase/sdk/go/bundlebasesdk"
	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/array"
	"github.com/apache/arrow-go/v18/arrow/memory"
)

type MySource struct{}

func (s *MySource) Discover(attachedLocations []string, args map[string]string) ([]sdk.Location, error) {
	return []sdk.Location{
		{Location: "data1.parquet", MustCopy: true, Format: "parquet", Version: "v1"},
		{Location: "data2.parquet", MustCopy: true, Format: "parquet", Version: "v1"},
	}, nil
}

func (s *MySource) Data(location sdk.Location, args map[string]string) ([]arrow.Record, error) {
	// Return Arrow records for the location
	alloc := memory.NewGoAllocator()
	schema := arrow.NewSchema([]arrow.Field{
		{Name: "id", Type: arrow.PrimitiveTypes.Int64},
		{Name: "name", Type: arrow.BinaryTypes.String},
	}, nil)
	// Build and return records...
	return records, nil
}

func main() {
	sdk.Serve(&MySource())
}
```

## Implementation

Implement the `Connector` interface:

- **`Discover(attachedLocations []string, args map[string]string)`** - Return available data locations
- **`Data(location Location, args map[string]string)`** - Return Arrow records for a location
- **`StableUrlProvider`** (optional) - Implement to provide stable URLs for locations

Call `Serve(instance)` to start the connector server.

### MapConnector

For a simpler API, implement `MapConnector` instead. It extends `Connector` with a `Schema()` method that maps column names to type strings, letting you return `[]map[string]interface{}` from `Data()` instead of building Arrow records manually:

```go
type MySource struct{}

func (s *MySource) Schema() map[string]string {
	return map[string]string{
		"id":   "Int64",
		"name": "Utf8",
	}
}

func (s *MySource) Discover(attached []string, args map[string]string) ([]sdk.Location, error) {
	return []sdk.Location{{Location: "all", Version: "v1"}}, nil
}

func (s *MySource) Data(loc sdk.Location, args map[string]string) ([]arrow.Record, error) {
	data := []map[string]interface{}{
		{"id": int64(1), "name": "Alice"},
		{"id": int64(2), "name": "Bob"},
	}
	return sdk.NormalizeToRecords(data, s.Schema())
}
```

The SDK's `NormalizeToRecords` helper converts Go maps to Arrow records using the schema.

## Documentation

For complete documentation, including advanced usage and API details, see [Custom Connectors](../../docs/guide/custom-connectors/).
