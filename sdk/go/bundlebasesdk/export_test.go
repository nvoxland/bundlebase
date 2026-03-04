package bundlebasesdk

import (
	"testing"
)

func TestParseExportArgs(t *testing.T) {
	attached, args, err := parseExportArgs(`{
		"attached_locations": ["loc1", "loc2"],
		"key1": "val1",
		"key2": "val2"
	}`)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(attached) != 2 {
		t.Fatalf("expected 2 attached, got %d", len(attached))
	}
	if attached[0] != "loc1" || attached[1] != "loc2" {
		t.Errorf("unexpected attached: %v", attached)
	}
	if args["key1"] != "val1" || args["key2"] != "val2" {
		t.Errorf("unexpected args: %v", args)
	}
	// attached_locations should not appear in args
	if _, ok := args["attached_locations"]; ok {
		t.Error("attached_locations should be excluded from args")
	}
}

func TestParseExportArgsEmpty(t *testing.T) {
	attached, args, err := parseExportArgs(`{}`)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(attached) != 0 {
		t.Errorf("expected empty attached, got %v", attached)
	}
	if len(args) != 0 {
		t.Errorf("expected empty args, got %v", args)
	}
}

func TestParseExportArgsInvalidJSON(t *testing.T) {
	_, _, err := parseExportArgs("not json")
	if err == nil {
		t.Error("expected error for invalid JSON")
	}
}

func TestParseExportArgsNonStringValuesSkipped(t *testing.T) {
	_, args, err := parseExportArgs(`{
		"str_key": "value",
		"int_key": 42,
		"bool_key": true
	}`)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if args["str_key"] != "value" {
		t.Error("string value should be included")
	}
	if _, ok := args["int_key"]; ok {
		t.Error("int value should be skipped")
	}
	if _, ok := args["bool_key"]; ok {
		t.Error("bool value should be skipped")
	}
}

func TestParseExportLocation(t *testing.T) {
	loc, err := parseExportLocation(`{
		"location": "data.parquet",
		"must_copy": false,
		"format": "csv",
		"version": "v2"
	}`)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if loc.Location != "data.parquet" {
		t.Errorf("expected data.parquet, got %s", loc.Location)
	}
	if loc.MustCopy != false {
		t.Error("expected must_copy=false")
	}
	if loc.Format != "csv" {
		t.Errorf("expected csv, got %s", loc.Format)
	}
	if loc.Version != "v2" {
		t.Errorf("expected v2, got %s", loc.Version)
	}
}

func TestParseExportLocationDefaults(t *testing.T) {
	loc, err := parseExportLocation(`{"location": "file.parquet"}`)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if loc.Location != "file.parquet" {
		t.Errorf("expected file.parquet, got %s", loc.Location)
	}
	// Format default is set by parseExportLocation
	if loc.Format != "parquet" {
		t.Errorf("expected default format parquet, got %s", loc.Format)
	}
}

func TestParseExportLocationInvalidJSON(t *testing.T) {
	_, err := parseExportLocation("not json")
	if err == nil {
		t.Error("expected error for invalid JSON")
	}
}

func TestParseSimpleArgs(t *testing.T) {
	args, err := parseSimpleArgs(`{"k1": "v1", "k2": "v2", "num": 42}`)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if args["k1"] != "v1" || args["k2"] != "v2" {
		t.Errorf("unexpected args: %v", args)
	}
	// Non-string values are skipped
	if _, ok := args["num"]; ok {
		t.Error("non-string value should be skipped")
	}
}

func TestExportSourceRegistration(t *testing.T) {
	src := &testSource{}

	// Before registration
	exportMu.Lock()
	exportedSource = nil
	exportMu.Unlock()

	if getExportedSource() != nil {
		t.Error("expected nil before registration")
	}

	ExportSource(src)

	if getExportedSource() == nil {
		t.Error("expected non-nil after registration")
	}

	// Clean up
	exportMu.Lock()
	exportedSource = nil
	exportMu.Unlock()
}

func TestNewRecordReaderEmpty(t *testing.T) {
	reader := newRecordReader(nil)
	if reader != nil {
		t.Error("expected nil reader for empty slice")
	}
}
