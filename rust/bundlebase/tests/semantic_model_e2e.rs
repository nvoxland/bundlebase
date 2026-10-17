//! Native semantic commands and optional document adapters share ordinary history.

use bundlebase::bundle::operation::AnyOperation;
use bundlebase::bundle::semantic_model::{document_operations, export_semantic_model};
use bundlebase::bundle::{BundleFacade, INIT_FILENAME};
use bundlebase::test_utils::{random_memory_url, test_config, test_datafile};
use bundlebase::{Bundle, BundleBuilder};
use bundlebase_command::{BundleBuilderExt, BundleFacadeCommandExt};
use bundlebase_common::BundlebaseError;
use bundlebase_io::IOReadWriteDir;
use rstest::rstest;
use serde_json::{json, Value};
use std::sync::Arc;

mod common;

struct Location {
    path: String,
    _tmp: Option<tempfile::TempDir>,
}

fn location(backend: &str) -> Result<Location, BundlebaseError> {
    match backend {
        "memory" => Ok(Location {
            path: random_memory_url().to_string(),
            _tmp: None,
        }),
        "local" => {
            let tmp = tempfile::tempdir()?;
            Ok(Location {
                path: tmp.path().to_string_lossy().into_owned(),
                _tmp: Some(tmp),
            })
        }
        other => Err(format!("Unknown backend {other}").into()),
    }
}

async fn setup(path: &str) -> Result<Arc<BundleBuilder>, BundlebaseError> {
    common::init_catalog();
    let builder = BundleBuilder::create(path, None).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    Ok(builder)
}

async fn command(builder: &BundleBuilder, sql: &str) -> Result<(), BundlebaseError> {
    builder.execute(sql, vec![]).await?;
    Ok(())
}

async fn commands(builder: &BundleBuilder, statements: &[&str]) -> Result<(), BundlebaseError> {
    for statement in statements {
        command(builder, statement).await?;
    }
    Ok(())
}

fn field_mut<'a>(document: &'a mut Value, name: &str) -> Result<&'a mut Value, BundlebaseError> {
    document["datasets"][0]["fields"]
        .as_array_mut()
        .ok_or("Missing fields")?
        .iter_mut()
        .find(|field| field["name"] == name)
        .ok_or_else(|| format!("Missing field {name}").into())
}

fn object_remove(value: &mut Value, key: &str) -> Result<(), BundlebaseError> {
    value
        .as_object_mut()
        .ok_or("Expected document object")?
        .remove(key);
    Ok(())
}

#[rstest]
#[case::memory("memory")]
#[case::local("local")]
#[tokio::test]
async fn native_properties_keys_and_metric_lifecycle(
    #[case] backend: &str,
) -> Result<(), BundlebaseError> {
    let loc = location(backend)?;
    let builder = setup(&loc.path).await?;
    commands(&builder, &[
        "ALTER COLUMN salary SET DESCRIPTION 'Annual gross pay'",
        "ALTER COLUMN salary SET LABEL 'Annual pay'",
        "ALTER COLUMN salary SET AI CONTEXT '{\"instructions\":\"Use annual units\",\"custom\":\"keep\"}'",
        "ALTER COLUMN salary SET AI CONTEXT MEMBER examples '[\"payroll\"]'",
        "ALTER COLUMN salary DROP AI CONTEXT MEMBER instructions",
        "ALTER DATASET bundle SET DESCRIPTION 'Employees'",
        "ALTER DATASET bundle SET AI CONTEXT 'One row per employee'",
        "ALTER SEMANTIC MODEL SET AI CONTEXT '{\"instructions\":\"Payroll reporting\"}'",
        "ALTER SEMANTIC MODEL SET AI CONTEXT MEMBER synonyms '[\"staff\"]'",
        "DECLARE PRIMARY KEY (id)",
        "DECLARE UNIQUE KEY (first_name, last_name)",
        "CREATE METRIC payroll AS 'SUM(salary)' DESCRIPTION 'Gross payroll'",
        "ALTER METRIC payroll SET AI CONTEXT '{\"instructions\":\"Compare within year\"}'",
        "CREATE METRIC payroll AS 'SUM(salary) * 2'",
    ]).await?;
    let salary = builder.column_id("salary").ok_or("Missing salary")?;
    let before_rename = builder.semantic_state();
    assert_eq!(
        before_rename.columns[&salary].description.as_deref(),
        Some("Annual gross pay")
    );
    assert_eq!(
        before_rename.columns[&salary].label.as_deref(),
        Some("Annual pay")
    );
    assert_eq!(
        before_rename.columns[&salary].ai_context,
        Some(json!({"custom":"keep","examples":["payroll"]}))
    );
    assert_eq!(
        before_rename.metrics["payroll"].description.as_deref(),
        Some("Gross payroll")
    );
    assert_eq!(
        before_rename.metrics["payroll"].ai_context,
        Some(json!({"instructions":"Compare within year"}))
    );
    assert!(before_rename.primary_key.is_some());
    assert_eq!(before_rename.unique_keys.len(), 1);
    let change_count = builder.status().changes().len();
    let operation_count = builder.status().operations_count();
    commands(
        &builder,
        &[
            "ALTER COLUMN salary SET DESCRIPTION 'Annual gross pay'",
            "ALTER COLUMN salary SET LABEL 'Annual pay'",
            "ALTER COLUMN salary SET AI CONTEXT '{\"custom\":\"keep\",\"examples\":[\"payroll\"]}'",
            "ALTER COLUMN salary SET AI CONTEXT MEMBER examples '[\"payroll\"]'",
            "ALTER DATASET bundle SET DESCRIPTION 'Employees'",
            "ALTER DATASET bundle SET AI CONTEXT 'One row per employee'",
            "DECLARE PRIMARY KEY (id)",
            "DECLARE UNIQUE KEY (last_name, first_name)",
            "ALTER METRIC payroll SET EXPRESSION 'SUM(salary) * 2'",
            "ALTER METRIC payroll SET DESCRIPTION 'Gross payroll'",
        ],
    )
    .await?;
    assert!(
        builder.status().changes().len() > change_count,
        "explicit native commands record write intent"
    );
    assert!(
        builder.status().operations_count() > operation_count,
        "native writes participate in replay even if the local value matches"
    );
    command(&builder, "RENAME COLUMN salary TO pay").await?;
    assert_eq!(builder.column_id("pay"), Some(salary));
    assert_eq!(builder.semantic_state(), before_rename);
    assert_eq!(builder.metrics()[0].expression, "SUM(pay) * 2");
    let schema = builder.described_schema().await?;
    assert_eq!(
        schema
            .field_with_name("pay")?
            .metadata()
            .get("description")
            .map(String::as_str),
        Some("Annual gross pay")
    );
    command(
        &builder,
        "ALTER COLUMN pay SET AI CONTEXT MEMBER nullable 'null'",
    )
    .await?;
    assert_eq!(
        builder.semantic_state().columns[&salary]
            .ai_context
            .as_ref()
            .and_then(|value| value.get("nullable")),
        Some(&Value::Null)
    );
    command(&builder, "SHOW SEMANTIC MODEL").await?;
    builder.commit("native semantic properties").await?;
    let reopened = Bundle::open(&loc.path, None).await?;
    assert_eq!(reopened.semantic_state(), builder.semantic_state());
    assert_eq!(
        reopened.semantic_model().await?["datasets"][0]["primary_key"],
        json!(["id"])
    );

    commands(
        &builder,
        &[
            "ALTER COLUMN pay DROP DESCRIPTION",
            "ALTER COLUMN pay DROP LABEL",
            "ALTER COLUMN pay DROP AI CONTEXT",
            "ALTER DATASET bundle DROP DESCRIPTION",
            "ALTER DATASET bundle DROP AI CONTEXT",
            "ALTER SEMANTIC MODEL DROP AI CONTEXT MEMBER synonyms",
            "ALTER SEMANTIC MODEL DROP AI CONTEXT",
            "ALTER METRIC payroll DROP DESCRIPTION",
            "ALTER METRIC payroll DROP AI CONTEXT",
            "DROP PRIMARY KEY",
            "DROP UNIQUE KEY (last_name, first_name)",
        ],
    )
    .await?;
    let cleared = builder.semantic_state();
    assert!(!cleared.columns.contains_key(&salary));
    assert_eq!(cleared.dataset_description, None);
    assert_eq!(cleared.dataset_ai_context, None);
    assert_eq!(cleared.model_ai_context, None);
    assert_eq!(cleared.metrics["payroll"].description, None);
    assert_eq!(cleared.metrics["payroll"].ai_context, None);
    assert_eq!(cleared.primary_key, None);
    assert!(cleared.unique_keys.is_empty());
    commands(
        &builder,
        &["DROP METRIC payroll", "CREATE METRIC payroll AS 'COUNT(*)'"],
    )
    .await?;
    assert_eq!(builder.metrics()[0].expression, "COUNT(*)");
    assert_eq!(
        builder.semantic_state().metrics["payroll"].description,
        None
    );
    builder.commit("clear and recreate").await?;
    assert_eq!(
        Bundle::open(&loc.path, None).await?.semantic_state(),
        builder.semantic_state()
    );
    Ok(())
}

#[rstest]
#[case::memory("memory")]
#[case::local("local")]
#[tokio::test]
async fn documents_diff_noop_merge_replace_undo_and_roundtrip(
    #[case] backend: &str,
) -> Result<(), BundlebaseError> {
    let loc = location(backend)?;
    let builder = setup(&loc.path).await?;
    commands(&builder, &[
        "SET NAME 'Employees'",
        "ALTER SEMANTIC MODEL SET DESCRIPTION 'Employee model'",
        "ALTER SEMANTIC MODEL DROP DESCRIPTION",
        "ALTER COLUMN salary SET DESCRIPTION 'Annual pay'",
        "ALTER COLUMN salary SET LABEL 'Payroll'",
        "ALTER COLUMN salary SET AI CONTEXT '{\"instructions\":\"Annual\",\"custom\":\"keep\"}'",
        "ALTER DATASET bundle SET DESCRIPTION 'Employees'",
        "DECLARE PRIMARY KEY (id)",
        "DECLARE UNIQUE KEY (email)",
        "CREATE METRIC payroll AS 'SUM(salary)' DESCRIPTION 'Gross payroll'",
    ]).await?;
    builder.commit("baseline semantic model").await?;
    let original = builder.semantic_model().await?;
    assert!(document_operations(&builder, &original, false)
        .await?
        .is_empty());
    builder.apply_semantic_model(&original, false).await?;
    assert!(
        builder.status().is_empty(),
        "identical import records no change"
    );
    let mut provenance = original.clone();
    provenance["custom_extensions"][0]["data"] =
        json!("{\"version\":\"ignored external provenance\"}");
    builder.apply_semantic_model(&provenance, false).await?;
    assert!(
        builder.status().is_empty(),
        "generated provenance is ignored as mutation input"
    );
    let mut single = original.clone();
    field_mut(&mut single, "salary")?["label"] = json!("Gross compensation");
    let diff = document_operations(&builder, &single, false).await?;
    assert_eq!(diff.len(), 1);
    assert!(
        matches!(&diff[0], AnyOperation::SetColumnLabel(op) if op.label.as_deref() == Some("Gross compensation"))
    );
    builder.apply_semantic_model(&single, false).await?;
    assert_eq!(builder.status().changes().len(), 1);
    assert_eq!(builder.status().operations_count(), 1);
    builder.undo().await?;
    assert_eq!(builder.semantic_model().await?, original);

    let mut merge = original.clone();
    object_remove(field_mut(&mut merge, "salary")?, "description")?;
    field_mut(&mut merge, "salary")?["ai_context"] = json!({"instructions":"Updated"});
    object_remove(&mut merge, "metrics")?;
    object_remove(&mut merge["datasets"][0], "primary_key")?;
    object_remove(&mut merge["datasets"][0], "unique_keys")?;
    let diff = document_operations(&builder, &merge, false).await?;
    assert_eq!(
        diff.len(),
        1,
        "merge changes only the supplied differing AI member"
    );
    assert!(
        matches!(&diff[0], AnyOperation::SetAiContextMember(op) if op.member == "instructions")
    );
    builder.apply_semantic_model(&merge, false).await?;
    let id = builder.column_id("salary").ok_or("Missing salary")?;
    let merged = builder.semantic_state();
    assert_eq!(
        merged.columns[&id].description.as_deref(),
        Some("Annual pay")
    );
    assert_eq!(
        merged.columns[&id].ai_context,
        Some(json!({"instructions":"Updated","custom":"keep"}))
    );
    assert!(merged.metrics.contains_key("payroll"));
    assert!(merged.primary_key.is_some());
    assert_eq!(merged.unique_keys.len(), 1);
    builder.undo().await?;
    assert_eq!(builder.semantic_model().await?, original);

    let replace = merge;
    let diff = document_operations(&builder, &replace, true).await?;
    assert!(diff
        .iter()
        .any(|op| matches!(op, AnyOperation::ClearColumnDescription(_))));
    assert!(diff
        .iter()
        .any(|op| matches!(op, AnyOperation::DropMetric(op) if op.name == "payroll")));
    assert!(diff
        .iter()
        .any(|op| matches!(op, AnyOperation::DropKey(op) if !op.unique)));
    assert!(diff
        .iter()
        .any(|op| matches!(op, AnyOperation::DropKey(op) if op.unique)));
    assert!(diff.iter().any(|op| matches!(op, AnyOperation::SetAiContextMember(op) if op.member == "custom" && op.value.is_none())));
    builder.apply_semantic_model(&replace, true).await?;
    let replaced = builder.semantic_state();
    assert_eq!(replaced.columns[&id].description, None);
    assert_eq!(
        replaced.columns[&id].ai_context,
        Some(json!({"instructions":"Updated"}))
    );
    assert!(replaced.metrics.is_empty());
    assert_eq!(replaced.primary_key, None);
    assert!(replaced.unique_keys.is_empty());
    builder.undo().await?;
    assert_eq!(builder.semantic_model().await?, original);

    let temp = tempfile::tempdir()?;
    for ext in ["json", "yaml"] {
        let path = temp.path().join(format!("semantic.{ext}"));
        let path = path.to_string_lossy();
        export_semantic_model(builder.as_ref(), &path).await?;
        builder.import_semantic_model(&path, false).await?;
        assert!(
            builder.status().is_empty(),
            "export/import is a semantic no-op for {ext}"
        );
    }
    builder.apply_semantic_model(&single, false).await?;
    builder.commit("apply one semantic property").await?;
    let reopened = Bundle::open(&loc.path, None).await?;
    assert_eq!(reopened.semantic_state(), builder.semantic_state());
    Ok(())
}

#[rstest]
#[case::memory("memory")]
#[case::local("local")]
#[tokio::test]
async fn rejected_commands_and_documents_are_atomic(
    #[case] backend: &str,
) -> Result<(), BundlebaseError> {
    let loc = location(backend)?;
    let builder = setup(&loc.path).await?;
    command(
        &builder,
        "CREATE METRIC payroll AS 'SUM(salary)' DESCRIPTION 'Keep'",
    )
    .await?;
    builder.commit("baseline").await?;
    let original = builder.semantic_model().await?;
    let state = builder.semantic_state();
    for sql in [
        "ALTER COLUMN missing SET LABEL 'Unknown'",
        "ALTER METRIC missing SET DESCRIPTION 'Unknown'",
        "ALTER METRIC missing SET EXPRESSION 'COUNT(*)'",
        "ALTER METRIC payroll SET EXPRESSION 'SUM(missing)'",
        "ALTER SEMANTIC MODEL SET AI CONTEXT 'null'",
        "ALTER COLUMN salary SET AI CONTEXT '{\"instructions\":null}'",
        "ALTER METRIC payroll SET AI CONTEXT MEMBER synonyms '[1]'",
        "DECLARE PRIMARY KEY (id, id)",
        "DECLARE UNIQUE KEY (missing)",
    ] {
        assert!(command(&builder, sql).await.is_err(), "must reject {sql}");
        assert_eq!(
            builder.semantic_state(),
            state,
            "rejected command leaves semantics intact: {sql}"
        );
        assert!(builder.status().is_empty());
    }
    let mut invalid = original.clone();
    invalid["description"] = json!("Must roll back");
    field_mut(&mut invalid, "salary")?["label"] = json!("Must roll back");
    invalid["metrics"][0]["expression"]["dialects"][0]["expression"] = json!("SUM(missing)");
    assert!(builder.apply_semantic_model(&invalid, false).await.is_err());
    assert_eq!(builder.semantic_model().await?, original);
    assert!(builder.status().is_empty());

    for invalid_kind in ["unknown_column", "wrong_datatype", "duplicate_column"] {
        let mut invalid = original.clone();
        invalid["description"] = json!("Must roll back");
        match invalid_kind {
            "unknown_column" => {
                field_mut(&mut invalid, "salary")?["name"] = json!("missing_column")
            }
            "wrong_datatype" => {
                let field = field_mut(&mut invalid, "salary")?;
                field["datatype"] = if field["datatype"] == "String" {
                    json!("Integer")
                } else {
                    json!("String")
                };
            }
            _ => {
                let duplicate = field_mut(&mut invalid, "salary")?.clone();
                invalid["datasets"][0]["fields"]
                    .as_array_mut()
                    .ok_or("Missing fields")?
                    .push(duplicate);
            }
        }
        assert!(
            builder.apply_semantic_model(&invalid, false).await.is_err(),
            "must reject {invalid_kind}"
        );
        assert_eq!(builder.semantic_model().await?, original);
        assert!(builder.status().is_empty());
    }

    // A valid preceding operation must also be rolled back when a later operation fails.
    let failure = builder
        .do_change("atomic semantic validation", |b| {
            Box::pin(async move {
                use bundlebase::bundle::operation::{SetColumnLabelOp, SetMetricDescriptionOp};
                let id = b.column_id("salary").ok_or("Missing salary")?;
                b.apply_operation(SetColumnLabelOp::setup(id, Some("Must roll back")).into())
                    .await?;
                b.apply_operation(SetMetricDescriptionOp::setup("missing", Some("Invalid")).into())
                    .await?;
                Ok(())
            })
        })
        .await;
    assert!(failure.is_err());
    assert_eq!(builder.semantic_model().await?, original);
    assert!(builder.status().is_empty());
    Ok(())
}

#[rstest]
#[case::memory("memory")]
#[case::local("local")]
#[tokio::test]
async fn dropped_columns_hide_metadata_keys_but_preserve_dangling_metrics(
    #[case] backend: &str,
) -> Result<(), BundlebaseError> {
    let loc = location(backend)?;
    let builder = setup(&loc.path).await?;
    commands(
        &builder,
        &[
            "ALTER COLUMN salary SET DESCRIPTION 'Annual pay'",
            "ALTER COLUMN salary SET LABEL 'Pay'",
            "DECLARE PRIMARY KEY (salary)",
            "DECLARE UNIQUE KEY (salary, id)",
            "CREATE METRIC payroll AS 'SUM(salary)' DESCRIPTION 'Still discoverable'",
            "DROP COLUMN salary",
        ],
    )
    .await?;
    builder.commit("drop metric dependency").await?;
    let reopened = Bundle::open(&loc.path, None).await?;
    let state = reopened.semantic_state();
    assert!(state.columns.is_empty());
    assert_eq!(state.primary_key, None);
    assert!(state.unique_keys.is_empty());
    assert!(state.metrics.contains_key("payroll"));
    assert!(reopened.schema().await?.field_with_name("salary").is_err());
    assert_eq!(
        reopened.metrics()[0].description.as_deref(),
        Some("Still discoverable")
    );
    let error = reopened
        .semantic_model()
        .await
        .expect_err("strict export rejects dangling references");
    assert!(error.to_string().contains("column"));
    assert!(reopened.compute_metric("payroll", &[], None).await.is_err());
    Ok(())
}

async fn meta_dir(path: &str) -> Result<Box<dyn IOReadWriteDir>, BundlebaseError> {
    bundlebase_io::writable_dir_from_str(path, test_config())
        .await?
        .writable_subdir("_bundlebase")
}

async fn manifest_names(path: &str) -> Result<Vec<String>, BundlebaseError> {
    let dir = meta_dir(path).await?;
    let prefix = dir.url().to_string();
    let mut names = dir
        .list_files()
        .await?
        .into_iter()
        .filter(|f| {
            let rel = f.url.as_str()[prefix.len()..].trim_start_matches('/');
            !rel.contains('/') && f.filename() != Some(INIT_FILENAME)
        })
        .filter_map(|f| f.filename().map(str::to_owned))
        .collect::<Vec<_>>();
    names.sort();
    Ok(names)
}

async fn take_new_manifest(
    path: &str,
    baseline: &[String],
) -> Result<(String, bytes::Bytes), BundlebaseError> {
    let name = manifest_names(path)
        .await?
        .into_iter()
        .find(|n| !baseline.contains(n))
        .ok_or("Missing new manifest")?;
    let file = meta_dir(path).await?.writable_file(&name)?;
    let bytes = file.read_bytes().await?.ok_or("Missing manifest bytes")?;
    file.delete().await?;
    Ok((name, bytes))
}

#[rstest]
#[case::memory("memory")]
#[case::local("local")]
#[tokio::test]
async fn tied_manifests_force_both_property_orders(
    #[case] backend: &str,
) -> Result<(), BundlebaseError> {
    let loc = location(backend)?;
    let builder = setup(&loc.path).await?;
    commands(
        &builder,
        &[
            "ALTER COLUMN salary SET DESCRIPTION 'Base'",
            "CREATE METRIC payroll AS 'SUM(salary)' DESCRIPTION 'Base metric'",
            "CREATE METRIC lifecycle AS 'COUNT(*)' DESCRIPTION 'Old lifetime'",
            "CREATE METRIC redefined AS 'COUNT(*)'",
            "ALTER METRIC lifecycle SET AI CONTEXT 'Old context'",
            "ALTER SEMANTIC MODEL SET AI CONTEXT '{\"custom\":\"keep\"}'",
        ],
    )
    .await?;
    builder.commit("baseline").await?;
    let baseline = manifest_names(&loc.path).await?;
    let a = BundleBuilder::extend(Bundle::open(&loc.path, None).await?, None).await?;
    let b = BundleBuilder::extend(Bundle::open(&loc.path, None).await?, None).await?;
    commands(
        &a,
        &[
            "ALTER COLUMN salary SET DESCRIPTION 'Alpha'",
            "CREATE METRIC redefined AS 'COUNT(*) * 2'",
            "ALTER METRIC payroll SET EXPRESSION 'SUM(salary) * 2'",
            "ALTER SEMANTIC MODEL SET AI CONTEXT MEMBER instructions 'Alpha context'",
            "DROP METRIC lifecycle",
        ],
    )
    .await?;
    commands(
        &b,
        &[
            "ALTER COLUMN salary DROP DESCRIPTION",
            "DROP METRIC redefined",
            "ALTER METRIC payroll SET DESCRIPTION 'Beta metric'",
            "ALTER SEMANTIC MODEL SET AI CONTEXT MEMBER examples '[\"Beta context\"]'",
            "DROP METRIC lifecycle",
            "CREATE METRIC lifecycle AS 'COUNT(*) * 3'",
        ],
    )
    .await?;
    a.commit("alpha writer").await?;
    let (a_name, a_bytes) = take_new_manifest(&loc.path, &baseline).await?;
    b.commit("beta writer").await?;
    let (b_name, b_bytes) = take_new_manifest(&loc.path, &baseline).await?;
    assert_eq!(
        &a_name[..5],
        &b_name[..5],
        "writers commit against the same directory listing"
    );
    let mut ordered_names = [a_name, b_name];
    ordered_names.sort();
    assert_ne!(ordered_names[0], ordered_names[1]);

    // Place actual committed files into controlled filename slots to force both
    // semantic tie orders. Replay is defined by filename order, not file creation order.
    for alpha_last in [false, true] {
        let (low, high) = if alpha_last {
            (b_bytes.clone(), a_bytes.clone())
        } else {
            (a_bytes.clone(), b_bytes.clone())
        };
        let dir = meta_dir(&loc.path).await?;
        dir.writable_file(&ordered_names[1])?.write(high).await?;
        dir.writable_file(&ordered_names[0])?.write(low).await?;
        let first = Bundle::open(&loc.path, None).await?;
        let second = Bundle::open(&loc.path, None).await?;
        let state = first.semantic_state();
        assert_eq!(
            state,
            second.semantic_state(),
            "independent readers replay identically"
        );
        assert_eq!(
            state.metrics.contains_key("redefined"),
            alpha_last,
            "explicit definition versus removal follows replay order"
        );
        let salary = first.column_id("salary").ok_or("Missing salary")?;
        assert_eq!(
            state
                .columns
                .get(&salary)
                .and_then(|column| column.description.as_deref()),
            if alpha_last { Some("Alpha") } else { None }
        );
        let metric = first
            .metrics()
            .into_iter()
            .find(|m| m.name == "payroll")
            .ok_or("Missing payroll")?;
        assert_eq!(
            metric.expression, "SUM(salary) * 2",
            "independent expression edit survives both orders"
        );
        assert_eq!(
            metric.description.as_deref(),
            Some("Beta metric"),
            "independent description survives both orders"
        );
        assert_eq!(
            state.model_ai_context,
            Some(
                json!({"custom":"keep","instructions":"Alpha context","examples":["Beta context"]})
            )
        );
        if alpha_last {
            assert!(!state.metrics.contains_key("lifecycle"));
        } else {
            let metric = state
                .metrics
                .get("lifecycle")
                .ok_or("Missing recreated metric")?;
            assert_eq!(
                metric.description, None,
                "drop and recreation clear prior lifetime properties"
            );
            assert_eq!(metric.ai_context, None);
        }
        dir.writable_file(&ordered_names[0])?.delete().await?;
        dir.writable_file(&ordered_names[1])?.delete().await?;
    }
    Ok(())
}

#[tokio::test]
async fn documents_preserve_pending_work_and_extension_member_values() -> Result<(), BundlebaseError>
{
    let loc = location("memory")?;
    let builder = setup(&loc.path).await?;
    command(&builder, "CREATE METRIC payroll AS 'SUM(salary)'").await?;
    builder.commit("baseline").await?;
    command(&builder, "ALTER COLUMN salary SET LABEL 'Pending label'").await?;
    let original = builder.semantic_model().await?;
    let changes = builder.status().changes().len();
    let mut bad = original.clone();
    bad["description"] = json!("Do not keep");
    bad["metrics"][0]["expression"]["dialects"][0]["expression"] = json!("SUM(missing)");
    assert!(builder.apply_semantic_model(&bad, true).await.is_err());
    assert_eq!(builder.status().changes().len(), changes);
    assert_eq!(builder.semantic_model().await?, original);
    let mut first = original.clone();
    field_mut(&mut first, "salary")?["ai_context"] = json!({"":1,"nullable":null});
    builder.apply_semantic_model(&first, false).await?;
    let mut next = builder.semantic_model().await?;
    field_mut(&mut next, "salary")?["ai_context"] = json!({"":2,"nullable":null});
    builder.apply_semantic_model(&next, false).await?;
    builder.commit("extension members").await?;
    builder.rename_column("salary", "Annual Pay").await?;
    let snapshot = builder.semantic_model().await?;
    assert!(
        document_operations(&builder, &snapshot, false)
            .await?
            .is_empty(),
        "quoted renamed expressions round-trip without edits"
    );
    builder.commit("rename").await?;
    let reopened = Bundle::open(&loc.path, None).await?;
    let id = reopened
        .column_id("Annual Pay")
        .ok_or("Annual Pay missing")?;
    assert_eq!(
        reopened.semantic_state().columns[&id].ai_context,
        Some(json!({"":2,"nullable":null}))
    );
    Ok(())
}

#[tokio::test]
async fn metadata_on_an_opened_view_has_its_own_history() -> Result<(), BundlebaseError> {
    let loc = location("memory")?;
    let parent = setup(&loc.path).await?;
    command(
        &parent,
        "ALTER COLUMN salary SET DESCRIPTION 'Inherited pay'",
    )
    .await?;
    parent.commit("base").await?;
    parent
        .create_view("employees", "SELECT * FROM bundle")
        .await?;
    parent.commit("view").await?;
    let view = parent.view("employees").await?;
    let builder = BundleBuilder::extend(view, None).await?;
    command(&builder, "ALTER COLUMN salary SET LABEL 'View pay'").await?;
    builder.commit("view metadata").await?;
    let reopened = Bundle::open(&loc.path, None).await?;
    let child = reopened.view("employees").await?;
    let id = child.column_id("salary").ok_or("salary missing")?;
    assert_eq!(
        child.semantic_state().columns[&id].description.as_deref(),
        Some("Inherited pay")
    );
    assert_eq!(
        child.semantic_state().columns[&id].label.as_deref(),
        Some("View pay")
    );
    assert!(reopened
        .semantic_state()
        .columns
        .values()
        .all(|column| column.label.is_none()));
    assert_eq!(
        child.semantic_model().await?["datasets"][0]["source"],
        "bundle"
    );
    Ok(())
}
