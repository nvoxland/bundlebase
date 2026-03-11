package bundlebasesdk

import (
	"fmt"
	"sort"
	"strings"

	"github.com/apache/arrow-go/v18/arrow"
)

// Location represents a discovered data location returned from Discover().
type Location struct {
	Location string `json:"location"`
	MustCopy bool   `json:"must_copy"`
	Format   string `json:"format"`
	Version  string `json:"version"`
}

// StableUrl represents a stable URL for a data location.
type StableUrl struct {
	URL string `json:"url"`
}

// TypeMap maps PascalCase type name strings to Arrow DataType values.
var TypeMap = map[string]arrow.DataType{
	"Boolean":     arrow.FixedWidthTypes.Boolean,
	"Int8":        arrow.PrimitiveTypes.Int8,
	"Int16":       arrow.PrimitiveTypes.Int16,
	"Int32":       arrow.PrimitiveTypes.Int32,
	"Int64":       arrow.PrimitiveTypes.Int64,
	"UInt8":       arrow.PrimitiveTypes.Uint8,
	"UInt16":      arrow.PrimitiveTypes.Uint16,
	"UInt32":      arrow.PrimitiveTypes.Uint32,
	"UInt64":      arrow.PrimitiveTypes.Uint64,
	"Float16":     arrow.FixedWidthTypes.Float16,
	"Float32":     arrow.PrimitiveTypes.Float32,
	"Float64":     arrow.PrimitiveTypes.Float64,
	"Utf8":        arrow.BinaryTypes.String,
	"LargeUtf8":   arrow.BinaryTypes.LargeString,
	"Binary":      arrow.BinaryTypes.Binary,
	"LargeBinary": arrow.BinaryTypes.LargeBinary,
	"Date32":      arrow.FixedWidthTypes.Date32,
	"Date64":      arrow.FixedWidthTypes.Date64,
	"Timestamp":   arrow.FixedWidthTypes.Timestamp_us,
}

// ResolveType looks up the Arrow DataType for a type name string.
// Returns an error if the type name is unknown.
func ResolveType(name string) (arrow.DataType, error) {
	dt, ok := TypeMap[name]
	if !ok {
		names := make([]string, 0, len(TypeMap))
		for k := range TypeMap {
			names = append(names, k)
		}
		sort.Strings(names)
		return nil, fmt.Errorf("unknown type %q; supported types: %s", name, strings.Join(names, ", "))
	}
	return dt, nil
}

// SchemaFromTypes builds an Arrow schema from a map of column names to type name strings.
func SchemaFromTypes(columns map[string]string) (*arrow.Schema, error) {
	fields := make([]arrow.Field, 0, len(columns))

	// Sort keys for deterministic field order.
	keys := make([]string, 0, len(columns))
	for k := range columns {
		keys = append(keys, k)
	}
	sort.Strings(keys)

	for _, colName := range keys {
		typeName := columns[colName]
		dt, err := ResolveType(typeName)
		if err != nil {
			return nil, fmt.Errorf("column %q: %w", colName, err)
		}
		fields = append(fields, arrow.Field{Name: colName, Type: dt, Nullable: true})
	}
	return arrow.NewSchema(fields, nil), nil
}
