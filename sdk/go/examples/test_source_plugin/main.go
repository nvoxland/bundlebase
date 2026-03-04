// Example: Build a plugin shared library for Bundlebase.
//
// Build with:
//
//	go build -buildmode=c-shared -o my_source.so ./examples/test_source_plugin/
//
// Use from Python:
//
//	bundle.create_source("plugin", {"call": "lib:./my_source.so"})
package main

import (
	"C"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/array"
	"github.com/apache/arrow-go/v18/arrow/memory"
	sdk "github.com/nvoxland/bundlebase/sdk/go/bundlebasesdk"
)

type TestSource struct{}

func (s *TestSource) Discover(attached []string, args map[string]string) ([]sdk.Location, error) {
	return []sdk.Location{
		{Location: "test_file_1.parquet", MustCopy: true, Format: "parquet", Version: "v1"},
		{Location: "test_file_2.parquet", MustCopy: true, Format: "parquet", Version: "v1"},
	}, nil
}

func (s *TestSource) Data(location sdk.Location, args map[string]string) ([]arrow.Record, error) {
	alloc := memory.NewGoAllocator()
	schema := arrow.NewSchema([]arrow.Field{
		{Name: "id", Type: arrow.PrimitiveTypes.Int64},
		{Name: "name", Type: arrow.BinaryTypes.String},
	}, nil)

	switch location.Location {
	case "test_file_1.parquet":
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
	case "test_file_2.parquet":
		b := array.NewRecordBuilder(alloc, schema)
		defer b.Release()
		b.Field(0).(*array.Int64Builder).AppendValues([]int64{4, 5}, nil)
		b.Field(1).(*array.StringBuilder).AppendValues([]string{"dave", "eve"}, nil)
		return []arrow.Record{b.NewRecord()}, nil
	}
	return nil, nil
}

// Register the source when the library is loaded.
func init() {
	sdk.ExportSource(&TestSource{})
}

// main is required for c-shared build mode but never called.
func main() {}
