//! User-defined function entry system for named, platform-aware SQL functions.
//!
//! A `FunctionEntry` is created via `IMPORT FUNCTION acme.double_val(Int64) RETURNS Int64`
//! and represents a single function binding for a name+platform pair.
//! `resolve_function` picks the best entry for the current platform at runtime.

use crate::bundle::connector_definition::Platform;
use crate::bundle::logic_runtime::LogicRuntime;
use crate::data::ObjectId;
use crate::function::ipc_bridge::SubprocessCache;
use crate::function::VersionFunction;
use crate::io::IOReadWriteDir;
use crate::namespaced_name::NamespacedName;
use crate::BundlebaseError;
use arrow::datatypes::{DataType, Field, Fields};
use datafusion::logical_expr::ScalarUDF;
use datafusion::prelude::SessionContext;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

/// Whether a function is scalar (row → row), aggregate (many rows → one result),
/// or table-valued (returns a table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FunctionKind {
    Scalar,
    Aggregate,
    /// Table-valued function that returns a table of rows.
    /// Infrastructure only — execution path is not yet implemented.
    TableValued,
}

impl fmt::Display for FunctionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FunctionKind::Scalar => write!(f, "scalar"),
            FunctionKind::Aggregate => write!(f, "aggregate"),
            FunctionKind::TableValued => write!(f, "table_valued"),
        }
    }
}

impl std::str::FromStr for FunctionKind {
    type Err = BundlebaseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "scalar" => Ok(FunctionKind::Scalar),
            "aggregate" => Ok(FunctionKind::Aggregate),
            "table_valued" | "tablevalued" => Ok(FunctionKind::TableValued),
            _ => Err(format!(
                "Unknown function type '{}'. Expected 'scalar', 'aggregate', or 'table_valued'.",
                s
            )
            .into()),
        }
    }
}

/// Custom serde for Arrow `DataType` ↔ string (e.g., `"Int64"` in YAML).
pub mod arrow_type_serde {
    use super::{arrow_type_to_name, parse_arrow_type_name, DataType};
    use serde::{self, Deserialize, Deserializer, Serializer};

    /// Serde helpers for a single `DataType` field.
    pub mod single {
        use super::*;

        pub fn serialize<S>(dt: &DataType, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serializer.serialize_str(&arrow_type_to_name(dt))
        }

        pub fn deserialize<'de, D>(deserializer: D) -> Result<DataType, D::Error>
        where
            D: Deserializer<'de>,
        {
            let s = String::deserialize(deserializer)?;
            parse_arrow_type_name(&s).map_err(serde::de::Error::custom)
        }
    }

    /// Serde helpers for a `Vec<DataType>` field.
    pub mod vec {
        use super::*;
        use serde::ser::SerializeSeq;

        pub fn serialize<S>(types: &[DataType], serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let mut seq = serializer.serialize_seq(Some(types.len()))?;
            for dt in types {
                seq.serialize_element(&arrow_type_to_name(dt))?;
            }
            seq.end()
        }

        pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<DataType>, D::Error>
        where
            D: Deserializer<'de>,
        {
            let strings: Vec<String> = Vec::<String>::deserialize(deserializer)?;
            strings
                .iter()
                .map(|s| parse_arrow_type_name(s).map_err(serde::de::Error::custom))
                .collect()
        }
    }
}

/// A single function entry binding a name+platform to runtime+signature.
///
/// Multiple entries can exist for the same function name (different platforms
/// or temporary vs persisted). Resolution picks the best match at runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionEntry {
    /// Unique identifier for this function entry
    pub id: ObjectId,
    /// Namespaced function name (e.g., "acme.double_val")
    pub name: NamespacedName,
    /// Arrow types for input parameters (e.g., [DataType::Int64, DataType::Utf8])
    #[serde(with = "arrow_type_serde::vec")]
    pub input_types: Vec<DataType>,
    /// Arrow type for the return value (e.g., DataType::Int64)
    #[serde(with = "arrow_type_serde::single")]
    pub return_type: DataType,
    /// Runtime with parsed logic (e.g., FfiRuntime { path: "./mylib.so", symbol: Some("double_val") })
    pub from: LogicRuntime,
    /// Platform pattern in Docker-style os/arch
    pub platform: Platform,
    /// Whether this is a temporary (session-only) entry
    pub temporary: bool,
    /// Scalar or aggregate function
    pub kind: FunctionKind,
}

/// Registry of function entries with lookup, resolution, and removal.
///
/// Wraps `Vec<FunctionEntry>` with all entry-management methods.
/// Used by `Bundle` behind an `Arc<RwLock<…>>`.
#[derive(Clone)]
pub struct FunctionRegistry {
    entries: Vec<FunctionEntry>,
    data_dir: Arc<RwLock<Arc<dyn IOReadWriteDir>>>,
    ctx: Arc<SessionContext>,
    subprocess_cache: SubprocessCache,
}

impl fmt::Debug for FunctionRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FunctionRegistry")
            .field("entries", &self.entries)
            .finish()
    }
}

impl FunctionRegistry {
    pub fn new(
        data_dir: Arc<RwLock<Arc<dyn IOReadWriteDir>>>,
        ctx: Arc<SessionContext>,
        subprocess_cache: SubprocessCache,
    ) -> Self {
        Self {
            entries: Vec::new(),
            data_dir,
            ctx,
            subprocess_cache,
        }
    }

    /// Add a function entry to the registry.
    pub fn add(&mut self, entry: FunctionEntry) {
        self.entries.push(entry);
    }

    /// Check if any function entry exists for the given name.
    pub fn has(&self, name: &str) -> bool {
        self.entries.iter().any(|e| e.name == name)
    }

    /// Resolve the best function entry for the current platform.
    ///
    /// Tries temporary entries first (reverse order, last wins), then persisted entries.
    /// Returns the first entry whose platform matches the current system.
    pub fn resolve(&self, name: &str) -> Result<FunctionEntry, BundlebaseError> {
        let matching: Vec<&FunctionEntry> = self.entries.iter().filter(|e| e.name == name).collect();

        if matching.is_empty() {
            return Err(format!("Function '{}' is not defined", name).into());
        }

        // Try temporary entries first (reverse order, last wins)
        for entry in matching.iter().rev() {
            if entry.temporary && entry.platform.matches_current() {
                return Ok((*entry).clone());
            }
        }

        // Then persisted entries (reverse order, last wins)
        for entry in matching.iter().rev() {
            if !entry.temporary && entry.platform.matches_current() {
                return Ok((*entry).clone());
            }
        }

        let platforms: Vec<String> = matching.iter().map(|e| e.platform.to_string()).collect();
        Err(format!(
            "No function logic matches current platform '{}' for function '{}'. Available platforms: {}",
            Platform::current(),
            name,
            platforms.join(", ")
        )
        .into())
    }

    /// Remove all function entries for a name.
    pub fn remove_all(&mut self, name: &str) {
        self.entries.retain(|e| e.name != name);
    }

    /// Remove matching function entries. Returns the number removed.
    pub fn remove(
        &mut self,
        name: &str,
        platform: Option<&Platform>,
        temporary_only: bool,
    ) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| {
            if e.name != name {
                return true;
            }
            if temporary_only && !e.temporary {
                return true;
            }
            if let Some(p) = platform {
                if &e.platform != p {
                    return true;
                }
            }
            false
        });
        before - self.entries.len()
    }

    /// Get a read-only view of all function entries.
    pub fn entries(&self) -> &[FunctionEntry] {
        &self.entries
    }

    /// Check if any temporary function entries exist.
    pub fn has_temporary(&self) -> bool {
        self.entries.iter().any(|e| e.temporary)
    }

    /// Resolve all overloads for a name and register them as a composite
    /// DataFusion UDF/UDAF/UDTF using the registry's session context.
    ///
    /// Uses the registry's `data_dir` to resolve bundle-relative logic paths
    /// and `subprocess_cache` for IPC-based functions.
    pub fn register_functions_for_name(
        &self,
        name: &str,
    ) -> Result<(), BundlebaseError> {
        let overloads = self.resolve_all(name);
        if overloads.is_empty() {
            return Ok(());
        }

        let data_dir = self.data_dir.read().clone();

        // Resolve bundle-relative logic paths against the data directory
        let overloads: Vec<_> = overloads
            .into_iter()
            .map(|mut e| {
                e.from = e.from.resolve_path(&data_dir);
                e
            })
            .collect();

        let kind = validate_kind_consistency(&overloads)?;

        match kind {
            FunctionKind::Scalar => {
                use crate::function::scalar::ScalarFunction;
                let func = ScalarFunction::new_composite(overloads, Arc::clone(&self.subprocess_cache))?;
                self.ctx.register_udf(ScalarUDF::from(func));
            }
            FunctionKind::Aggregate => {
                use crate::function::aggregate::AggregateFunction;
                use datafusion::logical_expr::AggregateUDF;
                let agg = AggregateFunction::new_composite(overloads, Arc::clone(&self.subprocess_cache))?;
                self.ctx.register_udaf(AggregateUDF::from(agg));
            }
            FunctionKind::TableValued => {
                tracing::warn!(
                    "Table-valued function '{}' registered but execution is not yet supported",
                    name
                );
            }
        }
        Ok(())
    }

    /// Re-register the version() UDF to reflect the current version string.
    pub fn refresh_version_udf(&self, version: String) {
        self.ctx
            .register_udf(ScalarUDF::new_from_impl(VersionFunction::new(version)));
    }

    /// Get sorted unique namespaces from all function entries.
    pub fn namespaces(&self) -> Vec<String> {
        let mut namespaces: Vec<String> = self
            .entries
            .iter()
            .map(|e| e.name.namespace.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        namespaces.sort();
        namespaces
    }

    /// Get NamespacedNames for all function entries.
    pub fn names(&self) -> Vec<crate::NamespacedName> {
        self.entries
            .iter()
            .map(|e| e.name.clone())
            .collect()
    }

    /// Resolve all function entries for a name, grouped by input type signature.
    ///
    /// For each unique input type signature, picks the best platform match
    /// (temporary entries shadow persistent, last wins within each tier).
    /// Returns one entry per distinct signature.
    pub fn resolve_all(&self, name: &str) -> Vec<FunctionEntry> {
        let matching: Vec<&FunctionEntry> = self.entries.iter().filter(|e| e.name == name).collect();

        if matching.is_empty() {
            return Vec::new();
        }

        // Group by input_types signature
        let mut signature_groups: std::collections::HashMap<Vec<DataType>, Vec<&FunctionEntry>> =
            std::collections::HashMap::new();
        for entry in &matching {
            signature_groups
                .entry(entry.input_types.clone())
                .or_default()
                .push(entry);
        }

        // For each signature group, pick best match (same logic as resolve)
        let mut results = Vec::new();
        for (_sig, group) in signature_groups {
            // Try temporary entries first (reverse order, last wins)
            let mut found = None;
            for entry in group.iter().rev() {
                if entry.temporary && entry.platform.matches_current() {
                    found = Some((*entry).clone());
                    break;
                }
            }
            if found.is_none() {
                // Then persisted entries (reverse order, last wins)
                for entry in group.iter().rev() {
                    if !entry.temporary && entry.platform.matches_current() {
                        found = Some((*entry).clone());
                        break;
                    }
                }
            }
            if let Some(entry) = found {
                results.push(entry);
            }
        }

        results
    }

    /// Remove function entries by their IDs.
    pub fn remove_by_ids(&mut self, ids: &[ObjectId]) {
        self.entries.retain(|e| !ids.contains(&e.id));
    }

    /// Rename function entries matching the given IDs to a new name.
    pub fn rename_entries(&mut self, ids: &[ObjectId], new_name: &crate::NamespacedName) {
        for entry in &mut self.entries {
            if ids.contains(&entry.id) {
                entry.name = new_name.clone();
            }
        }
    }

    /// Rename only temporary function entries matching the old name to a new name.
    pub fn rename_temp_entries(&mut self, old_name: &str, new_name: &crate::NamespacedName) {
        for entry in &mut self.entries {
            if entry.temporary && entry.name == old_name {
                entry.name = new_name.clone();
            }
        }
    }

    /// Remove entries matching a specific input type signature, or all if None.
    ///
    /// If `input_types` is `None`, removes all entries for the name (same as `remove_all`).
    /// If `input_types` is `Some(types)`, removes only entries matching that signature.
    pub fn remove_by_signature(
        &mut self,
        name: &str,
        input_types: Option<&[DataType]>,
    ) {
        match input_types {
            None => self.remove_all(name),
            Some(types) => {
                self.entries.retain(|e| {
                    !(e.name == name && e.input_types == types)
                });
            }
        }
    }
}

/// Validate that all entries share the same FunctionKind (scalar or aggregate).
///
/// Returns the consistent kind, or an error if entries mix scalar and aggregate.
pub fn validate_kind_consistency(entries: &[FunctionEntry]) -> Result<FunctionKind, BundlebaseError> {
    let first = entries.first().ok_or_else(|| {
        BundlebaseError::from("No function entries provided for kind validation")
    })?;
    let expected = first.kind;
    for entry in entries.iter().skip(1) {
        if entry.kind != expected {
            return Err(format!(
                "Function '{}' has overloads with mixed kinds (scalar and aggregate). \
                 All overloads of a function must be the same kind.",
                first.name
            ).into());
        }
    }
    Ok(expected)
}

/// Parse an Arrow type name string into a DataFusion `DataType`.
///
/// Supports common Arrow type names used in function signatures,
/// including complex/nested types.
///
/// # Examples
/// - `"Int64"` → `DataType::Int64`
/// - `"Utf8"` → `DataType::Utf8`
/// - `"List<Int64>"` → `DataType::List(Field::new("item", DataType::Int64, true))`
/// - `"Struct<x:Int64,y:Float64>"` → `DataType::Struct(...)`
/// - `"Map<Utf8,Int64>"` → `DataType::Map(...)`
/// - `"Decimal128(38,10)"` → `DataType::Decimal128(38, 10)`
pub fn parse_arrow_type_name(type_name: &str) -> Result<DataType, BundlebaseError> {
    let trimmed = type_name.trim();

    // Check for parameterized types first
    if let Some(inner) = trimmed.strip_prefix("List<").and_then(|s| s.strip_suffix('>')) {
        let element_type = parse_arrow_type_name(inner)?;
        return Ok(DataType::List(Arc::new(Field::new(
            "item",
            element_type,
            true,
        ))));
    }

    if let Some(inner) = trimmed
        .strip_prefix("Struct<")
        .and_then(|s| s.strip_suffix('>'))
    {
        let fields = parse_struct_fields(inner)?;
        return Ok(DataType::Struct(fields));
    }

    if let Some(inner) = trimmed.strip_prefix("Map<").and_then(|s| s.strip_suffix('>')) {
        let (key_str, value_str) = split_top_level_comma(inner).ok_or_else(|| {
            BundlebaseError::from(format!(
                "Invalid Map type '{}'. Expected format: Map<KeyType,ValueType>",
                trimmed
            ))
        })?;
        let key_type = parse_arrow_type_name(key_str.trim())?;
        let value_type = parse_arrow_type_name(value_str.trim())?;
        let entries_field = Field::new(
            "entries",
            DataType::Struct(Fields::from(vec![
                Field::new("key", key_type, false),
                Field::new("value", value_type, true),
            ])),
            false,
        );
        return Ok(DataType::Map(Arc::new(entries_field), false));
    }

    if let Some(inner) = trimmed
        .strip_prefix("Decimal128(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let parts: Vec<&str> = inner.split(',').collect();
        if parts.len() != 2 {
            return Err(format!(
                "Invalid Decimal128 type '{}'. Expected format: Decimal128(precision,scale)",
                trimmed
            )
            .into());
        }
        let precision: u8 = parts[0].trim().parse().map_err(|_| {
            BundlebaseError::from(format!(
                "Invalid Decimal128 precision '{}'. Must be a number 1-38.",
                parts[0].trim()
            ))
        })?;
        let scale: i8 = parts[1].trim().parse().map_err(|_| {
            BundlebaseError::from(format!(
                "Invalid Decimal128 scale '{}'. Must be a number.",
                parts[1].trim()
            ))
        })?;
        return Ok(DataType::Decimal128(precision, scale));
    }

    // Simple types
    match trimmed {
        "Boolean" => Ok(DataType::Boolean),
        "Int8" => Ok(DataType::Int8),
        "Int16" => Ok(DataType::Int16),
        "Int32" => Ok(DataType::Int32),
        "Int64" => Ok(DataType::Int64),
        "UInt8" => Ok(DataType::UInt8),
        "UInt16" => Ok(DataType::UInt16),
        "UInt32" => Ok(DataType::UInt32),
        "UInt64" => Ok(DataType::UInt64),
        "Float16" => Ok(DataType::Float16),
        "Float32" => Ok(DataType::Float32),
        "Float64" => Ok(DataType::Float64),
        "Utf8" => Ok(DataType::Utf8),
        "LargeUtf8" => Ok(DataType::LargeUtf8),
        "Binary" => Ok(DataType::Binary),
        "LargeBinary" => Ok(DataType::LargeBinary),
        "Date32" => Ok(DataType::Date32),
        "Date64" => Ok(DataType::Date64),
        "Timestamp" => Ok(DataType::Timestamp(
            arrow::datatypes::TimeUnit::Microsecond,
            None,
        )),
        _ => Err(format!(
            "Unknown Arrow type name '{}'. Supported types: Boolean, Int8, Int16, Int32, Int64, \
             UInt8, UInt16, UInt32, UInt64, Float16, Float32, Float64, Utf8, LargeUtf8, \
             Binary, LargeBinary, Date32, Date64, Timestamp, List<T>, Struct<name:type,...>, \
             Map<K,V>, Decimal128(precision,scale)",
            trimmed
        )
        .into()),
    }
}

/// Split a string at the first top-level comma, respecting nested `<>` and `()`.
///
/// Returns `None` if no top-level comma is found.
fn split_top_level_comma(s: &str) -> Option<(&str, &str)> {
    let mut depth = 0usize;
    for (i, ch) in s.char_indices() {
        match ch {
            '<' | '(' => depth += 1,
            '>' | ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                return Some((&s[..i], &s[i + 1..]));
            }
            _ => {}
        }
    }
    None
}

/// Parse struct field definitions like `"x:Int64,y:Float64"` into Arrow Fields.
///
/// Supports nested types in field definitions (e.g., `"data:List<Int64>,name:Utf8"`).
fn parse_struct_fields(fields_str: &str) -> Result<Fields, BundlebaseError> {
    let mut fields = Vec::new();
    let mut remaining = fields_str;

    while !remaining.is_empty() {
        // Find the colon separating field name from type
        let colon_pos = remaining.find(':').ok_or_else(|| {
            BundlebaseError::from(format!(
                "Invalid struct field '{}'. Expected format: name:type",
                remaining
            ))
        })?;
        let field_name = remaining[..colon_pos].trim();
        let after_colon = &remaining[colon_pos + 1..];

        // Find the end of this field's type (next top-level comma or end of string)
        let (type_str, rest) = match split_top_level_comma(after_colon) {
            Some((type_part, remainder)) => (type_part.trim(), remainder.trim()),
            None => (after_colon.trim(), ""),
        };

        let field_type = parse_arrow_type_name(type_str)?;
        fields.push(Field::new(field_name, field_type, true));
        remaining = rest;
    }

    Ok(Fields::from(fields))
}

/// Convert an Arrow `DataType` to its canonical string representation.
///
/// This is the inverse of `parse_arrow_type_name` and ensures roundtrip fidelity
/// for both simple and complex types.
pub fn arrow_type_to_name(dt: &DataType) -> String {
    match dt {
        DataType::Boolean => "Boolean".to_string(),
        DataType::Int8 => "Int8".to_string(),
        DataType::Int16 => "Int16".to_string(),
        DataType::Int32 => "Int32".to_string(),
        DataType::Int64 => "Int64".to_string(),
        DataType::UInt8 => "UInt8".to_string(),
        DataType::UInt16 => "UInt16".to_string(),
        DataType::UInt32 => "UInt32".to_string(),
        DataType::UInt64 => "UInt64".to_string(),
        DataType::Float16 => "Float16".to_string(),
        DataType::Float32 => "Float32".to_string(),
        DataType::Float64 => "Float64".to_string(),
        DataType::Utf8 => "Utf8".to_string(),
        DataType::LargeUtf8 => "LargeUtf8".to_string(),
        DataType::Binary => "Binary".to_string(),
        DataType::LargeBinary => "LargeBinary".to_string(),
        DataType::Date32 => "Date32".to_string(),
        DataType::Date64 => "Date64".to_string(),
        DataType::Timestamp(_, _) => "Timestamp".to_string(),
        DataType::Decimal128(precision, scale) => {
            format!("Decimal128({},{})", precision, scale)
        }
        DataType::List(field) => {
            format!("List<{}>", arrow_type_to_name(field.data_type()))
        }
        DataType::Struct(fields) => {
            let field_strs: Vec<String> = fields
                .iter()
                .map(|f| format!("{}:{}", f.name(), arrow_type_to_name(f.data_type())))
                .collect();
            format!("Struct<{}>", field_strs.join(","))
        }
        DataType::Map(field, _) => {
            // Map entries field is a Struct with "key" and "value" fields
            if let DataType::Struct(entries) = field.data_type() {
                if entries.len() == 2 {
                    let key_type = arrow_type_to_name(entries[0].data_type());
                    let value_type = arrow_type_to_name(entries[1].data_type());
                    return format!("Map<{},{}>", key_type, value_type);
                }
            }
            // Fallback for non-standard map shapes
            format!("Map<{}>", arrow_type_to_name(field.data_type()))
        }
        other => other.to_string(),
    }
}

/// Parse and validate a dotted function name.
///
/// Enforces single-level dotted namespace: exactly one dot, both parts alphanumeric
/// (letters, digits, underscores). Rejects multi-level names like `a.b.c`.
///
/// # Examples
/// - `"acme.double_val"` → `Ok(NamespacedName { namespace: "acme", name: "double_val" })`
/// - `"acme.datasources.weather"` → error (multi-level)
/// - `"weather"` → error (no dot)
/// - `"acme.123bad"` → error (starts with digit)
pub fn parse_function_name(name: &str) -> Result<NamespacedName, BundlebaseError> {
    NamespacedName::parse(name, "Function")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_registry() -> FunctionRegistry {
        use crate::function::ipc_bridge::new_subprocess_cache;
        use crate::io::plugin::object_store::ObjectStoreDir;
        use url::Url;
        let ctx = Arc::new(SessionContext::new());
        let url = Url::parse("memory:///test").expect("valid url");
        let config = Arc::new(crate::BundleConfig::new(None).expect("valid config"));
        let dir = ObjectStoreDir::from_url(&url, config).expect("valid dir");
        let data_dir = Arc::new(RwLock::new(Arc::new(dir) as Arc<dyn IOReadWriteDir>));
        FunctionRegistry::new(data_dir, ctx, new_subprocess_cache())
    }

    // ==================== parse_function_name tests ====================

    #[test]
    fn test_parse_function_name_valid() {
        let nn = parse_function_name("acme.double_val").unwrap();
        assert_eq!(nn.namespace, "acme");
        assert_eq!(nn.name, "double_val");
    }

    #[test]
    fn test_parse_function_name_no_dot() {
        let result = parse_function_name("weather");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must be in format 'namespace.name'"));
    }

    #[test]
    fn test_parse_function_name_multi_level() {
        let result = parse_function_name("acme.datasources.weather");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Multi-level namespaces are not supported"));
    }

    #[test]
    fn test_parse_function_name_special_chars() {
        let result = parse_function_name("acme.bad-name");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_function_name_starts_with_digit() {
        let result = parse_function_name("acme.123func");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_function_name_empty_parts() {
        assert!(parse_function_name(".func").is_err());
        assert!(parse_function_name("acme.").is_err());
        assert!(parse_function_name(".").is_err());
    }

    #[test]
    fn test_parse_function_name_underscores() {
        let nn = parse_function_name("_my_ns._my_func").unwrap();
        assert_eq!(nn.namespace, "_my_ns");
        assert_eq!(nn.name, "_my_func");
    }

    // ==================== parse_arrow_type_name tests ====================

    #[test]
    fn test_parse_arrow_type_common() {
        assert_eq!(parse_arrow_type_name("Int64").unwrap(), DataType::Int64);
        assert_eq!(parse_arrow_type_name("Utf8").unwrap(), DataType::Utf8);
        assert_eq!(parse_arrow_type_name("Float64").unwrap(), DataType::Float64);
        assert_eq!(parse_arrow_type_name("Boolean").unwrap(), DataType::Boolean);
    }

    #[test]
    fn test_parse_arrow_type_all_int_types() {
        assert_eq!(parse_arrow_type_name("Int8").unwrap(), DataType::Int8);
        assert_eq!(parse_arrow_type_name("Int16").unwrap(), DataType::Int16);
        assert_eq!(parse_arrow_type_name("Int32").unwrap(), DataType::Int32);
        assert_eq!(parse_arrow_type_name("UInt8").unwrap(), DataType::UInt8);
        assert_eq!(parse_arrow_type_name("UInt16").unwrap(), DataType::UInt16);
        assert_eq!(parse_arrow_type_name("UInt32").unwrap(), DataType::UInt32);
        assert_eq!(parse_arrow_type_name("UInt64").unwrap(), DataType::UInt64);
    }

    #[test]
    fn test_parse_arrow_type_invalid() {
        let result = parse_arrow_type_name("NotAType");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown Arrow type name"));
    }

    // ==================== FunctionRegistry tests ====================

    fn make_entry(name: &str, logic: &str, temporary: bool) -> FunctionEntry {
        let nn = parse_function_name(name).unwrap();
        FunctionEntry {
            id: ObjectId::generate(),
            name: nn,
            input_types: vec![DataType::Int64],
            return_type: DataType::Int64,
            from: LogicRuntime::parse_from(&format!("ipc::{}", logic)).unwrap(),
            platform: Platform::any(),
            temporary,
            kind: FunctionKind::Scalar,
        }
    }

    #[test]
    fn test_registry_add_and_has() {
        let mut reg = test_registry();
        assert!(!reg.has("test.func"));
        reg.add(make_entry("test.func", "logic", false));
        assert!(reg.has("test.func"));
        assert!(!reg.has("test.other"));
    }

    #[test]
    fn test_registry_entries() {
        let mut reg = test_registry();
        reg.add(make_entry("test.func", "a", false));
        reg.add(make_entry("test.func2", "b", false));
        assert_eq!(reg.entries().len(), 2);
    }

    #[test]
    fn test_registry_resolve_not_found() {
        let reg = test_registry();
        let result = reg.resolve("test.func");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("is not defined"));
    }

    #[test]
    fn test_registry_resolve_last_set_wins() {
        let mut reg = test_registry();
        reg.add(make_entry("test.func", "first", false));
        reg.add(make_entry("test.func", "second", false));

        let resolved = reg.resolve("test.func").expect("should resolve");
        assert_eq!(resolved.from.to_logic_string(), "second");
    }

    #[test]
    fn test_registry_resolve_temporary_overrides_persistent() {
        let mut reg = test_registry();
        reg.add(make_entry("test.func", "persisted", false));
        let mut temp = make_entry("test.func", "temporary", true);
        temp.from = LogicRuntime::parse_from("python::temp:module").unwrap();
        reg.add(temp);

        let resolved = reg.resolve("test.func").expect("should resolve");
        assert_eq!(resolved.from.runtime_name(), "python");
        assert!(resolved.temporary);
    }

    #[test]
    fn test_registry_resolve_no_platform_match() {
        let mut reg = test_registry();
        let mut entry = make_entry("test.func", "test", false);
        entry.platform = "nonexistent/arch".parse().unwrap();
        reg.add(entry);

        let result = reg.resolve("test.func");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No function logic matches"));
    }

    #[test]
    fn test_registry_remove_all() {
        let mut reg = test_registry();
        reg.add(make_entry("test.func", "a", false));
        reg.add(make_entry("test.func", "b", true));
        reg.add(make_entry("test.other", "c", false));
        reg.remove_all("test.func");
        assert!(!reg.has("test.func"));
        assert!(reg.has("test.other"));
        assert_eq!(reg.entries().len(), 1);
    }

    #[test]
    fn test_registry_remove_temporary_only() {
        let mut reg = test_registry();
        reg.add(make_entry("test.func", "persisted", false));
        reg.add(make_entry("test.func", "temp", true));
        let removed = reg.remove("test.func", None, true);
        assert_eq!(removed, 1);
        assert_eq!(reg.entries().len(), 1);
        assert_eq!(reg.entries()[0].from.to_logic_string(), "persisted");
    }

    #[test]
    fn test_registry_namespaces() {
        let mut reg = test_registry();
        reg.add(make_entry("acme.func1", "a", false));
        reg.add(make_entry("acme.func2", "b", false));
        reg.add(make_entry("other.func3", "c", false));
        let ns = reg.namespaces();
        assert_eq!(ns, vec!["acme", "other"]);
    }

    #[test]
    fn test_registry_names() {
        let mut reg = test_registry();
        reg.add(make_entry("acme.func1", "a", false));
        reg.add(make_entry("other.func2", "b", false));
        let names = reg.names();
        assert_eq!(names, vec![
            NamespacedName::new("acme", "func1"),
            NamespacedName::new("other", "func2"),
        ]);
    }

    // ==================== serde roundtrip tests ====================

    #[test]
    fn test_function_entry_serde_roundtrip() {
        let entry = FunctionEntry {
            id: ObjectId::generate(),
            name: NamespacedName::new("acme", "double_val"),
            input_types: vec![DataType::Int64],
            return_type: DataType::Int64,
            from: LogicRuntime::parse_from("ipc::./my_func").unwrap(),
            platform: Platform::any(),
            temporary: false,
            kind: FunctionKind::Scalar,
        };
        let yaml = serde_yaml_ng::to_string(&entry).expect("serialize");
        let deser: FunctionEntry = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(deser, entry);
    }

    #[test]
    fn test_function_entry_serde_roundtrip_with_multiple_inputs() {
        let entry = FunctionEntry {
            id: ObjectId::generate(),
            name: NamespacedName::new("acme", "add"),
            input_types: vec![DataType::Int64, DataType::Int64],
            return_type: DataType::Int64,
            from: LogicRuntime::parse_from("python::my_module:add").unwrap(),
            platform: "linux/amd64".parse().unwrap(),
            temporary: true,
            kind: FunctionKind::Scalar,
        };
        let yaml = serde_yaml_ng::to_string(&entry).expect("serialize");
        let deser: FunctionEntry = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(deser, entry);
    }

    #[test]
    fn test_function_entry_serde_roundtrip_aggregate() {
        let entry = FunctionEntry {
            id: ObjectId::generate(),
            name: NamespacedName::new("acme", "my_sum"),
            input_types: vec![DataType::Int64],
            return_type: DataType::Int64,
            from: LogicRuntime::parse_from("python::my_module:MySum").unwrap(),
            platform: Platform::any(),
            temporary: true,
            kind: FunctionKind::Aggregate,
        };
        let yaml = serde_yaml_ng::to_string(&entry).expect("serialize");
        let deser: FunctionEntry = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(deser, entry);
    }

    #[test]
    fn test_function_kind_from_str() {
        assert_eq!("scalar".parse::<FunctionKind>().unwrap(), FunctionKind::Scalar);
        assert_eq!("aggregate".parse::<FunctionKind>().unwrap(), FunctionKind::Aggregate);
        assert_eq!("Scalar".parse::<FunctionKind>().unwrap(), FunctionKind::Scalar);
        assert_eq!("AGGREGATE".parse::<FunctionKind>().unwrap(), FunctionKind::Aggregate);
        assert!("unknown".parse::<FunctionKind>().is_err());
    }

    #[test]
    fn test_function_kind_display() {
        assert_eq!(FunctionKind::Scalar.to_string(), "scalar");
        assert_eq!(FunctionKind::Aggregate.to_string(), "aggregate");
    }

    // ==================== resolve_all tests ====================

    fn make_entry_with_types(name: &str, input_types: Vec<DataType>, temporary: bool, logic: &str) -> FunctionEntry {
        let nn = parse_function_name(name).unwrap();
        FunctionEntry {
            id: ObjectId::generate(),
            name: nn,
            input_types,
            return_type: DataType::Int64,
            from: LogicRuntime::parse_from(&format!("ipc::{}", logic)).unwrap(),
            platform: Platform::any(),
            temporary,
            kind: FunctionKind::Scalar,
        }
    }

    #[test]
    fn test_resolve_all_empty() {
        let reg = test_registry();
        assert!(reg.resolve_all("test.func").is_empty());
    }

    #[test]
    fn test_resolve_all_single_entry() {
        let mut reg = test_registry();
        reg.add(make_entry("test.func", "logic_a", false));
        let resolved = reg.resolve_all("test.func");
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].from.to_logic_string(), "logic_a");
    }

    #[test]
    fn test_resolve_all_two_overloads() {
        let mut reg = test_registry();
        reg.add(make_entry_with_types("test.func", vec![DataType::Int64], false, "int_logic"));
        reg.add(make_entry_with_types("test.func", vec![DataType::Utf8], false, "str_logic"));
        let resolved = reg.resolve_all("test.func");
        assert_eq!(resolved.len(), 2);
        let logics: Vec<String> = resolved.iter().map(|e| e.from.to_logic_string()).collect();
        assert!(logics.contains(&"int_logic".to_string()));
        assert!(logics.contains(&"str_logic".to_string()));
    }

    #[test]
    fn test_resolve_all_temp_shadows_persistent_per_signature() {
        let mut reg = test_registry();
        reg.add(make_entry_with_types("test.func", vec![DataType::Int64], false, "persisted_int"));
        reg.add(make_entry_with_types("test.func", vec![DataType::Int64], true, "temp_int"));
        reg.add(make_entry_with_types("test.func", vec![DataType::Utf8], false, "persisted_str"));
        let resolved = reg.resolve_all("test.func");
        assert_eq!(resolved.len(), 2);
        // The Int64 overload should be the temp one
        let int_entry = resolved.iter().find(|e| e.input_types == vec![DataType::Int64]).unwrap();
        assert_eq!(int_entry.from.to_logic_string(), "temp_int");
        assert!(int_entry.temporary);
        // The Utf8 overload should be the persistent one
        let str_entry = resolved.iter().find(|e| e.input_types == vec![DataType::Utf8]).unwrap();
        assert_eq!(str_entry.from.to_logic_string(), "persisted_str");
    }

    #[test]
    fn test_resolve_all_ignores_other_names() {
        let mut reg = test_registry();
        reg.add(make_entry("test.func", "a", false));
        reg.add(make_entry("test.other", "b", false));
        let resolved = reg.resolve_all("test.func");
        assert_eq!(resolved.len(), 1);
    }

    // ==================== validate_kind_consistency tests ====================

    #[test]
    fn test_validate_kind_consistency_all_scalar() {
        let entries = vec![
            make_entry("test.func", "a", false),
            make_entry("test.func", "b", false),
        ];
        let kind = validate_kind_consistency(&entries).unwrap();
        assert_eq!(kind, FunctionKind::Scalar);
    }

    #[test]
    fn test_validate_kind_consistency_mixed_kinds() {
        let mut scalar = make_entry("test.func", "a", false);
        scalar.kind = FunctionKind::Scalar;
        let mut agg = make_entry("test.func", "b", false);
        agg.kind = FunctionKind::Aggregate;
        let result = validate_kind_consistency(&[scalar, agg]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("mixed kinds"));
    }

    #[test]
    fn test_validate_kind_consistency_empty() {
        let result = validate_kind_consistency(&[]);
        assert!(result.is_err());
    }

    // ==================== remove_by_signature tests ====================

    #[test]
    fn test_remove_by_signature_specific() {
        let mut reg = test_registry();
        reg.add(make_entry_with_types("test.func", vec![DataType::Int64], false, "int_logic"));
        reg.add(make_entry_with_types("test.func", vec![DataType::Utf8], false, "str_logic"));
        reg.remove_by_signature("test.func", Some(&[DataType::Int64]));
        assert_eq!(reg.entries().len(), 1);
        assert_eq!(reg.entries()[0].from.to_logic_string(), "str_logic");
    }

    #[test]
    fn test_remove_by_signature_none_removes_all() {
        let mut reg = test_registry();
        reg.add(make_entry_with_types("test.func", vec![DataType::Int64], false, "int_logic"));
        reg.add(make_entry_with_types("test.func", vec![DataType::Utf8], false, "str_logic"));
        reg.remove_by_signature("test.func", None);
        assert!(reg.entries().is_empty());
    }

    #[test]
    fn test_remove_by_signature_preserves_other_names() {
        let mut reg = test_registry();
        reg.add(make_entry_with_types("test.func", vec![DataType::Int64], false, "a"));
        reg.add(make_entry_with_types("test.other", vec![DataType::Int64], false, "b"));
        reg.remove_by_signature("test.func", Some(&[DataType::Int64]));
        assert_eq!(reg.entries().len(), 1);
        assert_eq!(reg.entries()[0].name.name, "other");
    }
}
