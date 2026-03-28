// Benchmark IPC function server for Go. Implements double_val (scalar) and int_sum (aggregate).
package main

import (
	"os"

	sdk "github.com/bundlebase/bundlebase/sdk/go/bundlebasesdk"
	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/array"
	"github.com/apache/arrow-go/v18/arrow/memory"
)

type doubleVal struct{}

func (d *doubleVal) Invoke(batch arrow.Record) (arrow.Array, error) {
	input := batch.Column(0).(*array.Int64)
	alloc := memory.NewGoAllocator()
	b := array.NewInt64Builder(alloc)
	for i := 0; i < input.Len(); i++ {
		b.Append(input.Value(i) * 2)
	}
	return b.NewArray(), nil
}

type intSum struct{}

func (s *intSum) CreateState() (interface{}, error) {
	return int64(0), nil
}

func (s *intSum) Accumulate(state interface{}, batch arrow.Record) (interface{}, error) {
	sum := state.(int64)
	input := batch.Column(0).(*array.Int64)
	for i := 0; i < input.Len(); i++ {
		sum += input.Value(i)
	}
	return sum, nil
}

func (s *intSum) Merge(stateA interface{}, stateB interface{}) (interface{}, error) {
	return stateA.(int64) + stateB.(int64), nil
}

func (s *intSum) Evaluate(state interface{}) (interface{}, error) {
	return state.(int64), nil
}

type benchProvider struct{}

func (p *benchProvider) Functions() map[string]sdk.FunctionRef {
	return map[string]sdk.FunctionRef{
		"double_val": {Scalar: &doubleVal{}},
		"int_sum":    {Aggregate: &intSum{}},
	}
}

func (p *benchProvider) Metadata() sdk.FunctionManifest {
	return sdk.FunctionManifest{
		Functions: []sdk.FunctionMeta{
			{Name: "double_val", InputTypes: []string{"Int64"}, ReturnType: "Int64", Kind: "scalar"},
			{Name: "int_sum", InputTypes: []string{"Int64"}, ReturnType: "Int64", Kind: "aggregate"},
		},
	}
}

func main() {
	_ = os.Stderr // Suppress unused import
	sdk.ServeFunction(&benchProvider{})
}
