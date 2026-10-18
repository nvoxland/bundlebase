//! Checkpoints must preserve the public full-replay result on both storage backends.

use bundlebase::bundle::operation::SetDescriptionOp;
use bundlebase::bundle::{BundleChange, BundleCommit, INIT_FILENAME, META_DIR};
use bundlebase::bundle_config::{PassedBundleConfig, Scope};
use bundlebase::test_utils::{random_memory_dir, test_datafile};
use bundlebase::{Bundle, BundleBuilder, BundleConfig, BundleFacade};
use bundlebase_command::BundleBuilderExt;
use bundlebase_common::checkpoint_config::{ENABLED_CFG, EVERY_CFG, GRACE_CFG, MAX_AGE_CFG};
use bundlebase_common::file_info::FileInfo;
use bundlebase_common::BundlebaseError;
use bundlebase_io::{read_yaml, IOReadWriteDir};
use bytes::Bytes;
use futures::StreamExt;
use std::sync::Arc;

mod common;

#[tokio::test]
async fn automatic_maintenance_failure_does_not_fail_a_durable_commit(
) -> Result<(), BundlebaseError> {
    let (_temp, dir) = storage(false).await?;
    let builder = BundleBuilder::create(
        dir.url().as_str(),
        Some(config(&[(EVERY_CFG.key, "invalid")])?),
    )
    .await?;
    builder.set_name("durable despite cache failure").await?;
    builder.commit("published successfully").await?;
    assert!(builder.status().is_empty());
    assert!(
        builder.checkpoint().await.is_err(),
        "explicit maintenance reports the invalid policy"
    );
    let reopened = Bundle::open(dir.url().as_str(), None).await?;
    assert_eq!(
        reopened.name().as_deref(),
        Some("durable despite cache failure")
    );
    assert_eq!(reopened.history().len(), 1);
    assert_eq!(reopened.version(), builder.version());
    Ok(())
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn maintenance_commands_preserve_pending_changes(
    #[case] local: bool,
) -> Result<(), BundlebaseError> {
    use bundlebase_command::{parser::parse_command, BundleFacadeCommandExt};

    let (_temp, dir) = storage(local).await?;
    let builder = BundleBuilder::create(dir.url().as_str(), Some(config(&[])?)).await?;
    assert!(builder.checkpoint().await.is_err());
    builder.set_name("durable").await?;
    builder.commit("initial").await?;
    let version = builder.version();
    builder.set_description("pending").await?;
    let pending = builder.status().operations();
    builder
        .execute_command(parse_command("CHECKPOINT")?)
        .await?;
    builder
        .execute_command(parse_command("CHECKPOINT PRUNE")?)
        .await?;
    assert_eq!(builder.status().operations(), pending);
    assert_eq!(builder.history().len(), 1);
    assert_eq!(checkpoints(dir.as_ref()).await?.len(), 1);
    let reopened = Bundle::open(dir.url().as_str(), Some(config(&[])?)).await?;
    assert_eq!(reopened.version(), version);
    assert_eq!(reopened.description(), None);
    Ok(())
}

fn config(values: &[(&str, &str)]) -> Result<PassedBundleConfig, BundlebaseError> {
    let mut result = PassedBundleConfig::new();
    let scope = Scope::try_from("checkpoint")?;
    result.set(&scope, EVERY_CFG.key, "0");
    for (key, value) in values {
        result.set(&scope, key, value);
    }
    Ok(result)
}

async fn storage(
    local: bool,
) -> Result<(tempfile::TempDir, Arc<dyn IOReadWriteDir>), BundlebaseError> {
    common::init_catalog();
    let temp = tempfile::tempdir()?;
    let dir = if local {
        let url = url::Url::from_directory_path(temp.path().join("bundle")).expect("directory URL");
        bundlebase_io::writable_dir_from_url(&url, Arc::new(BundleConfig::new(None)?)).await?
    } else {
        random_memory_dir()
    };
    Ok((temp, dir))
}

async fn manifests(dir: &dyn IOReadWriteDir) -> Result<Vec<FileInfo>, BundlebaseError> {
    let meta = dir.writable_subdir(META_DIR)?;
    let mut files: Vec<_> = meta
        .list_files()
        .await?
        .into_iter()
        .filter(|file| {
            file.filename() != Some(INIT_FILENAME) && !file.url.path().contains("/checkpoints/")
        })
        .collect();
    files.sort_by(|a, b| a.filename().cmp(&b.filename()));
    Ok(files)
}

async fn checkpoints(dir: &dyn IOReadWriteDir) -> Result<Vec<FileInfo>, BundlebaseError> {
    let cp = dir
        .writable_subdir(META_DIR)?
        .writable_subdir("checkpoints")?;
    let mut files = cp.list_files().await?;
    files.sort_by(|a, b| a.filename().cmp(&b.filename()));
    Ok(files)
}

async fn checkpoint_bytes(
    dir: &dyn IOReadWriteDir,
    file: &FileInfo,
) -> Result<Bytes, BundlebaseError> {
    dir.writable_subdir(META_DIR)?
        .writable_subdir("checkpoints")?
        .file(file.filename().expect("checkpoint filename"))?
        .read_bytes()
        .await?
        .ok_or_else(|| "checkpoint missing".into())
}

async fn scan(bundle: &dyn BundleFacade) -> Result<Vec<Vec<String>>, BundlebaseError> {
    let sql = if bundle.schema().await?.field_with_name("id").is_ok() {
        "SELECT * FROM bundle ORDER BY id"
    } else {
        "SELECT * FROM bundle"
    };
    let mut stream = bundle.query(sql, vec![], None).await?;
    let mut rows = Vec::new();
    while let Some(batch) = stream.next().await {
        let batch = batch?;
        for row in 0..batch.num_rows() {
            rows.push(
                (0..batch.num_columns())
                    .map(|column| {
                        arrow::util::display::array_value_to_string(
                            batch.column(column).as_ref(),
                            row,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            );
        }
    }
    Ok(rows)
}

async fn assert_same(
    left: &dyn BundleFacade,
    right: &dyn BundleFacade,
) -> Result<(), BundlebaseError> {
    assert_eq!(left.version(), right.version());
    assert_eq!(left.from(), right.from());
    assert_eq!(left.name(), right.name());
    assert_eq!(left.description(), right.description());
    assert_eq!(left.operations(), right.operations());
    let lh = left.history();
    let rh = right.history();
    assert_eq!(serde_json::to_value(&lh)?, serde_json::to_value(&rh)?);
    assert_eq!(
        lh.iter().map(|c| (&c.url, &c.data_dir)).collect::<Vec<_>>(),
        rh.iter().map(|c| (&c.url, &c.data_dir)).collect::<Vec<_>>()
    );
    assert_eq!(left.schema().await?, right.schema().await?);
    assert_eq!(left.semantic_model().await?, right.semantic_model().await?);
    assert_eq!(left.num_rows().await?, right.num_rows().await?);
    assert_eq!(scan(left).await?, scan(right).await?);
    Ok(())
}

async fn set_timestamp(
    dir: &dyn IOReadWriteDir,
    file: &FileInfo,
    value: &str,
) -> Result<(), BundlebaseError> {
    let meta = dir.writable_subdir(META_DIR)?;
    let target = meta.file(file.filename().expect("manifest filename"))?;
    let mut yaml: serde_yaml_ng::Value = read_yaml(target.as_ref()).await?.expect("manifest");
    yaml["timestamp"] = serde_yaml_ng::Value::String(value.to_string());
    meta.writable_file(file.filename().expect("manifest filename"))?
        .write(Bytes::from(serde_yaml_ng::to_string(&yaml)?))
        .await
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn checkpoint_and_tail_equal_full_replay(#[case] local: bool) -> Result<(), BundlebaseError> {
    let (_temp, dir) = storage(local).await?;
    let builder = BundleBuilder::create(dir.url().as_str(), Some(config(&[])?)).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    builder
        .define_metric("total_salary", "SUM(salary)", Some("Payroll"))
        .await?;
    builder.describe_column("salary", "Annual pay").await?;
    builder.commit("attach and metadata").await?;
    for salary in [100, 200] {
        builder
            .update(&format!("SET salary = {salary} WHERE id = 1"))
            .await?;
        builder.commit("update same cell").await?;
    }
    builder.delete("id = 2").await?;
    builder.commit("delete").await?;
    builder.checkpoint().await?;
    builder.set_description("uncovered tail").await?;
    builder.commit("tail").await?;
    let cached = Bundle::open(dir.url().as_str(), Some(config(&[])?)).await?;
    let full = Bundle::open(
        dir.url().as_str(),
        Some(config(&[(ENABLED_CFG.key, "false")])?),
    )
    .await?;
    assert_same(cached.as_ref(), full.as_ref()).await?;
    assert_eq!(cached.history().len(), 5);
    Ok(())
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn valid_checkpoint_avoids_reading_covered_manifests(
    #[case] local: bool,
) -> Result<(), BundlebaseError> {
    let (_temp, dir) = storage(local).await?;
    let builder = BundleBuilder::create(dir.url().as_str(), Some(config(&[])?)).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    builder.commit("covered").await?;
    builder.checkpoint().await?;
    let first = manifests(dir.as_ref()).await?.remove(0);
    dir.writable_subdir(META_DIR)?
        .writable_file(first.filename().expect("manifest"))?
        .write(Bytes::from_static(b"[invalid yaml"))
        .await?;
    let cached = Bundle::open(dir.url().as_str(), Some(config(&[])?)).await?;
    assert_eq!(cached.num_rows().await?, 1000);
    assert!(Bundle::open(
        dir.url().as_str(),
        Some(config(&[(ENABLED_CFG.key, "false")])?)
    )
    .await
    .is_err());
    Ok(())
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn late_same_number_manifest_invalidates_checkpoint(
    #[case] local: bool,
) -> Result<(), BundlebaseError> {
    let (_temp, dir) = storage(local).await?;
    let builder = BundleBuilder::create(dir.url().as_str(), Some(config(&[])?)).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    builder.commit("one").await?;
    builder.checkpoint().await?;
    builder.set_name("two").await?;
    builder.commit("two").await?;
    builder.checkpoint().await?;
    let mut change = BundleChange::new("late writer");
    change
        .operations
        .push(SetDescriptionOp::setup("straggler applied").into());
    let late = BundleCommit {
        url: None,
        data_dir: None,
        author: "late writer".into(),
        message: "late".into(),
        timestamp: "2020-01-01T00:00:00Z".into(),
        changes: vec![change],
    };
    dir.writable_subdir(META_DIR)?
        .writable_file("00001ffffffffffff.yaml")?
        .write(Bytes::from(serde_yaml_ng::to_string(&late)?))
        .await?;
    let cached = Bundle::open(dir.url().as_str(), Some(config(&[])?)).await?;
    let full = Bundle::open(
        dir.url().as_str(),
        Some(config(&[(ENABLED_CFG.key, "false")])?),
    )
    .await?;
    assert_same(cached.as_ref(), full.as_ref()).await?;
    assert_eq!(cached.description().as_deref(), Some("straggler applied"));
    let canonical_version = full.version();

    // The checkpoint ending before the insertion remains usable, even though
    // the newer checkpoint's covered set no longer matches. Prevent a full
    // replay from hiding a failure to use that earlier checkpoint.
    let first = manifests(dir.as_ref()).await?.remove(0);
    let first_file = dir
        .writable_subdir(META_DIR)?
        .writable_file(first.filename().expect("manifest"))?;
    let original = first_file.read_bytes().await?.expect("first manifest");
    first_file
        .write(Bytes::from_static(b"[invalid yaml"))
        .await?;
    let earlier_checkpoint = Bundle::open(dir.url().as_str(), Some(config(&[])?)).await?;
    assert_same(earlier_checkpoint.as_ref(), full.as_ref()).await?;
    first_file.write(original).await?;

    cached.extend(None).await?.checkpoint().await?;
    assert_eq!(checkpoints(dir.as_ref()).await?.len(), 2);
    let refreshed = Bundle::open(dir.url().as_str(), Some(config(&[])?)).await?;
    assert_eq!(refreshed.version(), canonical_version);
    let continued = refreshed.extend(None).await?;
    continued
        .set_description("commit after the insertion")
        .await?;
    continued.commit("continue canonical history").await?;
    let full_tail = Bundle::open(
        dir.url().as_str(),
        Some(config(&[(ENABLED_CFG.key, "false")])?),
    )
    .await?;
    let cached_tail = Bundle::open(dir.url().as_str(), Some(config(&[])?)).await?;
    assert_eq!(continued.version(), full_tail.version());
    assert_same(cached_tail.as_ref(), full_tail.as_ref()).await?;
    Ok(())
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn checkpoint_lifecycle_never_changes_commit_set_version(
    #[case] local: bool,
) -> Result<(), BundlebaseError> {
    let (_temp, dir) = storage(local).await?;
    let builder = BundleBuilder::create(dir.url().as_str(), Some(config(&[])?)).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    builder.commit("data").await?;
    for salary in [100, 200] {
        builder
            .update(&format!("SET salary = {salary} WHERE id = 1"))
            .await?;
        builder.commit("edit").await?;
    }
    let original = Bundle::open(
        dir.url().as_str(),
        Some(config(&[(ENABLED_CFG.key, "false")])?),
    )
    .await?;
    builder.checkpoint().await?;
    let cached = Bundle::open(dir.url().as_str(), Some(config(&[])?)).await?;
    assert_same(cached.as_ref(), original.as_ref()).await?;

    // Extend a checkpoint-loaded bundle, rather than the original live writer.
    let continued = cached.extend(None).await?;
    continued.delete("id = 2").await?;
    continued.commit("tail after checkpoint").await?;
    let canonical = Bundle::open(
        dir.url().as_str(),
        Some(config(&[(ENABLED_CFG.key, "false")])?),
    )
    .await?;
    assert_eq!(continued.version(), canonical.version());
    continued.checkpoint().await?;
    assert_eq!(checkpoints(dir.as_ref()).await?.len(), 2);
    continued.checkpoint_prune().await?;
    assert_eq!(checkpoints(dir.as_ref()).await?.len(), 1);
    let pruned = Bundle::open(dir.url().as_str(), Some(config(&[])?)).await?;
    assert_same(pruned.as_ref(), canonical.as_ref()).await?;

    let directory = dir
        .writable_subdir(META_DIR)?
        .writable_subdir("checkpoints")?;
    for file in checkpoints(dir.as_ref()).await? {
        directory
            .writable_file(file.filename().expect("checkpoint"))?
            .delete()
            .await?;
    }
    let without_cache = Bundle::open(dir.url().as_str(), Some(config(&[])?)).await?;
    assert_same(without_cache.as_ref(), canonical.as_ref()).await?;
    without_cache.extend(None).await?.checkpoint().await?;
    let replaced = Bundle::open(dir.url().as_str(), Some(config(&[])?)).await?;
    assert_same(replaced.as_ref(), canonical.as_ref()).await?;
    assert_eq!(manifests(dir.as_ref()).await?.len(), 4);
    Ok(())
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn checkpoint_is_deterministic_and_corruption_falls_back(
    #[case] local: bool,
) -> Result<(), BundlebaseError> {
    let (_temp, dir) = storage(local).await?;
    let builder = BundleBuilder::create(dir.url().as_str(), Some(config(&[])?)).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    builder.commit("one").await?;
    builder.checkpoint().await?;
    let before = checkpoints(dir.as_ref()).await?;
    let bytes = checkpoint_bytes(dir.as_ref(), &before[0]).await?;
    Bundle::open(dir.url().as_str(), Some(config(&[])?))
        .await?
        .extend(None)
        .await?
        .checkpoint()
        .await?;
    let after = checkpoints(dir.as_ref()).await?;
    assert_eq!(before[0].url, after[0].url);
    assert_eq!(bytes, checkpoint_bytes(dir.as_ref(), &after[0]).await?);
    // Valid YAML with an incorrect body checksum must also fall back safely.
    let mut yaml: serde_yaml_ng::Value = serde_yaml_ng::from_slice(&bytes)?;
    yaml["checkpoint"]["version"] = serde_yaml_ng::Value::String("00".repeat(32));
    dir.writable_subdir(META_DIR)?
        .writable_subdir("checkpoints")?
        .writable_file(after[0].filename().expect("checkpoint"))?
        .write(Bytes::from(serde_yaml_ng::to_string(&yaml)?))
        .await?;
    let cached = Bundle::open(dir.url().as_str(), Some(config(&[])?)).await?;
    let full = Bundle::open(
        dir.url().as_str(),
        Some(config(&[(ENABLED_CFG.key, "false")])?),
    )
    .await?;
    assert_same(cached.as_ref(), full.as_ref()).await?;
    Ok(())
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn changed_parent_invalidates_child_checkpoint(
    #[case] local: bool,
) -> Result<(), BundlebaseError> {
    let (_temp, dir) = storage(local).await?;
    let parent = BundleBuilder::create(dir.url().as_str(), Some(config(&[])?)).await?;
    parent
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    parent.commit("parent").await?;
    let child_dir = dir.writable_subdir("child")?;
    let child = Bundle::open(dir.url().as_str(), Some(config(&[])?))
        .await?
        .extend(Some(child_dir.url().as_str()))
        .await?;
    child.set_description("child metadata").await?;
    child.commit("child").await?;
    child.checkpoint().await?;
    let old = Bundle::open(child_dir.url().as_str(), Some(config(&[])?))
        .await?
        .version();
    parent.update("SET salary = 321 WHERE id = 1").await?;
    parent.commit("parent changed").await?;
    let cached = Bundle::open(child_dir.url().as_str(), Some(config(&[])?)).await?;
    let full = Bundle::open(
        child_dir.url().as_str(),
        Some(config(&[(ENABLED_CFG.key, "false")])?),
    )
    .await?;
    assert_ne!(cached.version(), old);
    assert_same(cached.as_ref(), full.as_ref()).await?;
    Ok(())
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn automatic_policy_and_manual_pruning(#[case] local: bool) -> Result<(), BundlebaseError> {
    let (_temp, dir) = storage(local).await?;
    let builder = BundleBuilder::create(
        dir.url().as_str(),
        Some(config(&[(EVERY_CFG.key, "3"), (GRACE_CFG.key, "0s")])?),
    )
    .await?;
    for number in 1..=9 {
        builder.set_name(&format!("commit {number}")).await?;
        builder.commit("policy").await?;
        let expected = (number / 3).min(2) as usize;
        assert_eq!(
            checkpoints(dir.as_ref()).await?.len(),
            expected,
            "after commit {number}"
        );
    }
    builder.checkpoint_prune().await?;
    assert_eq!(checkpoints(dir.as_ref()).await?.len(), 1);
    assert_eq!(manifests(dir.as_ref()).await?.len(), 9);
    assert_eq!(
        Bundle::open(dir.url().as_str(), None)
            .await?
            .name()
            .as_deref(),
        Some("commit 9")
    );
    Ok(())
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn age_trigger_respects_a_contiguous_grace_prefix(
    #[case] local: bool,
) -> Result<(), BundlebaseError> {
    let (_temp, dir) = storage(local).await?;
    let builder = BundleBuilder::create(dir.url().as_str(), Some(config(&[])?)).await?;
    for name in ["old", "recent", "old again"] {
        builder.set_name(name).await?;
        builder.commit(name).await?;
    }
    let files = manifests(dir.as_ref()).await?;
    set_timestamp(dir.as_ref(), &files[0], "2020-01-01T00:00:00Z").await?;
    set_timestamp(dir.as_ref(), &files[1], "2999-01-01T00:00:00Z").await?;
    set_timestamp(dir.as_ref(), &files[2], "2020-01-01T00:00:00Z").await?;
    let automatic = Bundle::open(
        dir.url().as_str(),
        Some(config(&[
            (EVERY_CFG.key, "100"),
            (MAX_AGE_CFG.key, "1d"),
            (GRACE_CFG.key, "1h"),
        ])?),
    )
    .await?
    .extend(None)
    .await?;
    automatic.set_description("trigger next commit").await?;
    automatic.commit("trigger age policy").await?;
    let cp = checkpoints(dir.as_ref()).await?;
    assert_eq!(cp.len(), 1);
    let body: serde_yaml_ng::Value =
        serde_yaml_ng::from_slice(&checkpoint_bytes(dir.as_ref(), &cp[0]).await?)?;
    assert_eq!(
        body["checkpoint"]["covers"]
            .as_sequence()
            .expect("covers")
            .len(),
        1
    );
    let cached = Bundle::open(dir.url().as_str(), Some(config(&[])?)).await?;
    let full = Bundle::open(
        dir.url().as_str(),
        Some(config(&[(ENABLED_CFG.key, "false")])?),
    )
    .await?;
    assert_same(cached.as_ref(), full.as_ref()).await?;
    Ok(())
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn checkpointed_views_tar_and_export_empty(
    #[case] local: bool,
) -> Result<(), BundlebaseError> {
    let (temp, dir) = storage(local).await?;
    let builder = BundleBuilder::create(dir.url().as_str(), Some(config(&[])?)).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    builder.commit("data").await?;
    let view = builder
        .create_view("first_ten", "SELECT * FROM bundle WHERE id <= 10")
        .await?;
    builder.commit("view registered").await?;
    view.checkpoint().await?;
    let cached_view = Bundle::open(view.url().as_str(), Some(config(&[])?)).await?;
    let full_view = Bundle::open(
        view.url().as_str(),
        Some(config(&[(ENABLED_CFG.key, "false")])?),
    )
    .await?;
    assert_same(cached_view.as_ref(), full_view.as_ref()).await?;
    assert_eq!(cached_view.num_rows().await?, 10);
    builder.checkpoint().await?;
    let archive = temp.path().join("checkpoint.tar");
    builder
        .export_tar(archive.to_str().expect("archive path"), false)
        .await?;
    let tar_url = format!(
        "tar+{}",
        url::Url::from_file_path(&archive).expect("archive URL")
    );
    let archived = Bundle::open(&tar_url, Some(config(&[])?)).await?;
    assert_eq!(archived.num_rows().await?, 1000);
    let empty_path = temp.path().join("empty");
    builder
        .export_empty(empty_path.to_str().expect("empty path"))
        .await?;
    let empty = Bundle::open(empty_path.to_str().expect("empty path"), None).await?;
    assert_eq!(empty.num_rows().await?, 0);
    Ok(())
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn stale_writer_checkpoints_the_complete_durable_prefix(
    #[case] local: bool,
) -> Result<(), BundlebaseError> {
    let (_temp, dir) = storage(local).await?;
    let initial = BundleBuilder::create(dir.url().as_str(), Some(config(&[])?)).await?;
    initial
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    initial.commit("initial").await?;
    let writer = Bundle::open(dir.url().as_str(), Some(config(&[])?))
        .await?
        .extend(None)
        .await?;
    let other = Bundle::open(dir.url().as_str(), Some(config(&[])?))
        .await?
        .extend(None)
        .await?;
    other
        .set_description("other writer's durable change")
        .await?;
    other.commit("other writer").await?;
    writer.set_name("stale writer").await?;
    writer.commit("stale writer").await?;
    assert_eq!(
        writer.description(),
        None,
        "writer deliberately has stale state"
    );
    writer.checkpoint().await?;
    let files = checkpoints(dir.as_ref()).await?;
    let body: serde_yaml_ng::Value =
        serde_yaml_ng::from_slice(&checkpoint_bytes(dir.as_ref(), &files[0]).await?)?;
    assert_eq!(
        body["checkpoint"]["covers"]
            .as_sequence()
            .expect("covers")
            .len(),
        3
    );
    let cached = Bundle::open(dir.url().as_str(), Some(config(&[])?)).await?;
    let full = Bundle::open(
        dir.url().as_str(),
        Some(config(&[(ENABLED_CFG.key, "false")])?),
    )
    .await?;
    assert_same(cached.as_ref(), full.as_ref()).await?;
    assert_eq!(
        cached.description().as_deref(),
        Some("other writer's durable change")
    );
    assert_eq!(cached.name().as_deref(), Some("stale writer"));
    Ok(())
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn edits_consolidate_and_missing_cache_overlay_replays_originals(
    #[case] local: bool,
) -> Result<(), BundlebaseError> {
    use bundlebase::AnyOperation;

    let (_temp, dir) = storage(local).await?;
    let builder = BundleBuilder::create(dir.url().as_str(), Some(config(&[])?)).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    builder.commit("data").await?;
    for id in 1..=3 {
        builder
            .update(&format!("SET salary = {} WHERE id = {id}", id * 100))
            .await?;
        builder.delete(&format!("id = {}", id + 10)).await?;
        builder.commit("edits").await?;
    }
    let originals: Vec<_> = builder
        .operations()
        .into_iter()
        .filter_map(|op| match op {
            AnyOperation::Delete(op) => Some(op.tombstone),
            AnyOperation::UpdateData(op) => Some(op.overlay),
            _ => None,
        })
        .collect();
    assert_eq!(originals.len(), 6);
    builder.checkpoint().await?;
    let cp_files = checkpoints(dir.as_ref()).await?;
    let body: serde_yaml_ng::Value =
        serde_yaml_ng::from_slice(&checkpoint_bytes(dir.as_ref(), &cp_files[0]).await?)?;
    let replay: Vec<AnyOperation> =
        serde_yaml_ng::from_value(body["checkpoint"]["operations"].clone())?;
    let mut compacted_deletes = Vec::new();
    let mut compacted_updates = Vec::new();
    for op in replay {
        match op {
            AnyOperation::Delete(op) => compacted_deletes.push(op.tombstone),
            AnyOperation::UpdateData(op) => compacted_updates.push(op.overlay),
            _ => {}
        }
    }
    assert_eq!(compacted_deletes.len(), 1);
    assert_eq!(compacted_updates.len(), 1);
    let vector_bytes = dir
        .file(&compacted_deletes[0])?
        .read_bytes()
        .await?
        .expect("compacted Puffin");
    let vectors = bundlebase::bundle::tombstone::deserialize(&vector_bytes)?;
    assert_eq!(
        vectors.len(),
        1,
        "one live block requires one merged vector"
    );
    assert_eq!(vectors[0].rows.len(), 3);
    assert!(
        !originals.contains(&compacted_updates[0]),
        "the merged file must differ from each one-row original"
    );
    let cached = Bundle::open(dir.url().as_str(), Some(config(&[])?)).await?;
    let full = Bundle::open(
        dir.url().as_str(),
        Some(config(&[(ENABLED_CFG.key, "false")])?),
    )
    .await?;
    assert_same(cached.as_ref(), full.as_ref()).await?;
    assert_eq!(cached.num_rows().await?, 997);
    dir.writable_file(&compacted_updates[0])?.delete().await?;
    for original in originals {
        assert!(dir.file(&original)?.exists().await?);
    }
    let fallback = Bundle::open(dir.url().as_str(), Some(config(&[])?)).await?;
    assert_same(fallback.as_ref(), full.as_ref()).await?;
    Ok(())
}

#[rstest::rstest]
#[case(false, false)]
#[case(false, true)]
#[case(true, false)]
#[case(true, true)]
#[tokio::test]
async fn cached_format_bounds_equal_full_replay_without_covered_reads(
    #[case] local: bool,
    #[case] minimum: bool,
) -> Result<(), BundlebaseError> {
    use bundlebase::bundle::operation::{SetMaxVersionOp, SetMinVersionOp};
    use bundlebase::AnyOperation;

    let (_temp, dir) = storage(local).await?;
    let builder = BundleBuilder::create(dir.url().as_str(), Some(config(&[])?)).await?;
    builder
        .attach(test_datafile("userdata.parquet"), None)
        .await?;
    builder.commit("data").await?;
    let op: AnyOperation = if minimum {
        SetMinVersionOp::setup("9999.0").into()
    } else {
        SetMaxVersionOp::setup("0.0").into()
    };
    builder
        .run_command("set incompatible format bound".to_string(), async {
            builder.apply_operation(op).await
        })
        .await?;
    builder.commit("incompatible bound").await?;
    builder.checkpoint().await?;
    let cached_error = match Bundle::open(dir.url().as_str(), Some(config(&[])?)).await {
        Ok(_) => panic!("checkpoint must enforce persisted format bounds"),
        Err(error) => error.to_string(),
    };
    let full_error = match Bundle::open(
        dir.url().as_str(),
        Some(config(&[(ENABLED_CFG.key, "false")])?),
    )
    .await
    {
        Ok(_) => panic!("full replay must enforce persisted format bounds"),
        Err(error) => error.to_string(),
    };
    assert_eq!(cached_error, full_error);
    assert!(
        cached_error.contains(if minimum {
            "requires bundlebase >="
        } else {
            "requires bundlebase <="
        }),
        "{cached_error}"
    );
    for file in manifests(dir.as_ref()).await? {
        dir.writable_subdir(META_DIR)?
            .writable_file(file.filename().expect("manifest"))?
            .write(Bytes::from_static(b"[invalid yaml"))
            .await?;
    }
    let still_cached_error = match Bundle::open(dir.url().as_str(), Some(config(&[])?)).await {
        Ok(_) => panic!("cached format bounds must still reject this reader"),
        Err(error) => error.to_string(),
    };
    assert_eq!(
        still_cached_error, cached_error,
        "covered manifests must not be read to recover cached bounds"
    );
    Ok(())
}

#[rstest::rstest]
#[case(false, false)]
#[case(false, true)]
#[case(true, false)]
#[case(true, true)]
#[tokio::test]
async fn uncovered_format_scan_precedes_unknown_operation_deserialization(
    #[case] prefix_checkpoint: bool,
    #[case] disabled: bool,
) -> Result<(), BundlebaseError> {
    let (_temp, dir) = storage(false).await?;
    let builder = BundleBuilder::create(dir.url().as_str(), Some(config(&[])?)).await?;
    builder.set_name("prefix").await?;
    builder.commit("prefix").await?;
    if prefix_checkpoint {
        builder.checkpoint().await?;
    }
    builder.set_description("future operation").await?;
    builder.commit("first uncovered manifest").await?;
    let files = manifests(dir.as_ref()).await?;
    let unknown = dir
        .writable_subdir(META_DIR)?
        .writable_file(files[1].filename().expect("manifest"))?;
    let bytes = unknown.read_bytes().await?.expect("manifest bytes");
    let yaml = std::str::from_utf8(&bytes)?.replace("setDescription", "futureOperation");
    assert!(yaml.contains("futureOperation"));
    unknown.write(Bytes::from(yaml)).await?;
    builder
        .run_command("future format bound".to_string(), async {
            builder
                .apply_operation(
                    bundlebase::bundle::operation::SetMinVersionOp::setup("9999.0").into(),
                )
                .await
        })
        .await?;
    builder
        .commit("later uncovered manifest sets format bound")
        .await?;
    let error = match Bundle::open(
        dir.url().as_str(),
        Some(config(&[(
            ENABLED_CFG.key,
            if disabled { "false" } else { "true" },
        )])?),
    )
    .await
    {
        Ok(_) => panic!("future format must be rejected"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("requires bundlebase >= 9999.0"), "{error}");
    Ok(())
}

#[tokio::test]
async fn due_automatic_checkpoint_revalidates_changed_parent() -> Result<(), BundlebaseError> {
    let (_temp, dir) = storage(false).await?;
    let policy = config(&[(EVERY_CFG.key, "2"), (GRACE_CFG.key, "0s")])?;
    let parent = BundleBuilder::create(dir.url().as_str(), Some(policy.clone())).await?;
    parent.set_name("parent").await?;
    parent.commit("parent").await?;
    let child_dir = dir.writable_subdir("child")?;
    let child = Bundle::open(dir.url().as_str(), Some(policy.clone()))
        .await?
        .extend(Some(child_dir.url().as_str()))
        .await?;
    for number in 1..=2 {
        child.set_description(&format!("child {number}")).await?;
        child.commit("child").await?;
    }
    let files = checkpoints(child_dir.as_ref()).await?;
    assert_eq!(files.len(), 1);
    let old_bytes = checkpoint_bytes(child_dir.as_ref(), &files[0]).await?;
    let old: serde_yaml_ng::Value = serde_yaml_ng::from_slice(&old_bytes)?;
    parent.set_name("updated parent").await?;
    parent.commit("parent changed").await?;
    child.set_description("child 3").await?;
    child.commit("not yet due").await?;
    assert_eq!(checkpoints(child_dir.as_ref()).await?.len(), 1);
    child.set_description("child 4").await?;
    child.commit("checkpoint due").await?;
    let files = checkpoints(child_dir.as_ref()).await?;
    assert_eq!(files.len(), 2);
    let bytes = checkpoint_bytes(child_dir.as_ref(), files.last().expect("checkpoint")).await?;
    let new: serde_yaml_ng::Value = serde_yaml_ng::from_slice(&bytes)?;
    assert_ne!(
        old["checkpoint"]["dependency"],
        new["checkpoint"]["dependency"]
    );
    assert_eq!(
        new["checkpoint"]["covers"]
            .as_sequence()
            .expect("covers")
            .len(),
        4
    );
    let full = Bundle::open(
        child_dir.url().as_str(),
        Some(config(&[(ENABLED_CFG.key, "false")])?),
    )
    .await?;
    let head = new["checkpoint"]["version"].as_str().expect("version");
    assert_eq!(&head[..12], full.version());
    let cached = Bundle::open(child_dir.url().as_str(), Some(policy)).await?;
    assert_same(cached.as_ref(), full.as_ref()).await?;
    Ok(())
}
