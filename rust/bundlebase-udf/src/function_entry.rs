//! User-defined function entry system for named, platform-aware SQL functions.
//!
//! A `FunctionEntry` is created via `IMPORT FUNCTION acme.double_val(Int64) RETURNS Int64`
//! and represents a single function binding for a name+platform pair.
//! `resolve_function` picks the best entry for the current platform at runtime.

use crate::bridge::ipc_bridge::SubprocessCache;
use crate::bridge::version_function::VersionFunction;
use crate::runtime::UdfRuntime;
use arrow::datatypes::DataType;
use bundlebase_common::namespaced_name::NamespacedName;
use bundlebase_common::object_id::ObjectId;
use bundlebase_common::platform::Platform;
use bundlebase_common::BundlebaseError;
use bundlebase_io::IOReadWriteDir;
use datafusion::logical_expr::ScalarUDF;
use datafusion::prelude::SessionContext;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

const INTERNAL_FUNCTION_PREFIX: &str = "fn_";

/// Stable internal name for a registered UDF: `fn_<hex_id>`.
///
/// DataFusion sees only this name; user-visible names live in
/// `FunctionRegistry::name_to_internal_id`. RENAME FUNCTION updates the
/// map but leaves the DataFusion registration alone.
pub fn internal_function_name(id: &ObjectId) -> String {
    format!("{}{}", INTERNAL_FUNCTION_PREFIX, id)
}

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
    pub input_types: Vec<DataType>,
    /// Arrow type for the return value (e.g., DataType::Int64)
    pub return_type: DataType,
    /// Runtime with parsed entrypoint (e.g., FfiRuntime { path: "./mylib.so", symbol: Some("double_val") })
    pub from: UdfRuntime,
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
    /// Stable internal ObjectId per user-visible function name. The DataFusion
    /// registration uses `fn_<id>`, so RENAME FUNCTION moves a key in this map
    /// but never deregisters/re-registers the underlying composite UDF.
    /// First registration of a name claims a fresh id (the first overload's
    /// ObjectId); subsequent overloads of the same name reuse it.
    name_to_internal_id: HashMap<String, ObjectId>,
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
            name_to_internal_id: HashMap::new(),
            data_dir,
            ctx,
            subprocess_cache,
        }
    }

    /// Stable internal ObjectId for a user-visible function name, if registered.
    pub fn internal_id(&self, name: &str) -> Option<ObjectId> {
        self.name_to_internal_id.get(name).copied()
    }

    /// Build a `user_name → fn_<id>` map for SQL translation.
    pub fn name_to_internal_name(&self) -> HashMap<String, String> {
        self.name_to_internal_id
            .iter()
            .map(|(name, id)| (name.clone(), internal_function_name(id)))
            .collect()
    }

    /// Add a function entry to the registry without registering with DataFusion.
    /// Prefer `add_and_register` for production use — it also validates kind consistency
    /// and registers with DataFusion.
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
        let matching: Vec<&FunctionEntry> =
            self.entries.iter().filter(|e| e.name == name).collect();

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
            "No function entrypoint matches current platform '{}' for function '{}'. Available platforms: {}",
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

    /// Return function names where ALL entries are temporary (no persistent entry exists).
    ///
    /// A name is "temporary-only" if it has at least one temporary entry and zero persistent entries.
    /// If a persistent entry shadows a temporary one (same name), the name is NOT included.
    pub fn temporary_only_names(&self) -> Vec<String> {
        let mut temp_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut persistent_names: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        for entry in &self.entries {
            let name_str = entry.name.to_string();
            if entry.temporary {
                temp_names.insert(name_str);
            } else {
                persistent_names.insert(name_str);
            }
        }

        temp_names.difference(&persistent_names).cloned().collect()
    }

    /// Resolve all overloads for a name and register them as a composite
    /// DataFusion UDF/UDAF/UDTF under the stable internal name `fn_<id>`.
    ///
    /// The internal id is assigned the first time a name is registered and
    /// reused for every subsequent overload — so RENAME FUNCTION can move
    /// the user-visible mapping without re-registering anything in
    /// DataFusion. SQL translation at command time rewrites user-visible
    /// names to `fn_<id>` before queries reach DataFusion.
    ///
    /// Uses the registry's `data_dir` to resolve bundle-relative entrypoint
    /// paths and `subprocess_cache` for IPC-based functions.
    pub fn register_functions_for_name(&mut self, name: &str) -> Result<(), BundlebaseError> {
        let overloads = self.resolve_all(name);
        if overloads.is_empty() {
            // Name has no live entries; release its internal id slot.
            self.name_to_internal_id.remove(name);
            return Ok(());
        }

        // Reuse the existing internal id, or claim the first overload's
        // ObjectId on first registration.
        let internal_id = *self
            .name_to_internal_id
            .entry(name.to_string())
            .or_insert_with(|| overloads[0].id);
        let internal_name = internal_function_name(&internal_id);

        let data_dir = self.data_dir.read().clone();

        // Resolve bundle-relative entrypoint paths against the data directory
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
                use crate::bridge::scalar::ScalarFunction;
                let func = ScalarFunction::new_composite(
                    overloads,
                    Arc::clone(&self.subprocess_cache),
                )?
                .with_name(internal_name.clone());
                self.ctx.register_udf(ScalarUDF::from(func));
            }
            FunctionKind::Aggregate => {
                use crate::bridge::aggregate::AggregateFunction;
                use datafusion::logical_expr::AggregateUDF;
                let agg = AggregateFunction::new_composite(
                    overloads,
                    Arc::clone(&self.subprocess_cache),
                )?
                .with_name(internal_name.clone());
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
    pub fn names(&self) -> Vec<NamespacedName> {
        self.entries.iter().map(|e| e.name.clone()).collect()
    }

    /// Resolve all function entries for a name, grouped by input type signature.
    ///
    /// For each unique input type signature, picks the best platform match
    /// (temporary entries shadow persistent, last wins within each tier).
    /// Returns one entry per distinct signature.
    pub fn resolve_all(&self, name: &str) -> Vec<FunctionEntry> {
        let matching: Vec<&FunctionEntry> =
            self.entries.iter().filter(|e| e.name == name).collect();

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

    /// Remove function entries by their IDs (low-level).
    /// Prefer `drop_by_ids` which also handles deregistration and re-registration.
    fn remove_by_ids(&mut self, ids: &[ObjectId]) {
        self.entries.retain(|e| !ids.contains(&e.id));
    }

    /// Rename function entries matching the given IDs to a new name (low-level).
    /// Prefer `rename_by_ids` which also handles deregistration and re-registration.
    fn rename_entries(&mut self, ids: &[ObjectId], new_name: &NamespacedName) {
        for entry in &mut self.entries {
            if ids.contains(&entry.id) {
                entry.name = new_name.clone();
            }
        }
    }

    /// Rename only temporary function entries matching the old name to a new name (low-level).
    /// Prefer `rename_temp` which also handles validation, deregistration, and re-registration.
    fn rename_temp_entries(&mut self, old_name: &str, new_name: &NamespacedName) {
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
    pub fn remove_by_signature(&mut self, name: &str, input_types: Option<&[DataType]>) {
        match input_types {
            None => self.remove_all(name),
            Some(types) => {
                self.entries
                    .retain(|e| !(e.name == name && e.input_types == types));
            }
        }
    }

    // --- Composite operations (mutate + register) ---

    /// Deregister the DataFusion UDF/UDAF for a user-visible name, if any.
    /// Looks up the internal `fn_<id>` name from the registry's mapping —
    /// the user-visible name is never seen by DataFusion directly.
    fn deregister_by_user_name(&self, name: &str) {
        if let Some(id) = self.name_to_internal_id.get(name) {
            let internal = internal_function_name(id);
            let _ = self.ctx.deregister_udf(&internal);
            let _ = self.ctx.deregister_udaf(&internal);
        }
    }

    /// Look up a function name from a set of entry IDs.
    pub fn name_for_ids(&self, ids: &[ObjectId]) -> Option<String> {
        self.entries
            .iter()
            .find(|e| ids.contains(&e.id))
            .map(|e| e.name.to_string())
    }

    /// Add a function entry, validate kind consistency, and register with DataFusion.
    ///
    /// This is the preferred way to add function entries. It:
    /// 1. Validates that the new entry's kind matches existing overloads
    /// 2. Adds the entry to the registry
    /// 3. Registers all overloads for the name with DataFusion
    pub fn add_and_register(&mut self, entry: FunctionEntry) -> Result<(), BundlebaseError> {
        let name = entry.name.to_string();

        // Validate kind consistency with existing entries
        let existing = self.resolve_all(&name);
        if !existing.is_empty() {
            let existing_kind = existing[0].kind;
            if entry.kind != existing_kind {
                return Err(format!(
                    "Function '{}' has overloads with mixed kinds (scalar and aggregate). \
                     All overloads of a function must be the same kind.",
                    name
                )
                .into());
            }
        }

        self.add(entry);
        self.register_functions_for_name(&name)
    }

    /// Remove function entries by ID and re-register the remaining overloads
    /// (under the SAME stable internal `fn_<id>`). If no overloads remain,
    /// deregister the internal name and release its slot.
    pub fn drop_by_ids(&mut self, ids: &[ObjectId]) -> Result<(), BundlebaseError> {
        let name = self.name_for_ids(ids);
        self.remove_by_ids(ids);

        if let Some(name) = name {
            if self.resolve_all(&name).is_empty() {
                self.deregister_by_user_name(&name);
                self.name_to_internal_id.remove(&name);
            } else {
                self.register_functions_for_name(&name)?;
            }
        }
        Ok(())
    }

    /// Rename function entries by ID. The DataFusion registration is keyed
    /// off the stable internal `fn_<id>` and is left untouched — this is a
    /// pure metadata move on `name_to_internal_id` plus updating the entry
    /// `name` fields.
    pub fn rename_by_ids(
        &mut self,
        ids: &[ObjectId],
        new_name: &NamespacedName,
    ) -> Result<(), BundlebaseError> {
        let old_name = self.name_for_ids(ids);

        self.rename_entries(ids, new_name);

        if let Some(old_name) = old_name {
            if let Some(internal_id) = self.name_to_internal_id.remove(&old_name) {
                self.name_to_internal_id
                    .insert(new_name.to_string(), internal_id);
            }
        }
        Ok(())
    }

    /// Remove temporary function entries. If overloads remain for the name,
    /// re-register the composite (same internal id); otherwise drop the slot.
    pub fn drop_temp(
        &mut self,
        name: &str,
        platform: Option<&Platform>,
    ) -> Result<usize, BundlebaseError> {
        let removed = self.remove(name, platform, true);
        if self.resolve_all(name).is_empty() {
            self.deregister_by_user_name(name);
            self.name_to_internal_id.remove(name);
        } else {
            self.register_functions_for_name(name)?;
        }
        Ok(removed)
    }

    /// Rename temporary function entries. Like `rename_by_ids`, this is a
    /// metadata-only move — the DataFusion registration stays put.
    pub fn rename_temp(
        &mut self,
        old_name: &str,
        new_name: &NamespacedName,
    ) -> Result<(), BundlebaseError> {
        // Validate old name has temporary entries
        let has_temp = self
            .entries
            .iter()
            .any(|e| e.temporary && e.name == old_name);
        if !has_temp {
            return Err(format!(
                "No temporary function entries found for '{}'. Use IMPORT TEMP FUNCTION first.",
                old_name
            )
            .into());
        }

        // Check new name doesn't conflict
        let new_name_str = new_name.to_string();
        if self.has(&new_name_str) {
            return Err(format!(
                "Function '{}' already exists. Drop it first or choose a different name.",
                new_name_str
            )
            .into());
        }

        self.rename_temp_entries(old_name, new_name);
        if let Some(internal_id) = self.name_to_internal_id.remove(old_name) {
            self.name_to_internal_id.insert(new_name_str, internal_id);
        }
        Ok(())
    }
}

/// Validate that all entries share the same FunctionKind (scalar or aggregate).
///
/// Returns the consistent kind, or an error if entries mix scalar and aggregate.
pub fn validate_kind_consistency(
    entries: &[FunctionEntry],
) -> Result<FunctionKind, BundlebaseError> {
    let first = entries
        .first()
        .ok_or_else(|| BundlebaseError::from("No function entries provided for kind validation"))?;
    let expected = first.kind;
    for entry in entries.iter().skip(1) {
        if entry.kind != expected {
            return Err(format!(
                "Function '{}' has overloads with mixed kinds (scalar and aggregate). \
                 All overloads of a function must be the same kind.",
                first.name
            )
            .into());
        }
    }
    Ok(expected)
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

    struct EmptyConfigProvider;
    impl bundlebase_common::ConfigProvider for EmptyConfigProvider {
        fn get_in_scope(
            &self,
            _scope: &bundlebase_common::Scope,
            _key: &bundlebase_common::ConfigKey,
        ) -> Result<Option<String>, bundlebase_common::BundlebaseError> {
            Ok(None)
        }
    }

    fn test_registry() -> FunctionRegistry {
        use crate::bridge::ipc_bridge::new_subprocess_cache;
        use bundlebase_io::plugin::object_store::ObjectStoreDir;
        use url::Url;
        let ctx = Arc::new(SessionContext::new());
        let url = Url::parse("memory:///test").expect("valid url");
        let config: Arc<dyn bundlebase_common::ConfigProvider> = Arc::new(EmptyConfigProvider);

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
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must be in format 'namespace.name'"));
    }

    #[test]
    fn test_parse_function_name_multi_level() {
        let result = parse_function_name("acme.datasources.weather");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Multi-level namespaces are not supported"));
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

    // ==================== FunctionRegistry tests ====================

    fn make_entry(name: &str, entrypoint: &str, temporary: bool) -> FunctionEntry {
        let nn = parse_function_name(name).unwrap();
        FunctionEntry {
            id: ObjectId::generate(),
            name: nn,
            input_types: vec![DataType::Int64],
            return_type: DataType::Int64,
            from: UdfRuntime::parse_from(&format!("ipc::{}", entrypoint)).unwrap(),
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
        assert_eq!(resolved.from.to_entrypoint_string(), "second");
    }

    #[test]
    fn test_registry_resolve_temporary_overrides_persistent() {
        let mut reg = test_registry();
        reg.add(make_entry("test.func", "persisted", false));
        let mut temp = make_entry("test.func", "temporary", true);
        temp.from = UdfRuntime::parse_from("python::temp:module").unwrap();
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
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No function entrypoint matches"));
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
        assert_eq!(reg.entries()[0].from.to_entrypoint_string(), "persisted");
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
        assert_eq!(
            names,
            vec![
                NamespacedName::new("acme", "func1"),
                NamespacedName::new("other", "func2"),
            ]
        );
    }

    // ==================== serde roundtrip tests ====================

    #[test]
    fn test_function_entry_serde_roundtrip() {
        let entry = FunctionEntry {
            id: ObjectId::generate(),
            name: NamespacedName::new("acme", "double_val"),
            input_types: vec![DataType::Int64],
            return_type: DataType::Int64,
            from: UdfRuntime::parse_from("ipc::./my_func").unwrap(),
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
            from: UdfRuntime::parse_from("python::my_module:add").unwrap(),
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
            from: UdfRuntime::parse_from("python::my_module:MySum").unwrap(),
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
        assert_eq!(
            "scalar".parse::<FunctionKind>().unwrap(),
            FunctionKind::Scalar
        );
        assert_eq!(
            "aggregate".parse::<FunctionKind>().unwrap(),
            FunctionKind::Aggregate
        );
        assert_eq!(
            "Scalar".parse::<FunctionKind>().unwrap(),
            FunctionKind::Scalar
        );
        assert_eq!(
            "AGGREGATE".parse::<FunctionKind>().unwrap(),
            FunctionKind::Aggregate
        );
        assert!("unknown".parse::<FunctionKind>().is_err());
    }

    #[test]
    fn test_function_kind_display() {
        assert_eq!(FunctionKind::Scalar.to_string(), "scalar");
        assert_eq!(FunctionKind::Aggregate.to_string(), "aggregate");
    }

    // ==================== resolve_all tests ====================

    fn make_entry_with_types(
        name: &str,
        input_types: Vec<DataType>,
        temporary: bool,
        entrypoint: &str,
    ) -> FunctionEntry {
        let nn = parse_function_name(name).unwrap();
        FunctionEntry {
            id: ObjectId::generate(),
            name: nn,
            input_types,
            return_type: DataType::Int64,
            from: UdfRuntime::parse_from(&format!("ipc::{}", entrypoint)).unwrap(),
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
        assert_eq!(resolved[0].from.to_entrypoint_string(), "logic_a");
    }

    #[test]
    fn test_resolve_all_two_overloads() {
        let mut reg = test_registry();
        reg.add(make_entry_with_types(
            "test.func",
            vec![DataType::Int64],
            false,
            "int_logic",
        ));
        reg.add(make_entry_with_types(
            "test.func",
            vec![DataType::Utf8],
            false,
            "str_logic",
        ));
        let resolved = reg.resolve_all("test.func");
        assert_eq!(resolved.len(), 2);
        let entrypoints: Vec<String> = resolved
            .iter()
            .map(|e| e.from.to_entrypoint_string())
            .collect();
        assert!(entrypoints.contains(&"int_logic".to_string()));
        assert!(entrypoints.contains(&"str_logic".to_string()));
    }

    #[test]
    fn test_resolve_all_temp_shadows_persistent_per_signature() {
        let mut reg = test_registry();
        reg.add(make_entry_with_types(
            "test.func",
            vec![DataType::Int64],
            false,
            "persisted_int",
        ));
        reg.add(make_entry_with_types(
            "test.func",
            vec![DataType::Int64],
            true,
            "temp_int",
        ));
        reg.add(make_entry_with_types(
            "test.func",
            vec![DataType::Utf8],
            false,
            "persisted_str",
        ));
        let resolved = reg.resolve_all("test.func");
        assert_eq!(resolved.len(), 2);
        // The Int64 overload should be the temp one
        let int_entry = resolved
            .iter()
            .find(|e| e.input_types == vec![DataType::Int64])
            .unwrap();
        assert_eq!(int_entry.from.to_entrypoint_string(), "temp_int");
        assert!(int_entry.temporary);
        // The Utf8 overload should be the persistent one
        let str_entry = resolved
            .iter()
            .find(|e| e.input_types == vec![DataType::Utf8])
            .unwrap();
        assert_eq!(str_entry.from.to_entrypoint_string(), "persisted_str");
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
        reg.add(make_entry_with_types(
            "test.func",
            vec![DataType::Int64],
            false,
            "int_logic",
        ));
        reg.add(make_entry_with_types(
            "test.func",
            vec![DataType::Utf8],
            false,
            "str_logic",
        ));
        reg.remove_by_signature("test.func", Some(&[DataType::Int64]));
        assert_eq!(reg.entries().len(), 1);
        assert_eq!(reg.entries()[0].from.to_entrypoint_string(), "str_logic");
    }

    #[test]
    fn test_remove_by_signature_none_removes_all() {
        let mut reg = test_registry();
        reg.add(make_entry_with_types(
            "test.func",
            vec![DataType::Int64],
            false,
            "int_logic",
        ));
        reg.add(make_entry_with_types(
            "test.func",
            vec![DataType::Utf8],
            false,
            "str_logic",
        ));
        reg.remove_by_signature("test.func", None);
        assert!(reg.entries().is_empty());
    }

    #[test]
    fn test_remove_by_signature_preserves_other_names() {
        let mut reg = test_registry();
        reg.add(make_entry_with_types(
            "test.func",
            vec![DataType::Int64],
            false,
            "a",
        ));
        reg.add(make_entry_with_types(
            "test.other",
            vec![DataType::Int64],
            false,
            "b",
        ));
        reg.remove_by_signature("test.func", Some(&[DataType::Int64]));
        assert_eq!(reg.entries().len(), 1);
        assert_eq!(reg.entries()[0].name.name, "other");
    }

    // ==================== temporary_only_names tests ====================

    #[test]
    fn test_temporary_only_names_empty_registry() {
        let reg = test_registry();
        assert!(reg.temporary_only_names().is_empty());
    }

    #[test]
    fn test_temporary_only_names_only_persistent() {
        let mut reg = test_registry();
        reg.add(make_entry("test.func", "a", false));
        assert!(reg.temporary_only_names().is_empty());
    }

    #[test]
    fn test_temporary_only_names_only_temp() {
        let mut reg = test_registry();
        reg.add(make_entry("test.func", "a", true));
        let names = reg.temporary_only_names();
        assert_eq!(names.len(), 1);
        assert!(names.contains(&"test.func".to_string()));
    }

    #[test]
    fn test_temporary_only_names_shadowed_by_persistent() {
        let mut reg = test_registry();
        reg.add(make_entry("test.func", "persistent", false));
        reg.add(make_entry("test.func", "temporary", true));
        // Has both persistent and temporary — NOT temporary-only
        assert!(reg.temporary_only_names().is_empty());
    }

    #[test]
    fn test_temporary_only_names_mixed() {
        let mut reg = test_registry();
        reg.add(make_entry("test.temp_only", "a", true));
        reg.add(make_entry("test.shadowed", "b", false));
        reg.add(make_entry("test.shadowed", "c", true));
        reg.add(make_entry("test.persistent", "d", false));
        let names = reg.temporary_only_names();
        assert_eq!(names.len(), 1);
        assert!(names.contains(&"test.temp_only".to_string()));
    }

    // ==================== fn_<id> stable internal name tests ====================
    use datafusion::execution::FunctionRegistry as DfFunctionRegistry;


    /// `register_functions_for_name` should register the composite UDF in
    /// DataFusion under `fn_<id>` rather than the user-visible name. The
    /// id is the first overload's ObjectId and stays stable for the life
    /// of the name.
    #[test]
    fn test_register_uses_fn_internal_name() {
        let mut reg = test_registry();
        let entry = make_entry("test.foo", "a", false);
        let entry_id = entry.id;
        reg.add_and_register(entry).expect("add_and_register");

        // Internal id is the entry's ObjectId.
        assert_eq!(reg.internal_id("test.foo"), Some(entry_id));

        // DataFusion sees the function under fn_<id>, not under "test.foo".
        let internal = internal_function_name(&entry_id);
        let session = &reg.ctx;
        assert!(
            session.udf(&internal).is_ok(),
            "udf '{}' should be registered with DataFusion",
            internal
        );
        assert!(
            session.udf("test.foo").is_err(),
            "user-visible name should NOT be registered with DataFusion"
        );
    }

    /// Multiple overloads of the same name share one stable `fn_<id>` —
    /// the first overload's id wins; subsequent overloads do not get a
    /// fresh DataFusion registration of their own.
    #[test]
    fn test_overloads_share_internal_id() {
        let mut reg = test_registry();
        let mut e1 = make_entry("test.foo", "int_logic", false);
        e1.input_types = vec![DataType::Int64];
        let id1 = e1.id;
        reg.add_and_register(e1).expect("add e1");

        let mut e2 = make_entry("test.foo", "str_logic", false);
        e2.input_types = vec![DataType::Utf8];
        reg.add_and_register(e2).expect("add e2");

        assert_eq!(reg.internal_id("test.foo"), Some(id1));
        // Composite registered exactly once under fn_<id1>.
        assert!(reg.ctx.udf(&internal_function_name(&id1)).is_ok());
    }

    /// RENAME FUNCTION must be metadata-only: the DataFusion registration
    /// keeps its `fn_<id>` name; only `name_to_internal_id` shifts the key
    /// from old → new.
    #[test]
    fn test_rename_does_not_touch_datafusion() {
        let mut reg = test_registry();
        let entry = make_entry("test.foo", "a", false);
        let entry_id = entry.id;
        reg.add_and_register(entry).expect("add");

        let internal = internal_function_name(&entry_id);
        assert!(reg.ctx.udf(&internal).is_ok());

        let new_name = NamespacedName::new("test", "bar");
        reg.rename_by_ids(&[entry_id], &new_name).expect("rename");

        // Internal id moved from foo → bar but DataFusion registration is
        // still keyed by fn_<id>.
        assert_eq!(reg.internal_id("test.foo"), None);
        assert_eq!(reg.internal_id("test.bar"), Some(entry_id));
        assert!(
            reg.ctx.udf(&internal).is_ok(),
            "fn_<id> registration should survive rename"
        );
    }

    /// DROP of all overloads should release the `fn_<id>` slot AND
    /// deregister it from DataFusion (otherwise an orphan UDF lingers).
    #[test]
    fn test_drop_all_releases_internal_id() {
        let mut reg = test_registry();
        let entry = make_entry("test.foo", "a", false);
        let entry_id = entry.id;
        reg.add_and_register(entry).expect("add");
        let internal = internal_function_name(&entry_id);
        assert!(reg.ctx.udf(&internal).is_ok());

        reg.drop_by_ids(&[entry_id]).expect("drop");

        assert_eq!(reg.internal_id("test.foo"), None);
        assert!(
            reg.ctx.udf(&internal).is_err(),
            "fn_<id> registration must be deregistered when all overloads are dropped"
        );
    }
}
