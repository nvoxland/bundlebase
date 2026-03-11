package bundlebasesdk

import "github.com/apache/arrow-go/v18/arrow"

// Connector is the interface for implementing a custom Bundlebase source.
// Implement Discover and Data. Optionally implement StableUrlProvider for stable URLs.
type Connector interface {
	// Discover returns the available data locations.
	Discover(attachedLocations []string, args map[string]string) ([]Location, error)

	// Data returns Arrow record batches for the given location.
	// Return nil for no data.
	//
	// The args map may contain reserved keys prefixed with "_".
	// Currently defined:
	//   - "_columns": comma-separated column names the caller wants.
	//     Connectors that support column pushdown can parse this to
	//     return only the requested columns. It is safe to ignore.
	Data(location Location, args map[string]string) ([]arrow.Record, error)
}

// MapConnector is an optional interface for connectors that return data as Go
// maps/slices instead of Arrow records. Implementations provide a Schema() that
// maps column names to type name strings (see TypeMap), and Data() returns a
// generic value that NormalizeToRecords converts into Arrow records.
type MapConnector interface {
	Connector

	// Schema returns a map of column names to type name strings.
	Schema() map[string]string
}

// StableUrlProvider is an optional interface for sources that provide stable URLs.
type StableUrlProvider interface {
	StableUrl(location Location, args map[string]string) (*StableUrl, error)
}
