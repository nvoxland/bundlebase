//! Scaffolding for `bundlebase init-sdk` command.
//!
//! Generates a new connector/function project for a given language.

use std::fs;
use std::path::Path;

/// Supported SDK languages.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum SdkLanguage {
    Python,
    Go,
    Java,
    Rust,
}

/// What kind of project to scaffold.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ProjectType {
    Connector,
    Function,
    Both,
}

/// Run the init-sdk scaffolding.
pub fn run(language: SdkLanguage, name: &str, project_type: ProjectType) -> Result<(), String> {
    let root = Path::new(name);
    if root.exists() {
        return Err(format!("Directory '{}' already exists", name));
    }
    fs::create_dir_all(root)
        .map_err(|e| format!("Failed to create directory '{}': {}", name, e))?;

    match language {
        SdkLanguage::Python => scaffold_python(root, name, project_type),
        SdkLanguage::Go => scaffold_go(root, name, project_type),
        SdkLanguage::Java => scaffold_java(root, name, project_type),
        SdkLanguage::Rust => scaffold_rust(root, name, project_type),
    }?;

    println!("Created {} project in '{}'", language_label(language), name);
    println!();
    print_next_steps(language, name, project_type);

    Ok(())
}

fn language_label(lang: SdkLanguage) -> &'static str {
    match lang {
        SdkLanguage::Python => "Python",
        SdkLanguage::Go => "Go",
        SdkLanguage::Java => "Java",
        SdkLanguage::Rust => "Rust",
    }
}

fn print_next_steps(lang: SdkLanguage, name: &str, project_type: ProjectType) {
    println!("Next steps:");
    println!("  cd {}", name);
    match lang {
        SdkLanguage::Python => {
            println!("  pip install -e .");
            match project_type {
                ProjectType::Connector => println!("  python connector.py"),
                ProjectType::Function => println!("  python functions.py"),
                ProjectType::Both => {
                    println!("  python connector.py    # run connector");
                    println!("  python functions.py    # run functions");
                }
            }
        }
        SdkLanguage::Go => {
            println!("  go build -o {}", name);
            println!("  ./{}", name);
        }
        SdkLanguage::Java => {
            println!("  mvn package");
            println!("  java -jar target/{}-1.0.0.jar", name);
        }
        SdkLanguage::Rust => {
            println!("  cargo build");
            println!("  cargo run");
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn write_file(dir: &Path, filename: &str, content: &str) -> Result<(), String> {
    let path = dir.join(filename);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create directory '{}': {}", parent.display(), e))?;
    }
    fs::write(&path, content)
        .map_err(|e| format!("Failed to write '{}': {}", path.display(), e))
}

fn wants_connector(pt: ProjectType) -> bool {
    matches!(pt, ProjectType::Connector | ProjectType::Both)
}

fn wants_function(pt: ProjectType) -> bool {
    matches!(pt, ProjectType::Function | ProjectType::Both)
}

// ---------------------------------------------------------------------------
// Python
// ---------------------------------------------------------------------------

fn scaffold_python(root: &Path, name: &str, pt: ProjectType) -> Result<(), String> {
    // pyproject.toml
    write_file(root, "pyproject.toml", &python_pyproject(name))?;

    if wants_connector(pt) {
        write_file(root, "connector.py", &python_connector(name))?;
    }
    if wants_function(pt) {
        write_file(root, "functions.py", &python_functions(name))?;
    }
    write_file(root, "README.md", &python_readme(name, pt))?;

    Ok(())
}

fn python_pyproject(name: &str) -> String {
    format!(
        r#"[project]
name = "{name}"
version = "0.1.0"
description = "Bundlebase connector/function project"
requires-python = ">=3.10"
dependencies = [
    "bundlebase-sdk>=0.1.0",
    "pyarrow>=14.0.0",
]

[build-system]
requires = ["setuptools>=68.0"]
build-backend = "setuptools.build_meta"
"#
    )
}

fn python_connector(name: &str) -> String {
    let _ = name;
    r#""""Example Bundlebase connector."""

from bundlebase_sdk import Connector, Location, StableUrl, serve


class MyConnector(Connector):
    """A sample connector that serves static data."""

    def discover(self, attached_locations: list[str], **kwargs: str) -> list[Location]:
        return [
            Location(location="sample_data.parquet", must_copy=True, format="parquet", version="v1"),
        ]

    def data(self, location: Location, **kwargs: str):
        import pyarrow as pa

        if location.location == "sample_data.parquet":
            return pa.table({
                "id": [1, 2, 3],
                "name": ["alice", "bob", "charlie"],
                "value": [10.0, 20.0, 30.0],
            })
        return None

    def stable_url(self, location: Location, **kwargs: str):
        return None


if __name__ == "__main__":
    serve(MyConnector())
"#
    .to_string()
}

fn python_functions(_name: &str) -> String {
    r#""""Example Bundlebase function provider."""

import pyarrow as pa
from bundlebase_sdk import Function, serve_function


class MyFunctions(Function):
    """A sample function provider with a scalar and aggregate function."""

    def functions(self) -> list[dict]:
        return [
            {
                "name": "double_value",
                "input_types": ["Int64"],
                "return_type": "Int64",
                "kind": "scalar",
            },
            {
                "name": "sum_values",
                "input_types": ["Int64"],
                "return_type": "Int64",
                "kind": "aggregate",
            },
        ]

    def invoke(self, name: str, batch: pa.RecordBatch) -> pa.RecordBatch:
        if name == "double_value":
            values = batch.column(0).to_pylist()
            result = [v * 2 if v is not None else None for v in values]
            return pa.record_batch({"result": pa.array(result, type=pa.int64())})
        raise ValueError(f"Unknown function: {name}")

    # -- Aggregate: sum_values --

    def create_state(self, name: str) -> object:
        if name == "sum_values":
            return 0
        raise ValueError(f"Unknown aggregate function: {name}")

    def accumulate(self, name: str, state: object, batch: pa.RecordBatch) -> object:
        if name == "sum_values":
            values = batch.column(0).to_pylist()
            return state + sum(v for v in values if v is not None)
        raise ValueError(f"Unknown aggregate function: {name}")

    def merge(self, name: str, state1: object, state2: object) -> object:
        if name == "sum_values":
            return state1 + state2
        raise ValueError(f"Unknown aggregate function: {name}")

    def evaluate(self, name: str, state: object) -> pa.Scalar:
        if name == "sum_values":
            return pa.scalar(state, type=pa.int64())
        raise ValueError(f"Unknown aggregate function: {name}")


if __name__ == "__main__":
    serve_function(MyFunctions())
"#
    .to_string()
}

fn python_readme(name: &str, pt: ProjectType) -> String {
    let mut s = format!(
        "# {name}\n\nA Bundlebase SDK project.\n\n## Setup\n\n```bash\npip install -e .\n```\n\n"
    );
    if wants_connector(pt) {
        s.push_str(
            "## Connector\n\nRun the connector:\n\n```bash\npython connector.py\n```\n\nRegister it in your bundle config:\n\n```yaml\nsources:\n  my_source:\n    connector: python connector.py\n```\n\n",
        );
    }
    if wants_function(pt) {
        s.push_str(
            "## Functions\n\nRun the function provider:\n\n```bash\npython functions.py\n```\n\nRegister it in your bundle config:\n\n```yaml\nfunctions:\n  - command: python functions.py\n```\n\n",
        );
    }
    s
}

// ---------------------------------------------------------------------------
// Go
// ---------------------------------------------------------------------------

fn scaffold_go(root: &Path, name: &str, pt: ProjectType) -> Result<(), String> {
    write_file(root, "go.mod", &go_mod(name))?;
    write_file(root, "main.go", &go_main(name, pt))?;
    write_file(root, "README.md", &go_readme(name, pt))?;

    Ok(())
}

fn go_mod(name: &str) -> String {
    format!(
        r#"module {name}

go 1.22

require (
	github.com/apache/arrow-go/v18 v18.5.1
	github.com/nvoxland/bundlebase/sdk/go v0.0.0
)
"#
    )
}

fn go_main(name: &str, pt: ProjectType) -> String {
    let _ = name;
    let mut s = String::from(
        r#"package main

import (
	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/array"
	"github.com/apache/arrow-go/v18/arrow/memory"
	sdk "github.com/nvoxland/bundlebase/sdk/go/bundlebasesdk"
)

"#,
    );

    if wants_connector(pt) {
        s.push_str(
            r#"// MyConnector implements sdk.Connector.
type MyConnector struct{}

func (c *MyConnector) Discover(attached []string, args map[string]string) ([]sdk.Location, error) {
	return []sdk.Location{
		{Location: "sample_data.parquet", MustCopy: true, Format: "parquet", Version: "v1"},
	}, nil
}

func (c *MyConnector) Data(location sdk.Location, args map[string]string) ([]arrow.Record, error) {
	alloc := memory.NewGoAllocator()
	schema := arrow.NewSchema([]arrow.Field{
		{Name: "id", Type: arrow.PrimitiveTypes.Int64},
		{Name: "name", Type: arrow.BinaryTypes.String},
		{Name: "value", Type: arrow.PrimitiveTypes.Float64},
	}, nil)

	b := array.NewRecordBuilder(alloc, schema)
	defer b.Release()
	b.Field(0).(*array.Int64Builder).AppendValues([]int64{1, 2, 3}, nil)
	b.Field(1).(*array.StringBuilder).AppendValues([]string{"alice", "bob", "charlie"}, nil)
	b.Field(2).(*array.Float64Builder).AppendValues([]float64{10.0, 20.0, 30.0}, nil)

	return []arrow.Record{b.NewRecord()}, nil
}

"#,
        );
    }

    if wants_function(pt) {
        s.push_str(
            r#"// DoubleValue is a scalar function that doubles Int64 values.
type DoubleValue struct{}

func (f *DoubleValue) Invoke(args []arrow.Array) (arrow.Array, error) {
	input := args[0].(*array.Int64)
	alloc := memory.NewGoAllocator()
	builder := array.NewInt64Builder(alloc)
	defer builder.Release()
	for i := 0; i < input.Len(); i++ {
		if input.IsNull(i) {
			builder.AppendNull()
		} else {
			builder.Append(input.Value(i) * 2)
		}
	}
	return builder.NewArray(), nil
}

// SumValues is an aggregate function that sums Int64 values.
type SumValues struct{}

func (f *SumValues) CreateState() (interface{}, error) {
	var sum int64
	return &sum, nil
}

func (f *SumValues) Accumulate(state interface{}, args []arrow.Array) (interface{}, error) {
	sum := state.(*int64)
	input := args[0].(*array.Int64)
	for i := 0; i < input.Len(); i++ {
		if !input.IsNull(i) {
			*sum += input.Value(i)
		}
	}
	return sum, nil
}

func (f *SumValues) Merge(stateA interface{}, stateB interface{}) (interface{}, error) {
	a := stateA.(*int64)
	b := stateB.(*int64)
	*a += *b
	return a, nil
}

func (f *SumValues) Evaluate(state interface{}) (interface{}, error) {
	return *state.(*int64), nil
}

// MyFunctionProvider groups functions together.
type MyFunctionProvider struct{}

func (p *MyFunctionProvider) Functions() map[string]interface{} {
	return map[string]interface{}{
		"double_value": &DoubleValue{},
		"sum_values":   &SumValues{},
	}
}

func (p *MyFunctionProvider) Metadata() sdk.FunctionManifest {
	return sdk.FunctionManifest{
		Functions: []sdk.FunctionMeta{
			{Name: "double_value", InputTypes: []string{"Int64"}, ReturnType: "Int64", Kind: "scalar"},
			{Name: "sum_values", InputTypes: []string{"Int64"}, ReturnType: "Int64", Kind: "aggregate"},
		},
	}
}

"#,
        );
    }

    // main function
    s.push_str("func main() {\n");
    if wants_connector(pt) {
        s.push_str("\tsdk.Serve(&MyConnector{})\n");
    } else if wants_function(pt) {
        s.push_str("\tsdk.ServeFunction(&MyFunctionProvider{})\n");
    }
    s.push_str("}\n");

    s
}

fn go_readme(name: &str, pt: ProjectType) -> String {
    let mut s = format!(
        "# {name}\n\nA Bundlebase SDK project (Go).\n\n## Build\n\n```bash\ngo build -o {name}\n```\n\n## Run\n\n```bash\n./{name}\n```\n\n"
    );
    if wants_connector(pt) {
        s.push_str(&format!(
            "## Connector\n\nRegister in your bundle config:\n\n```yaml\nsources:\n  my_source:\n    connector: ./{name}\n```\n\n"
        ));
    }
    if wants_function(pt) {
        s.push_str(&format!(
            "## Functions\n\nRegister in your bundle config:\n\n```yaml\nfunctions:\n  - command: ./{name}\n```\n\n"
        ));
    }
    s
}

// ---------------------------------------------------------------------------
// Java
// ---------------------------------------------------------------------------

fn scaffold_java(root: &Path, name: &str, pt: ProjectType) -> Result<(), String> {
    let pkg_dir = format!("src/main/java/com/{}", name.replace('-', "_"));
    write_file(root, "pom.xml", &java_pom(name))?;

    if wants_connector(pt) {
        write_file(
            root,
            &format!("{}/MyConnector.java", pkg_dir),
            &java_connector(name),
        )?;
    }
    if wants_function(pt) {
        write_file(
            root,
            &format!("{}/MyFunctions.java", pkg_dir),
            &java_functions(name),
        )?;
    }
    write_file(
        root,
        &format!("{}/Main.java", pkg_dir),
        &java_main(name, pt),
    )?;
    write_file(root, "README.md", &java_readme(name, pt))?;

    Ok(())
}

fn java_pom(name: &str) -> String {
    let pkg = name.replace('-', "_");
    format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<project xmlns="http://maven.apache.org/POM/4.0.0"
         xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
         xsi:schemaLocation="http://maven.apache.org/POM/4.0.0 http://maven.apache.org/xsd/maven-4.0.0.xsd">
    <modelVersion>4.0.0</modelVersion>

    <groupId>com.{pkg}</groupId>
    <artifactId>{name}</artifactId>
    <version>1.0.0</version>
    <packaging>jar</packaging>

    <properties>
        <maven.compiler.source>22</maven.compiler.source>
        <maven.compiler.target>22</maven.compiler.target>
        <project.build.sourceEncoding>UTF-8</project.build.sourceEncoding>
        <arrow.version>18.1.0</arrow.version>
    </properties>

    <dependencies>
        <dependency>
            <groupId>com.bundlebase</groupId>
            <artifactId>bundlebase-sdk</artifactId>
            <version>0.1.0</version>
        </dependency>
        <dependency>
            <groupId>org.apache.arrow</groupId>
            <artifactId>arrow-vector</artifactId>
            <version>${{arrow.version}}</version>
        </dependency>
        <dependency>
            <groupId>org.apache.arrow</groupId>
            <artifactId>arrow-memory-netty</artifactId>
            <version>${{arrow.version}}</version>
            <scope>runtime</scope>
        </dependency>
    </dependencies>

    <build>
        <plugins>
            <plugin>
                <groupId>org.apache.maven.plugins</groupId>
                <artifactId>maven-jar-plugin</artifactId>
                <version>3.3.0</version>
                <configuration>
                    <archive>
                        <manifest>
                            <mainClass>com.{pkg}.Main</mainClass>
                        </manifest>
                    </archive>
                </configuration>
            </plugin>
            <plugin>
                <groupId>org.apache.maven.plugins</groupId>
                <artifactId>maven-compiler-plugin</artifactId>
                <version>3.13.0</version>
                <configuration>
                    <release>22</release>
                </configuration>
            </plugin>
        </plugins>
    </build>
</project>
"##
    )
}

fn java_connector(name: &str) -> String {
    let pkg = name.replace('-', "_");
    format!(
        r#"package com.{pkg};

import com.bundlebase.sdk.*;
import org.apache.arrow.memory.BufferAllocator;
import org.apache.arrow.memory.RootAllocator;
import org.apache.arrow.vector.BigIntVector;
import org.apache.arrow.vector.VarCharVector;
import org.apache.arrow.vector.Float8Vector;
import org.apache.arrow.vector.VectorSchemaRoot;
import org.apache.arrow.vector.types.pojo.ArrowType;
import org.apache.arrow.vector.types.pojo.Field;
import org.apache.arrow.vector.types.pojo.Schema;

import java.util.Arrays;
import java.util.List;
import java.util.Map;

public class MyConnector implements Connector {{
    private final BufferAllocator allocator = new RootAllocator();

    @Override
    public List<Location> discover(List<String> attachedLocations, Map<String, String> args) {{
        return List.of(
            new Location("sample_data.parquet", true, "parquet", "v1")
        );
    }}

    @Override
    public Object data(Location location, Map<String, String> args) {{
        Schema schema = new Schema(Arrays.asList(
            Field.nullable("id", new ArrowType.Int(64, true)),
            Field.nullable("name", new ArrowType.Utf8()),
            Field.nullable("value", new ArrowType.FloatingPoint(ArrowType.FloatingPoint.Precision.DOUBLE))
        ));

        VectorSchemaRoot root = VectorSchemaRoot.create(schema, allocator);
        root.allocateNew();

        BigIntVector idVec = (BigIntVector) root.getVector("id");
        VarCharVector nameVec = (VarCharVector) root.getVector("name");
        Float8Vector valueVec = (Float8Vector) root.getVector("value");

        idVec.setSafe(0, 1); nameVec.setSafe(0, "alice".getBytes()); valueVec.setSafe(0, 10.0);
        idVec.setSafe(1, 2); nameVec.setSafe(1, "bob".getBytes());   valueVec.setSafe(1, 20.0);
        idVec.setSafe(2, 3); nameVec.setSafe(2, "charlie".getBytes()); valueVec.setSafe(2, 30.0);
        root.setRowCount(3);

        return root;
    }}
}}
"#
    )
}

fn java_functions(name: &str) -> String {
    let pkg = name.replace('-', "_");
    format!(
        r#"package com.{pkg};

import com.bundlebase.sdk.Function;
import org.apache.arrow.memory.BufferAllocator;
import org.apache.arrow.memory.RootAllocator;
import org.apache.arrow.vector.BigIntVector;
import org.apache.arrow.vector.FieldVector;

import java.util.List;
import java.util.Map;

public class MyFunctions implements Function.FunctionProvider {{
    private final BufferAllocator allocator = new RootAllocator();

    @Override
    public Map<String, Object> functions() {{
        return Map.of(
            "double_value", (Function.ScalarFunction) this::doubleValue,
            "sum_values", (Function.AggregateFunction<Long>) new SumValues()
        );
    }}

    @Override
    public Function.FunctionManifest metadata() {{
        return new Function.FunctionManifest(List.of(
            new Function.FunctionMeta("double_value", List.of("Int64"), "Int64", "scalar"),
            new Function.FunctionMeta("sum_values", List.of("Int64"), "Int64", "aggregate")
        ));
    }}

    private FieldVector doubleValue(List<FieldVector> args) {{
        BigIntVector input = (BigIntVector) args.get(0);
        BigIntVector result = new BigIntVector("result", allocator);
        result.allocateNew(input.getValueCount());
        for (int i = 0; i < input.getValueCount(); i++) {{
            if (!input.isNull(i)) {{
                result.setSafe(i, input.get(i) * 2);
            }}
        }}
        result.setValueCount(input.getValueCount());
        return result;
    }}

    private static class SumValues implements Function.AggregateFunction<Long> {{
        @Override
        public Long createState() {{
            return 0L;
        }}

        @Override
        public Long accumulate(Long state, List<FieldVector> args) {{
            BigIntVector input = (BigIntVector) args.get(0);
            long sum = state;
            for (int i = 0; i < input.getValueCount(); i++) {{
                if (!input.isNull(i)) {{
                    sum += input.get(i);
                }}
            }}
            return sum;
        }}

        @Override
        public Long merge(Long stateA, Long stateB) {{
            return stateA + stateB;
        }}

        @Override
        public Object evaluate(Long state) {{
            return state;
        }}
    }}
}}
"#
    )
}

fn java_main(name: &str, pt: ProjectType) -> String {
    let pkg = name.replace('-', "_");
    let body = if wants_connector(pt) {
        "        Serve.run(new MyConnector());"
    } else {
        "        Serve.runFunction(new MyFunctions());"
    };

    let imports = if wants_connector(pt) && wants_function(pt) {
        format!(
            "import com.bundlebase.sdk.Serve;\n"
        )
    } else {
        "import com.bundlebase.sdk.Serve;\n".to_string()
    };

    format!(
        r#"package com.{pkg};

{imports}
public class Main {{
    public static void main(String[] args) {{
{body}
    }}
}}
"#
    )
}

fn java_readme(name: &str, pt: ProjectType) -> String {
    let mut s = format!(
        "# {name}\n\nA Bundlebase SDK project (Java).\n\n## Build\n\n```bash\nmvn package\n```\n\n## Run\n\n```bash\njava -jar target/{name}-1.0.0.jar\n```\n\n"
    );
    if wants_connector(pt) {
        s.push_str(&format!(
            "## Connector\n\nRegister in your bundle config:\n\n```yaml\nsources:\n  my_source:\n    connector: java -jar target/{name}-1.0.0.jar\n```\n\n"
        ));
    }
    if wants_function(pt) {
        s.push_str(&format!(
            "## Functions\n\nRegister in your bundle config:\n\n```yaml\nfunctions:\n  - command: java -jar target/{name}-1.0.0.jar\n```\n\n"
        ));
    }
    s
}

// ---------------------------------------------------------------------------
// Rust
// ---------------------------------------------------------------------------

fn scaffold_rust(root: &Path, name: &str, pt: ProjectType) -> Result<(), String> {
    let src = root.join("src");
    fs::create_dir_all(&src)
        .map_err(|e| format!("Failed to create src directory: {}", e))?;

    write_file(root, "Cargo.toml", &rust_cargo_toml(name))?;
    write_file(root, "src/main.rs", &rust_main(name, pt))?;
    write_file(root, "README.md", &rust_readme(name, pt))?;

    Ok(())
}

fn rust_cargo_toml(name: &str) -> String {
    format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2021"

[dependencies]
bundlebase-sdk = "0.1.0"
arrow = "53"
"#
    )
}

fn rust_main(_name: &str, pt: ProjectType) -> String {
    let mut s = String::from(
        r#"use arrow::array::{ArrayRef, Int64Array, StringArray, Float64Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use std::collections::HashMap;
use std::sync::Arc;

"#,
    );

    if wants_connector(pt) {
        s.push_str(
            r#"use bundlebase_sdk::{Connector, Location};

struct MyConnector;

impl Connector for MyConnector {
    fn discover(
        &self,
        _attached: &[String],
        _args: &HashMap<String, String>,
    ) -> Result<Vec<Location>, Box<dyn std::error::Error>> {
        Ok(vec![Location {
            location: "sample_data.parquet".into(),
            must_copy: true,
            format: "parquet".into(),
            version: "v1".into(),
        }])
    }

    fn data(
        &self,
        location: &Location,
        _args: &HashMap<String, String>,
    ) -> Result<Option<Vec<RecordBatch>>, Box<dyn std::error::Error>> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, false),
            Field::new("value", DataType::Float64, false),
        ]));

        match location.location.as_str() {
            "sample_data.parquet" => {
                let batch = RecordBatch::try_new(
                    schema,
                    vec![
                        Arc::new(Int64Array::from(vec![1, 2, 3])),
                        Arc::new(StringArray::from(vec!["alice", "bob", "charlie"])),
                        Arc::new(Float64Array::from(vec![10.0, 20.0, 30.0])),
                    ],
                )?;
                Ok(Some(vec![batch]))
            }
            _ => Ok(None),
        }
    }
}

"#,
        );
    }

    if wants_function(pt) {
        s.push_str(
            r#"use bundlebase_sdk::{ScalarFunction, AggregateFunction, FunctionProvider, FunctionRef, FunctionMeta, FunctionManifest};
use arrow::error::ArrowError;

/// A scalar function that doubles Int64 values.
struct DoubleValue;

impl ScalarFunction for DoubleValue {
    fn invoke(&self, args: &[ArrayRef]) -> Result<ArrayRef, ArrowError> {
        let input = args[0]
            .as_any()
            .downcast_ref::<Int64Array>()
            .ok_or_else(|| ArrowError::InvalidArgumentError("Expected Int64 input".into()))?;
        let result: Int64Array = input.iter().map(|v| v.map(|x| x * 2)).collect();
        Ok(Arc::new(result))
    }
}

/// An aggregate function that sums Int64 values.
struct SumValues;

impl AggregateFunction for SumValues {
    type State = i64;

    fn create_state(&self) -> Result<Self::State, ArrowError> {
        Ok(0)
    }

    fn accumulate(&self, state: &mut Self::State, args: &[ArrayRef]) -> Result<(), ArrowError> {
        let input = args[0]
            .as_any()
            .downcast_ref::<Int64Array>()
            .ok_or_else(|| ArrowError::InvalidArgumentError("Expected Int64 input".into()))?;
        for v in input.iter().flatten() {
            *state += v;
        }
        Ok(())
    }

    fn merge(&self, state_a: &mut Self::State, state_b: Self::State) -> Result<(), ArrowError> {
        *state_a += state_b;
        Ok(())
    }

    fn evaluate(&self, state: &Self::State) -> Result<ArrayRef, ArrowError> {
        Ok(Arc::new(Int64Array::from(vec![*state])))
    }
}

struct MyFunctions {
    double_value: DoubleValue,
    sum_values: SumValues,
}

impl MyFunctions {
    fn new() -> Self {
        Self {
            double_value: DoubleValue,
            sum_values: SumValues,
        }
    }
}

impl FunctionProvider for MyFunctions {
    fn get_function(&self, name: &str) -> Option<FunctionRef<'_>> {
        match name {
            "double_value" => Some(FunctionRef::Scalar(&self.double_value)),
            "sum_values" => Some(FunctionRef::Aggregate(&self.sum_values)),
            _ => None,
        }
    }

    fn metadata(&self) -> FunctionManifest {
        FunctionManifest {
            functions: vec![
                FunctionMeta {
                    name: "double_value".into(),
                    input_types: vec!["Int64".into()],
                    return_type: "Int64".into(),
                    kind: "scalar".into(),
                    symbol: None,
                },
                FunctionMeta {
                    name: "sum_values".into(),
                    input_types: vec!["Int64".into()],
                    return_type: "Int64".into(),
                    kind: "aggregate".into(),
                    symbol: None,
                },
            ],
        }
    }
}

"#,
        );
    }

    // main function
    s.push_str("fn main() {\n");
    if wants_connector(pt) {
        s.push_str("    bundlebase_sdk::serve(&MyConnector);\n");
    } else if wants_function(pt) {
        s.push_str("    bundlebase_sdk::serve_function(&MyFunctions::new());\n");
    }
    s.push_str("}\n");

    s
}

fn rust_readme(name: &str, pt: ProjectType) -> String {
    let mut s = format!(
        "# {name}\n\nA Bundlebase SDK project (Rust).\n\n## Build\n\n```bash\ncargo build\n```\n\n## Run\n\n```bash\ncargo run\n```\n\n"
    );
    if wants_connector(pt) {
        s.push_str(&format!(
            "## Connector\n\nRegister in your bundle config:\n\n```yaml\nsources:\n  my_source:\n    connector: ./target/debug/{name}\n```\n\n"
        ));
    }
    if wants_function(pt) {
        s.push_str(&format!(
            "## Functions\n\nRegister in your bundle config:\n\n```yaml\nfunctions:\n  - command: ./target/debug/{name}\n```\n\n"
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(suffix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bundlebase_init_sdk_test_{}", suffix));
        if dir.exists() {
            fs::remove_dir_all(&dir).expect("cleanup");
        }
        dir
    }

    #[test]
    fn test_scaffold_python_both() {
        let dir = temp_dir("py_both");
        fs::create_dir_all(&dir).expect("create");
        scaffold_python(&dir, "my_connector", ProjectType::Both).expect("scaffold");
        assert!(dir.join("pyproject.toml").exists());
        assert!(dir.join("connector.py").exists());
        assert!(dir.join("functions.py").exists());
        assert!(dir.join("README.md").exists());
        fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn test_scaffold_python_connector_only() {
        let dir = temp_dir("py_conn");
        fs::create_dir_all(&dir).expect("create");
        scaffold_python(&dir, "my_connector", ProjectType::Connector).expect("scaffold");
        assert!(dir.join("pyproject.toml").exists());
        assert!(dir.join("connector.py").exists());
        assert!(!dir.join("functions.py").exists());
        fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn test_scaffold_python_function_only() {
        let dir = temp_dir("py_func");
        fs::create_dir_all(&dir).expect("create");
        scaffold_python(&dir, "my_connector", ProjectType::Function).expect("scaffold");
        assert!(dir.join("pyproject.toml").exists());
        assert!(!dir.join("connector.py").exists());
        assert!(dir.join("functions.py").exists());
        fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn test_scaffold_go_both() {
        let dir = temp_dir("go_both");
        fs::create_dir_all(&dir).expect("create");
        scaffold_go(&dir, "my_connector", ProjectType::Both).expect("scaffold");
        assert!(dir.join("go.mod").exists());
        assert!(dir.join("main.go").exists());
        assert!(dir.join("README.md").exists());
        fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn test_scaffold_java_both() {
        let dir = temp_dir("java_both");
        fs::create_dir_all(&dir).expect("create");
        scaffold_java(&dir, "my_connector", ProjectType::Both).expect("scaffold");
        assert!(dir.join("pom.xml").exists());
        assert!(dir
            .join("src/main/java/com/my_connector/MyConnector.java")
            .exists());
        assert!(dir
            .join("src/main/java/com/my_connector/MyFunctions.java")
            .exists());
        assert!(dir
            .join("src/main/java/com/my_connector/Main.java")
            .exists());
        fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn test_scaffold_rust_both() {
        let dir = temp_dir("rust_both");
        fs::create_dir_all(&dir).expect("create");
        scaffold_rust(&dir, "my_connector", ProjectType::Both).expect("scaffold");
        assert!(dir.join("Cargo.toml").exists());
        assert!(dir.join("src/main.rs").exists());
        assert!(dir.join("README.md").exists());
        fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn test_run_rejects_existing_dir() {
        let dir = temp_dir("exists");
        fs::create_dir_all(&dir).expect("create");
        // run() checks if dir exists, so pass the dir path as name
        let result = run(
            SdkLanguage::Python,
            dir.to_str().expect("path"),
            ProjectType::Both,
        );
        assert!(result.is_err());
        assert!(result
            .expect_err("should error")
            .contains("already exists"));
        fs::remove_dir_all(&dir).expect("cleanup");
    }
}
