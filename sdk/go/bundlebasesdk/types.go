package bundlebasesdk

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
