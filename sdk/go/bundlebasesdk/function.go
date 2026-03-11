package bundlebasesdk

import "github.com/apache/arrow-go/v18/arrow"

// ScalarFunction is the interface for implementing a custom scalar UDF.
type ScalarFunction interface {
	// Invoke applies the function to the given input arrays, returning an output array.
	Invoke(args []arrow.Array) (arrow.Array, error)
}

// AggregateFunction is the interface for implementing a custom aggregate UDF.
type AggregateFunction interface {
	// CreateState returns a new accumulator state (can be any type, stored opaquely).
	CreateState() (interface{}, error)

	// Accumulate adds data from input arrays into the accumulator state.
	Accumulate(state interface{}, args []arrow.Array) (interface{}, error)

	// Merge combines two accumulator states into one.
	Merge(stateA interface{}, stateB interface{}) (interface{}, error)

	// Evaluate produces a final scalar result from the accumulator state.
	Evaluate(state interface{}) (interface{}, error)
}

// FunctionProvider groups functions together with metadata for discovery.
type FunctionProvider interface {
	// Functions returns the available functions.
	Functions() map[string]interface{} // values are ScalarFunction or AggregateFunction

	// Metadata returns the function metadata for auto-discovery.
	Metadata() FunctionManifest
}

// FunctionManifest describes all functions in a provider for auto-detection.
type FunctionManifest struct {
	Functions []FunctionMeta `json:"functions"`
}

// FunctionMeta describes a single function for auto-detection.
type FunctionMeta struct {
	Name       string   `json:"name"`
	InputTypes []string `json:"input_types"`
	ReturnType string   `json:"return_type"`
	Kind       string   `json:"kind"` // "scalar" or "aggregate"
	Symbol     string   `json:"symbol,omitempty"`
}
