//! Ossie views and document-to-operation diffs over native semantic metadata.
use super::operation::*;
use crate::{BundleBuilder, BundleFacade, BundlebaseError};
use arrow_schema::DataType;
use bundlebase_ossie::{validate_document, DocumentFormat};
use datafusion::sql::sqlparser::{
    ast::{visit_expressions, Expr},
    dialect::{AnsiDialect, GenericDialect},
    parser::Parser,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

fn invalid(path: &str, message: impl std::fmt::Display) -> BundlebaseError {
    format!("Semantic model {path}: {message}").into()
}
fn put(object: &mut Value, key: &str, value: &Option<impl serde::Serialize>) {
    if let Some(value) = value {
        object[key] = json!(value);
    }
}
fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
fn expression(sql: &str) -> Value {
    json!({"dialects": [{"dialect": "ANSI_SQL", "expression": sql}]})
}
/// Export a deliberately small SQL subset honestly as ANSI_SQL. Native metric
/// execution remains available for DataFusion-specific expressions.
fn portable_expression(sql: &str, path: &str) -> Result<(), BundlebaseError> {
    let query = format!("SELECT ({sql}) FROM bundle");
    let statements = Parser::parse_sql(&AnsiDialect {}, &query)
        .map_err(|e| invalid(path, format!("not in the supported ANSI SQL subset: {e}")))?;
    if statements.len() != 1 {
        return Err(invalid(path, "expected a single SQL expression"));
    }
    let result = visit_expressions(&statements, |expr| {
        if let Expr::Function(function) = expr {
            let name = function
                .name
                .to_string()
                .trim_matches('"')
                .to_ascii_uppercase();
            if !matches!(
                name.as_str(),
                "SUM"
                    | "COUNT"
                    | "AVG"
                    | "MIN"
                    | "MAX"
                    | "COALESCE"
                    | "NULLIF"
                    | "FLOOR"
                    | "CEILING"
                    | "ABS"
                    | "ROUND"
                    | "UPPER"
                    | "LOWER"
                    | "CHAR_LENGTH"
                    | "CHARACTER_LENGTH"
                    | "SUBSTRING"
                    | "TRIM"
                    | "STDDEV_POP"
                    | "STDDEV_SAMP"
                    | "VAR_POP"
                    | "VAR_SAMP"
            ) {
                return std::ops::ControlFlow::Break(invalid(path,format!("function '{name}' is outside the supported ANSI SQL subset; use native metric execution")));
            }
        }
        std::ops::ControlFlow::Continue(())
    });
    if let std::ops::ControlFlow::Break(error) = result {
        return Err(error);
    }
    Ok(())
}
fn datatype(data_type: &DataType) -> &'static str {
    match data_type {
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => "String",
        DataType::Int8
        | DataType::Int16
        | DataType::Int32
        | DataType::Int64
        | DataType::UInt8
        | DataType::UInt16
        | DataType::UInt32
        | DataType::UInt64 => "Integer",
        DataType::Float16 | DataType::Float32 | DataType::Float64 => "Float",
        DataType::Decimal32(_, _)
        | DataType::Decimal64(_, _)
        | DataType::Decimal128(_, _)
        | DataType::Decimal256(_, _) => "Decimal",
        DataType::Boolean => "Boolean",
        DataType::Date32 | DataType::Date64 => "Date",
        DataType::Time32(_) | DataType::Time64(_) => "Time",
        DataType::Timestamp(_, Some(_)) => "DateTimeTz",
        DataType::Timestamp(_, None) => "DateTime",
        _ => "Opaque",
    }
}

/// Generate an Ossie snapshot without executing a data query or recording changes.
pub async fn semantic_model<B: BundleFacade + ?Sized>(
    bundle: &B,
) -> Result<Value, BundlebaseError> {
    let state = bundle.semantic_state();
    let schema = bundle.schema().await?;
    let registry = bundle.bundle_schema();
    let live_ids: BTreeSet<_> = schema
        .fields()
        .iter()
        .filter_map(|field| registry.column_id(field.name()))
        .collect();
    // Match the native column registry, excluding the empty-bundle sentinel.
    // A real user column named no_data remains present because it has an ID.
    let fields: Vec<Value> = schema.fields().iter().filter(|field| registry.column_id(field.name()).is_some()).map(|field| {
        let mut value = json!({"name": field.name(), "expression": expression(&quote(field.name())), "datatype": datatype(field.data_type())});
        if let Some(metadata) = registry.column_id(field.name()).and_then(|id| state.columns.get(&id)) {
            put(&mut value, "description", &metadata.description);
            put(&mut value, "label", &metadata.label);
            put(&mut value, "ai_context", &metadata.ai_context);
        }
        value
    }).collect();
    let mut dataset = json!({"name": "bundle", "source": "bundle", "fields": fields});
    put(&mut dataset, "description", &state.dataset_description);
    put(&mut dataset, "ai_context", &state.dataset_ai_context);
    let names = |columns: &[crate::object_id::ColumnId]| -> Vec<String> {
        columns
            .iter()
            .filter_map(|id| registry.column_name(id))
            .collect()
    };
    if let Some(key) = state
        .primary_key
        .as_ref()
        .filter(|key| key.iter().all(|id| live_ids.contains(id)))
    {
        dataset["primary_key"] = json!(names(key));
    }
    let unique_keys: Vec<_> = state
        .unique_keys
        .iter()
        .filter(|key| key.iter().all(|id| live_ids.contains(id)))
        .map(|key| names(key))
        .collect();
    if !unique_keys.is_empty() {
        dataset["unique_keys"] = json!(unique_keys);
    }
    let metrics: Vec<Value> = state
        .metrics
        .iter()
        .map(|(name, metric)| {
            let sql = bundle.display_sql(&metric.expression);
            let stored = Parser::parse_sql(
                &GenericDialect {},
                &format!("SELECT ({}) FROM bundle", metric.expression),
            )
            .map_err(|e| invalid(&format!("/metrics/{name}/expression"), e))?;
            let references = visit_expressions(&stored, |expr| {
                let identifier = match expr {
                    Expr::Identifier(id) => Some(id),
                    Expr::CompoundIdentifier(ids) => ids.last(),
                    _ => None,
                };
                if let Some(id) = identifier {
                    if super::bundle_schema::parse_internal_name(&id.value)
                        .is_some_and(|id| !live_ids.contains(&id))
                    {
                        return std::ops::ControlFlow::Break(
                            "metric references a column that is no longer present",
                        );
                    }
                }
                if let Expr::Function(function) = expr {
                    if function
                        .name
                        .to_string()
                        .trim_matches('"')
                        .starts_with("fn_")
                    {
                        return std::ops::ControlFlow::Break(
                            "imported functions cannot be exported as ANSI_SQL",
                        );
                    }
                }
                std::ops::ControlFlow::Continue(())
            });
            if let std::ops::ControlFlow::Break(message) = references {
                return Err(invalid(&format!("/metrics/{name}/expression"), message));
            }
            portable_expression(&sql, &format!("/metrics/{name}/expression"))?;
            let mut value = json!({"name": name, "expression": expression(&sql)});
            put(&mut value, "description", &metric.description);
            put(&mut value, "ai_context", &metric.ai_context);
            Ok(value)
        })
        .collect::<Result<_, BundlebaseError>>()?;
    let mut document = json!({
        "version": "0.2.0.dev0", "name": bundle.name().unwrap_or_else(|| bundle.id()),
        "datasets": [dataset], "metrics": metrics,
        "custom_extensions": [{"vendor_name": "BUNDLEBASE", "data": json!({
            "version": bundle.version(), "bundlebase_version": env!("CARGO_PKG_VERSION"),
            "latest_commit": bundle.history().last().map(|c| json!({"message":c.message,"author":c.author,"timestamp":c.timestamp}))
        }).to_string()}]
    });
    put(&mut document, "description", &bundle.description());
    put(&mut document, "ai_context", &state.model_ai_context);
    validate_document(&document).map_err(|e| invalid("/", e))?;
    Ok(document)
}

fn sql_expression<'a>(value: &'a Value, path: &str) -> Result<&'a str, BundlebaseError> {
    let dialects = value["dialects"]
        .as_array()
        .ok_or_else(|| invalid(path, "expected expression dialects"))?;
    let matches: Vec<_> = dialects
        .iter()
        .filter(|d| d["dialect"] == "ANSI_SQL")
        .collect();
    if matches.len() != 1 || dialects.len() != 1 {
        return Err(invalid(
            path,
            "exactly one ANSI_SQL expression is required; dialect translation is not supported",
        ));
    }
    matches[0]["expression"]
        .as_str()
        .ok_or_else(|| invalid(path, "expected SQL expression"))
}
fn string<'a>(object: &'a Value, key: &str, path: &str) -> Result<&'a str, BundlebaseError> {
    object[key]
        .as_str()
        .ok_or_else(|| invalid(path, format!("expected {key} string")))
}
fn reject_extensions(object: &Value, path: &str) -> Result<(), BundlebaseError> {
    if let Some(extensions) = object["custom_extensions"].as_array() {
        for extension in extensions {
            if extension["vendor_name"] != "BUNDLEBASE" {
                return Err(invalid(
                    path,
                    format!("unsupported vendor extension {}", extension["vendor_name"]),
                ));
            }
        }
    }
    Ok(())
}
fn unique_named<'a>(
    values: &'a [Value],
    path: &str,
) -> Result<BTreeMap<&'a str, &'a Value>, BundlebaseError> {
    let mut result = BTreeMap::new();
    for (index, value) in values.iter().enumerate() {
        let name = string(value, "name", path)?;
        if result.insert(name, value).is_some() {
            return Err(invalid(
                &format!("{path}/{index}/name"),
                format!("duplicate name '{name}'"),
            ));
        }
    }
    Ok(result)
}
fn ai_operations(
    target: SemanticTarget,
    current: Option<&Value>,
    desired: Option<&Value>,
    replace: bool,
    ops: &mut Vec<AnyOperation>,
) {
    let Some(desired) = desired else {
        if replace && current.is_some() {
            ops.push(
                SetAiContextOp {
                    target,
                    context: None,
                }
                .into(),
            );
        }
        return;
    };
    if current == Some(desired) {
        return;
    }
    if let (Some(Value::Object(old)), Value::Object(new)) = (current, desired) {
        for (member, value) in new {
            if old.get(member) != Some(value) {
                ops.push(
                    SetAiContextMemberOp {
                        target: target.clone(),
                        member: member.clone(),
                        value: Some(value.clone()),
                    }
                    .into(),
                );
            }
        }
        if replace {
            for member in old.keys().filter(|key| !new.contains_key(*key)) {
                ops.push(
                    SetAiContextMemberOp {
                        target: target.clone(),
                        member: member.clone(),
                        value: None,
                    }
                    .into(),
                );
            }
        }
    } else {
        ops.push(
            SetAiContextOp {
                target,
                context: Some(desired.clone()),
            }
            .into(),
        );
    }
}
fn requested_string(
    object: &Value,
    key: &str,
    old: &Option<String>,
    replace: bool,
) -> Option<Option<String>> {
    if !replace && object.get(key).is_none() {
        return None;
    }
    let new = object[key].as_str().map(str::to_owned);
    if &new == old {
        None
    } else {
        Some(new)
    }
}

/// Validate and lower a supplied document into a concrete minimal operation list.
/// Omission is unchanged for MERGE, removed within the selected model for REPLACE.
pub async fn document_operations(
    builder: &BundleBuilder,
    document: &Value,
    replace: bool,
) -> Result<Vec<AnyOperation>, BundlebaseError> {
    validate_document(document).map_err(|e| invalid("/", e))?;
    reject_extensions(document, "/custom_extensions")?;
    let datasets = document["datasets"]
        .as_array()
        .ok_or_else(|| invalid("/datasets", "expected datasets"))?;
    if datasets.len() != 1 {
        return Err(invalid(
            "/datasets",
            "exactly one dataset for the opened bundle is supported",
        ));
    }
    let dataset = &datasets[0];
    if dataset["name"] != "bundle" {
        return Err(invalid(
            "/datasets/0/name",
            "expected 'bundle'; dataset name is derived",
        ));
    }
    if dataset["source"] != "bundle" {
        return Err(invalid(
            "/datasets/0/source",
            "expected 'bundle'; attaching sources is not a semantic operation",
        ));
    }
    reject_extensions(dataset, "/datasets/0/custom_extensions")?;
    if document["relationships"]
        .as_array()
        .is_some_and(|v| !v.is_empty())
    {
        return Err(invalid(
            "/relationships",
            "relationships are not supported; joins are not foreign-key declarations",
        ));
    }
    let state = builder.semantic_state();
    let registry = builder.bundle_schema();
    let schema = builder.schema().await?;
    let mut ops: Vec<AnyOperation> = Vec::new();
    let name = string(document, "name", "/name")?;
    if name != builder.name().unwrap_or_else(|| builder.id()) {
        ops.push(SetNameOp::setup(name).into());
    }
    if let Some(value) = requested_string(document, "description", &builder.description(), replace)
    {
        match value {
            Some(value) => ops.push(SetDescriptionOp::setup(&value).into()),
            None => ops.push(ClearDescriptionOp::setup().into()),
        }
    }
    ai_operations(
        SemanticTarget::Model,
        state.model_ai_context.as_ref(),
        document.get("ai_context"),
        replace,
        &mut ops,
    );
    if let Some(value) =
        requested_string(dataset, "description", &state.dataset_description, replace)
    {
        ops.push(SetDatasetDescriptionOp { description: value }.into());
    }
    ai_operations(
        SemanticTarget::Dataset,
        state.dataset_ai_context.as_ref(),
        dataset.get("ai_context"),
        replace,
        &mut ops,
    );
    let empty = Vec::new();
    let fields = dataset["fields"].as_array().unwrap_or(&empty);
    let fields = unique_named(fields, "/datasets/0/fields")?;
    for (name, field) in &fields {
        let path = format!("/datasets/0/fields/{name}");
        schema.field_with_name(name).map_err(|_| {
            invalid(
                &path,
                format!("unknown column '{name}' in the current dataset"),
            )
        })?;
        let id = registry
            .column_id(name)
            .ok_or_else(|| invalid(&path, format!("unknown column '{name}'")))?;
        let sql = sql_expression(&field["expression"], &format!("{path}/expression"))?;
        if sql != *name && sql != quote(name) {
            return Err(invalid(
                &format!("{path}/expression"),
                "computed fields are not supported; use the column's identity expression",
            ));
        }
        if field.get("dimension").is_some() {
            return Err(invalid(
                &format!("{path}/dimension"),
                "dimension metadata is not supported",
            ));
        }
        reject_extensions(field, &path)?;
        if let Some(declared) = field["datatype"].as_str() {
            let actual = schema
                .field_with_name(name)
                .map_err(|e| invalid(&path, e))?;
            if declared != datatype(actual.data_type()) {
                return Err(invalid(&format!("{path}/datatype"),format!("{declared} does not match current type {}; semantic updates do not cast columns",datatype(actual.data_type()))));
            }
        }
        let old = state.columns.get(&id).cloned().unwrap_or_default();
        if let Some(description) = requested_string(field, "description", &old.description, replace)
        {
            match description {
                Some(description) => ops.push(DescribeColumnOp::setup(id, &description).into()),
                None => ops.push(ClearColumnDescriptionOp { id }.into()),
            }
        }
        if let Some(label) = requested_string(field, "label", &old.label, replace) {
            ops.push(SetColumnLabelOp { id, label }.into());
        }
        ai_operations(
            SemanticTarget::Column(id),
            old.ai_context.as_ref(),
            field.get("ai_context"),
            replace,
            &mut ops,
        );
    }
    if replace {
        for (id, old) in &state.columns {
            let Some(name) = registry.column_name(id) else {
                continue;
            };
            if fields.contains_key(name.as_str()) || schema.field_with_name(&name).is_err() {
                continue;
            }
            if old.description.is_some() {
                ops.push(ClearColumnDescriptionOp { id: *id }.into());
            }
            if old.label.is_some() {
                ops.push(
                    SetColumnLabelOp {
                        id: *id,
                        label: None,
                    }
                    .into(),
                );
            }
            if old.ai_context.is_some() {
                ops.push(
                    SetAiContextOp {
                        target: SemanticTarget::Column(*id),
                        context: None,
                    }
                    .into(),
                );
            }
        }
    }
    let metric_values = document["metrics"].as_array().unwrap_or(&empty);
    let metrics = unique_named(metric_values, "/metrics")?;
    for (name, metric) in &metrics {
        let path = format!("/metrics/{name}");
        reject_extensions(metric, &path)?;
        // Metric result datatypes are descriptive, not a request to transform data.
        if metric.get("datatype").is_some() {
            return Err(invalid(
                &format!("{path}/datatype"),
                "metric datatype declarations are not supported; omit datatype",
            ));
        }
        let sql = sql_expression(&metric["expression"], &format!("{path}/expression"))?;
        portable_expression(sql, &format!("{path}/expression"))?;
        let old = state.metrics.get(*name);
        // Reuse stored SQL for an unchanged public expression. Rendering a
        // renamed identifier can introduce quoting; round-tripping that quote
        // style must not manufacture a metric edit.
        let translated = match old.filter(|old| builder.display_sql(&old.expression) == sql) {
            Some(old) => old.expression.clone(),
            None => builder.translate_sql(sql),
        };
        // Validate supplied definitions even when their text matches an existing
        // metric that has since become invalid after a schema/function change.
        DefineMetricOp::setup(name, &translated)
            .check(builder.bundle())
            .await?;
        if old.is_none_or(|old| old.expression != translated) {
            if old.is_some() {
                ops.push(
                    SetMetricExpressionOp {
                        name: (*name).to_owned(),
                        expression: translated,
                    }
                    .into(),
                );
            } else {
                ops.push(DefineMetricOp::setup(name, &translated).into());
            }
        }
        let old_description = old.and_then(|old| old.description.clone());
        if let Some(description) =
            requested_string(metric, "description", &old_description, replace)
        {
            ops.push(
                SetMetricDescriptionOp {
                    name: (*name).to_owned(),
                    description,
                }
                .into(),
            );
        }
        ai_operations(
            SemanticTarget::Metric((*name).to_owned()),
            old.and_then(|old| old.ai_context.as_ref()),
            metric.get("ai_context"),
            replace,
            &mut ops,
        );
    }
    if replace {
        for name in state
            .metrics
            .keys()
            .filter(|name| !metrics.contains_key(name.as_str()))
        {
            ops.push(DropMetricOp::setup(name).into());
        }
    }
    let resolve_key =
        |value: &Value, path: &str| -> Result<Vec<crate::object_id::ColumnId>, BundlebaseError> {
            let values = value
                .as_array()
                .ok_or_else(|| invalid(path, "expected key columns"))?;
            let mut seen = BTreeSet::new();
            values
                .iter()
                .map(|name| {
                    let name = name
                        .as_str()
                        .ok_or_else(|| invalid(path, "expected column name"))?;
                    schema.field_with_name(name).map_err(|_| {
                        invalid(
                            path,
                            format!("unknown key column '{name}' in the current dataset"),
                        )
                    })?;
                    let id = registry
                        .column_id(name)
                        .ok_or_else(|| invalid(path, format!("unknown key column '{name}'")))?;
                    if !seen.insert(id) {
                        return Err(invalid(path, format!("duplicate key column '{name}'")));
                    }
                    Ok(id)
                })
                .collect()
        };
    if let Some(key) = dataset.get("primary_key") {
        let columns = resolve_key(key, "/datasets/0/primary_key")?;
        if state.primary_key.as_ref() != Some(&columns) {
            ops.push(
                DeclareKeyOp {
                    columns,
                    unique: false,
                }
                .into(),
            );
        }
    } else if replace && state.primary_key.is_some() {
        ops.push(
            DropKeyOp {
                columns: vec![],
                unique: false,
            }
            .into(),
        );
    }
    if dataset.get("unique_keys").is_some() || replace {
        let requested = dataset["unique_keys"].as_array().unwrap_or(&empty);
        let canonical = |key: &[crate::object_id::ColumnId]| {
            let mut key = key.to_vec();
            key.sort();
            key
        };
        let mut desired = BTreeMap::new();
        for (index, key) in requested.iter().enumerate() {
            let columns = resolve_key(key, &format!("/datasets/0/unique_keys/{index}"))?;
            if desired.insert(canonical(&columns), columns).is_some() {
                return Err(invalid("/datasets/0/unique_keys", "duplicate unique key"));
            }
        }
        let current: BTreeSet<_> = state.unique_keys.iter().map(|key| canonical(key)).collect();
        for (key, columns) in &desired {
            if !current.contains(key) {
                ops.push(
                    DeclareKeyOp {
                        columns: columns.clone(),
                        unique: true,
                    }
                    .into(),
                );
            }
        }
        if replace {
            for columns in &state.unique_keys {
                if !desired.contains_key(&canonical(columns)) {
                    ops.push(
                        DropKeyOp {
                            columns: columns.clone(),
                            unique: true,
                        }
                        .into(),
                    );
                }
            }
        }
    }
    // Check against prospective state without touching the caller's bundle. Sequential
    // validation inside do_change also handles a new metric followed by its metadata.
    for op in &ops {
        if let Some(result) = super::operation::check_semantic_replay(op) {
            result?;
        }
        if let AnyOperation::SetName(op) = op {
            op.check(builder.bundle()).await?;
        }
    }
    Ok(ops)
}

impl BundleBuilder {
    /// Apply supplied metadata, or replace the supported metadata when `replace` is true.
    pub async fn apply_semantic_model(
        &self,
        document: &Value,
        replace: bool,
    ) -> Result<&Self, BundlebaseError> {
        let operations = Box::pin(document_operations(self, document, replace)).await?;
        if operations.is_empty() {
            return Ok(self);
        }
        self.do_change("Apply semantic model", move |builder| {
            Box::pin(async move {
                for operation in operations {
                    builder.apply_operation(operation).await?;
                }
                Ok(())
            })
        })
        .await?;
        Ok(self)
    }
    /// Import JSON/YAML using the normal storage abstraction.
    pub async fn import_semantic_model(
        &self,
        path: &str,
        replace: bool,
    ) -> Result<&Self, BundlebaseError> {
        let file = crate::io::readable_file_from_url(&document_url(path)?, self.config()).await?;
        let text = file
            .read_str()
            .await?
            .ok_or_else(|| invalid(path, "file does not exist"))?;
        let document = bundlebase_ossie::parse_document(&text, file_format(path)?)
            .map_err(|e| invalid(path, e))?;
        Box::pin(self.apply_semantic_model(&document, replace)).await
    }
}
fn file_format(path: &str) -> Result<DocumentFormat, BundlebaseError> {
    let path = path.split('?').next().unwrap_or(path).to_ascii_lowercase();
    if path.ends_with(".json") {
        Ok(DocumentFormat::Json)
    } else if path.ends_with(".yaml") || path.ends_with(".yml") {
        Ok(DocumentFormat::Yaml)
    } else {
        Err(invalid("/path", "use a .json, .yaml, or .yml extension"))
    }
}
pub async fn export_semantic_model<B: BundleFacade + ?Sized>(
    bundle: &B,
    path: &str,
) -> Result<(), BundlebaseError> {
    let document = semantic_model(bundle).await?;
    let text = match file_format(path)? {
        DocumentFormat::Json => bundlebase_ossie::to_json(&document),
        _ => bundlebase_ossie::to_yaml(&document),
    }
    .map_err(|e| invalid(path, e))?;
    let file = crate::io::writable_file_from_url(&document_url(path)?, bundle.config()).await?;
    file.write(bytes::Bytes::from(text)).await
}

/// File paths use the caller's working directory; explicit storage URLs retain
/// their scheme, independently of where the bundle itself is stored.
fn document_url(path: &str) -> Result<url::Url, BundlebaseError> {
    if path.contains("://") {
        return Ok(url::Url::parse(path)?);
    }
    let path = std::path::Path::new(path);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    url::Url::from_file_path(absolute).map_err(|_| invalid("/path", "invalid file path"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn portable_sql_uses_ast_not_string_or_keyword_heuristics() -> Result<(), BundlebaseError> {
        portable_expression(
            "SUM(CASE WHEN (salary > 0) AND (salary < 10) THEN salary ELSE 0 END)",
            "/test",
        )?;
        portable_expression("COUNT(CASE WHEN status = 'fn_active' THEN 1 END)", "/test")?;
        assert!(portable_expression("arrow_cast(salary, 'Int64')", "/test").is_err());
        assert!(portable_expression("\"private_function\"(salary)", "/test").is_err());
        Ok(())
    }
}
