# Bundlebase Go SDK

Build custom Bundlebase source functions in Go.

## Installation

Add the SDK to your Go module:

```bash
go get github.com/nvoxland/bundlebase/sdk/go/bundlebasesdk
```

## Quick Start

Implement the `SourceFunction` interface:

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

Implement the `SourceFunction` interface:

- **`Discover(attachedLocations []string, args map[string]string)`** - Return available data locations
- **`Data(location Location, args map[string]string)`** - Return Arrow records for a location
- **`StableUrlProvider`** (optional) - Implement to provide stable URLs for locations

Call `Serve(instance)` to start the source function server.

## Documentation

For complete documentation, including advanced usage and API details, see [Custom Source Functions](../../docs/guide/custom-sources/).
