use bundlebase_ossie::{
    parse_document, parse_model, to_json, to_yaml, validate_document, DocumentFormat, OssieError,
    SemanticModel, SCHEMA_JSON, SPEC_VERSION,
};
use serde_json::{json, Value};

const EXAMPLES: &[&str] = &[
    include_str!("../vendor/examples/flights.semantic_model.yaml"),
    include_str!("../vendor/examples/tpcds_semantic_model.yaml"),
];

fn minimal() -> Value {
    json!({
        "version": SPEC_VERSION,
        "name": "retail",
        "datasets": [{"name": "sales", "source": "catalog.public.sales"}]
    })
}

#[test]
fn upstream_examples_round_trip_without_field_loss() -> Result<(), Box<dyn std::error::Error>> {
    for example in EXAMPLES {
        let original: Value = serde_yaml_ng::from_str(example)?;
        let document = parse_document(example, DocumentFormat::Yaml)?;
        assert_eq!(document, original);

        let typed = parse_model(example, DocumentFormat::Auto)?;
        assert_eq!(typed.to_value()?, original);
        assert_eq!(
            parse_document(&to_json(&document)?, DocumentFormat::Json)?,
            original
        );
        assert_eq!(
            parse_document(&to_yaml(&document)?, DocumentFormat::Yaml)?,
            original
        );
        assert_eq!(parse_model(&typed.to_json()?, DocumentFormat::Json)?, typed);
        assert_eq!(parse_model(&typed.to_yaml()?, DocumentFormat::Yaml)?, typed);
    }
    Ok(())
}

#[test]
fn full_standard_shapes_preserve_ai_and_vendor_extensions() -> Result<(), Box<dyn std::error::Error>>
{
    let mut document = minimal();
    document["ai_context"] = json!({
        "instructions": "Use the retail model",
        "synonyms": [],
        "examples": ["How many sales?"],
        "arbitrary_vendor_key": {"nested": [null, true, 12]}
    });
    document["custom_extensions"] = json!([{
        "vendor_name": "MY_NEW_VENDOR",
        "data": "{\"arbitrary\":true}"
    }]);
    document["datasets"][0]["ai_context"] = json!("Free-text context");
    document["datasets"][0]["primary_key"] = json!(["id"]);
    document["datasets"][0]["unique_keys"] = json!([["id"], ["order_id", "line_id"]]);
    document["datasets"][0]["fields"] = json!([{
        "name": "date",
        "expression": {"dialects": [{"dialect": "OSSIE_SQL_2026", "expression": "sale_date"}]},
        "dimension": {"is_time": false},
        "label": "Audit timestamp",
        "description": "When a sale was recorded",
        "datatype": "DateTimeTz",
        "custom_extensions": []
    }]);
    document["relationships"] = json!([{
        "name": "sales_customer",
        "from": "sales", "to": "customers",
        "from_columns": ["customer_id"], "to_columns": ["id"],
        "ai_context": {"examples": []}, "custom_extensions": []
    }]);
    document["metrics"] = json!([{
        "name": "sales_total",
        "expression": {"dialects": [
            {"dialect": "ANSI_SQL", "expression": "SUM(sales.amount)"},
            {"dialect": "DAX", "expression": "SUM(sales[amount])"}
        ]},
        "datatype": "Decimal", "description": "Total sales",
        "ai_context": {"synonyms": ["revenue"]}, "custom_extensions": []
    }]);
    let typed = SemanticModel::from_value(document.clone())?;
    assert_eq!(typed.to_value()?, document);
    assert_eq!(
        parse_document(&typed.to_yaml()?, DocumentFormat::Auto)?,
        document
    );
    Ok(())
}

#[test]
fn rejects_versions_unknown_fields_and_wrong_extension_shape() {
    let cases = [
        ("/version", json!("999.0")),
        ("/name", json!("")),
        ("/datasets", json!([])),
        (
            "/custom_extensions",
            json!([{"vendor": "BUNDLEBASE", "data": {}}]),
        ),
        ("/ai_context", json!({"synonyms": 42})),
    ];
    for (pointer, replacement) in cases {
        let mut document = minimal();
        if let Some(key) = pointer.strip_prefix('/') {
            document[key] = replacement;
        }
        let error = validate_document(&document).expect_err("invalid document must be rejected");
        assert!(error.to_string().contains(pointer), "{error}");
        assert!(to_json(&document).is_err());
        assert!(to_yaml(&document).is_err());
    }
    let mut document = minimal();
    document["unknown_property"] = json!(true);
    assert!(validate_document(&document).is_err());
}

#[test]
fn schema_errors_report_precise_json_pointers() {
    let mut document = minimal();
    document["datasets"][0]["fields"] = json!([{
        "name": "amount",
        "expression": {"dialects": [{"dialect": "NOT_A_DIALECT", "expression": "amount"}]}
    }]);
    let error = validate_document(&document).expect_err("unknown dialect must fail");
    let OssieError::Validation(issues) = error else {
        panic!("expected schema violations");
    };
    assert!(issues.iter().any(|issue| {
        issue.instance_path == "/datasets/0/fields/0/expression/dialects/0/dialect"
            && !issue.schema_path.is_empty()
    }));
}

#[test]
fn full_schema_accepts_all_declared_dialects_and_datatypes(
) -> Result<(), Box<dyn std::error::Error>> {
    let schema: Value = serde_json::from_str(SCHEMA_JSON)?;
    let dialects = schema["$defs"]["Dialect"]["enum"]
        .as_array()
        .ok_or("missing dialect vocabulary")?;
    let datatypes = schema["$defs"]["DataType"]["enum"]
        .as_array()
        .ok_or("missing datatype vocabulary")?;
    for dialect in dialects {
        for datatype in datatypes {
            let mut document = minimal();
            document["metrics"] = json!([{
                "name": "test", "datatype": datatype,
                "expression": {"dialects": [{"dialect": dialect, "expression": "test"}]}
            }]);
            assert_eq!(
                SemanticModel::from_value(document.clone())?.to_value()?,
                document
            );
        }
    }
    Ok(())
}

#[test]
fn malformed_input_returns_parse_errors_and_json_auto_is_strict() {
    for (text, format) in [
        ("{bad json}", DocumentFormat::Json),
        ("{bad json}", DocumentFormat::Auto),
        ("datasets: [", DocumentFormat::Yaml),
    ] {
        assert!(matches!(
            parse_document(text, format),
            Err(OssieError::Parse(_))
        ));
    }
}
