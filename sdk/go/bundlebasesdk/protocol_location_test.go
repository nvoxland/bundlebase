package bundlebasesdk

import "testing"

// bb-cko: parseLocation must default must_copy to true (matching the
// Python/Rust/Java SDKs) and preserve num_rows when the host sends it.
func TestParseLocationDefaults(t *testing.T) {
	// must_copy absent -> defaults to true.
	loc := parseLocation(map[string]interface{}{"location": "data.parquet"})
	if !loc.MustCopy {
		t.Errorf("must_copy should default to true when absent, got false")
	}
	if loc.NumRows != nil {
		t.Errorf("num_rows should be nil when absent, got %v", *loc.NumRows)
	}

	// must_copy explicitly false is honored.
	loc = parseLocation(map[string]interface{}{"location": "d", "must_copy": false})
	if loc.MustCopy {
		t.Errorf("explicit must_copy=false should be honored")
	}

	// num_rows is parsed (JSON numbers decode to float64).
	loc = parseLocation(map[string]interface{}{"location": "d", "num_rows": float64(1234)})
	if loc.NumRows == nil || *loc.NumRows != 1234 {
		t.Errorf("num_rows should be 1234, got %v", loc.NumRows)
	}
}
