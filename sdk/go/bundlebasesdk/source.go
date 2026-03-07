package bundlebasesdk

import "github.com/apache/arrow-go/v18/arrow"

// Connector is the interface for implementing a custom Bundlebase source.
// Implement Discover and Data. Optionally implement StableUrlProvider for stable URLs.
type Connector interface {
	// Discover returns the available data locations.
	Discover(attachedLocations []string, args map[string]string) ([]Location, error)

	// Data returns Arrow record batches for the given location.
	// Return nil for no data.
	Data(location Location, args map[string]string) ([]arrow.Record, error)
}

// StableUrlProvider is an optional interface for sources that provide stable URLs.
type StableUrlProvider interface {
	StableUrl(location Location, args map[string]string) (*StableUrl, error)
}
