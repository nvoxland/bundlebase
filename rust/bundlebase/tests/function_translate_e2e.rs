//! End-to-end checks for the `fn_<id>` UDF normalization:
//!   * imported functions register with DataFusion under `fn_<id>`,
//!   * `BundleBuilder::translate_sql` rewrites user-visible function
//!     references to `fn_<id>` in arbitrary SQL fragments,
//!   * `RENAME FUNCTION` is metadata-only — the DataFusion registration
//!     keeps its `fn_<id>` and the new name translates to the same id.
//!
//! Mirrors the existing `col_<id>` invariant for columns.

use arrow::datatypes::DataType;
use bundlebase::bundle::function_entry::{
    internal_function_name, FunctionEntry, FunctionKind,
};
use bundlebase::bundle::BundleFacade;
use bundlebase_common::namespaced_name::NamespacedName;
use bundlebase_common::platform::Platform;
use bundlebase::test_utils::random_memory_url;
use bundlebase::bundle::UdfRuntime;
use bundlebase_common::object_id::ObjectId;
use bundlebase_common::BundlebaseError;
use datafusion::execution::FunctionRegistry as DfFunctionRegistry;

mod common;
fn init() {
    common::init_catalog();
}

fn fake_entry(name: &str, entrypoint: &str) -> FunctionEntry {
    FunctionEntry {
        id: ObjectId::generate(),
        name: NamespacedName::parse(name, "Function").expect("valid namespaced name"),
        input_types: vec![DataType::Int64],
        return_type: DataType::Int64,
        from: UdfRuntime::parse_from(&format!("ipc::{}", entrypoint)).unwrap(),
        platform: Platform::any(),
        temporary: false,
        kind: FunctionKind::Scalar,
    }
}

#[tokio::test]
async fn test_function_registers_under_fn_internal_name() -> Result<(), BundlebaseError> {
    init();
    let builder = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;

    let entry = fake_entry("acme.foo", "stub");
    let id = entry.id;
    builder
        .bundle()
        .function_registry()
        .write()
        .add_and_register(entry)?;

    let internal = internal_function_name(&id);
    let ctx = builder.bundle().ctx();
    assert!(ctx.udf(&internal).is_ok(), "fn_<id> registered with DataFusion");
    assert!(
        ctx.udf("acme.foo").is_err(),
        "user-visible name must not leak to DataFusion"
    );

    Ok(())
}

#[tokio::test]
async fn test_translate_sql_rewrites_function_calls() -> Result<(), BundlebaseError> {
    init();
    let builder = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;

    let entry = fake_entry("acme.foo", "stub");
    let id = entry.id;
    builder
        .bundle()
        .function_registry()
        .write()
        .add_and_register(entry)?;

    let internal = internal_function_name(&id);
    let translated = builder.translate_sql("SELECT acme.foo(x) FROM bundle");
    assert!(
        translated.contains(&format!("{}(x)", internal)),
        "expected `acme.foo(x)` rewritten to `{}(x)`, got {:?}",
        internal,
        translated
    );
    assert!(
        !translated.contains("acme.foo("),
        "no user-visible function name should remain after translation"
    );

    Ok(())
}

#[tokio::test]
async fn test_rename_function_is_metadata_only() -> Result<(), BundlebaseError> {
    init();
    let builder = bundlebase::BundleBuilder::create(random_memory_url().as_str(), None).await?;

    let entry = fake_entry("acme.foo", "stub");
    let id = entry.id;
    builder
        .bundle()
        .function_registry()
        .write()
        .add_and_register(entry)?;

    let internal = internal_function_name(&id);
    let ctx = builder.bundle().ctx();
    assert!(ctx.udf(&internal).is_ok(), "registered before rename");

    let new_name = NamespacedName::parse("acme.bar", "Function")?;
    builder
        .bundle()
        .function_registry()
        .write()
        .rename_by_ids(&[id], &new_name)?;

    // DataFusion registration is unchanged after rename.
    assert!(
        ctx.udf(&internal).is_ok(),
        "rename must not deregister the fn_<id>"
    );

    // The new name now translates to the same fn_<id>.
    let translated = builder.translate_sql("SELECT acme.bar(x)");
    assert!(translated.contains(&format!("{}(x)", internal)));

    // Old name no longer translates (and isn't registered with DataFusion either).
    let translated_old = builder.translate_sql("SELECT acme.foo(x)");
    assert!(translated_old.contains("acme.foo("), "old name shouldn't map");

    Ok(())
}
