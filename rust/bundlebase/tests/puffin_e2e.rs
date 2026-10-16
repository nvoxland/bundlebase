//! Persistence-specific DELETE coverage. Scan, UPDATE, undo and block-version
//! semantics are covered by overlay_correctness_e2e and edit_keying_e2e.

use bundlebase::bundle::operation::{AnyOperation, DeleteOp, Operation};
use bundlebase::bundle::BundleFacade;
use bundlebase::test_utils::{random_memory_dir, test_config, test_datafile};
use bundlebase_command::BundleBuilderExt;
use bundlebase_common::BundlebaseError;
use bundlebase_data::puffin::{self, DeletionVector};
use bundlebase_io::{writable_dir_from_url, IOReadWriteDir};
use futures::TryStreamExt;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

mod common;

async fn stores() -> Result<(tempfile::TempDir, Vec<Arc<dyn IOReadWriteDir>>), BundlebaseError> {
    common::init_catalog();
    let temp = tempfile::tempdir()?;
    let url = url::Url::from_directory_path(temp.path())
        .map_err(|()| BundlebaseError::from("invalid temporary directory URL"))?;
    let local = writable_dir_from_url(&url, test_config()).await?;
    Ok((temp, vec![local, random_memory_dir()]))
}

async fn latest_vectors(
    dir: &dyn IOReadWriteDir,
) -> Result<(String, Vec<DeletionVector>), BundlebaseError> {
    let (yaml, commit, _) = common::latest_commit(dir).await?.expect("persisted commit");
    let deletes: Vec<_> = commit
        .operations()
        .into_iter()
        .filter_map(|op| match op {
            AnyOperation::Delete(op) => Some(op),
            _ => None,
        })
        .collect();
    assert_eq!(deletes.len(), 1, "one delete operation per commit: {yaml}");
    let path = deletes[0].tombstone.clone();
    assert!(path.ends_with(".overlay.tomb.puffin"), "{path}");
    assert!(path.contains('/'), "content-addressed files remain sharded");
    let bytes = dir.file(&path)?.read_bytes().await?.expect("Puffin file");
    assert_eq!(&bytes[..4], b"PFA1");
    assert_eq!(&bytes[bytes.len() - 4..], b"PFA1");
    Ok((path, puffin::deserialize(&bytes)?))
}

async fn scanned_rows(bundle: &dyn BundleFacade) -> Result<usize, BundlebaseError> {
    let mut stream = bundle.query("SELECT * FROM bundle", vec![], None).await?;
    let mut rows = 0;
    while let Some(batch) = stream.try_next().await? {
        rows += batch.num_rows();
    }
    Ok(rows)
}

#[tokio::test]
async fn delete_writes_one_puffin_blob_per_affected_block() -> Result<(), BundlebaseError> {
    let (_temp, stores) = stores().await?;
    for dir in stores {
        let builder = bundlebase::BundleBuilder::create(dir.url().as_str(), None).await?;
        // Exercise a relative attachment as well as an external URL, without
        // requiring the attachment to be Parquet.
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test_data/customers-0-100.csv");
        dir.writable_file("customers.csv")?
            .write(std::fs::read(fixture)?.into())
            .await?;
        builder.attach("customers.csv", None).await?;
        builder
            .attach(test_datafile("customers-101-150.csv"), None)
            .await?;
        builder.commit("attachments").await?;
        let (_, attachment_commit, _) = common::latest_commit(dir.as_ref())
            .await?
            .expect("attachments committed");
        let attachments: Vec<_> = attachment_commit
            .operations()
            .into_iter()
            .filter_map(|op| match op {
                AnyOperation::AttachBlock(op) => Some(op),
                _ => None,
            })
            .collect();
        assert_eq!(attachments.len(), 2);

        assert_eq!(builder.delete("\"Index\" IN (1, 2, 120)").await?, 3);
        builder.commit("delete across two blocks").await?;
        let (_, vectors) = latest_vectors(dir.as_ref()).await?;
        assert_eq!(vectors.len(), 2);
        let mut identities = BTreeSet::new();
        for vector in vectors {
            let properties = &vector.properties;
            let id = properties.get("bundlebase.block-id").expect("block ID");
            let attachment = attachments
                .iter()
                .find(|op| op.id.to_string() == *id)
                .expect("vector identifies an attachment");
            assert!(identities.insert(id.clone()), "one blob per block");
            assert_eq!(
                properties.get("bundlebase.block-version"),
                Some(&attachment.version)
            );
            assert_eq!(
                properties.get("referenced-data-file"),
                Some(&attachment.location)
            );
            assert_eq!(
                properties.get("cardinality"),
                Some(&vector.positions.len().to_string())
            );
            let expected: Vec<u64> = if attachment.location == "customers.csv" {
                vec![0, 1]
            } else {
                vec![19]
            };
            assert_eq!(vector.positions.iter().collect::<Vec<_>>(), expected);
        }
        let reopened = bundlebase::Bundle::open(dir.url().as_str(), None).await?;
        assert_eq!(reopened.num_rows().await?, 147);
        assert_eq!(scanned_rows(reopened.as_ref()).await?, 147);
    }
    Ok(())
}

#[tokio::test]
async fn missing_corrupt_and_legacy_delete_files_fail_open() -> Result<(), BundlebaseError> {
    let (_temp, stores) = stores().await?;
    for dir in stores {
        for mode in ["missing", "corrupt", "legacy"] {
            let case = dir.writable_subdir(mode)?;
            let builder = bundlebase::BundleBuilder::create(case.url().as_str(), None).await?;
            builder
                .attach(test_datafile("customers-0-100.csv"), None)
                .await?;
            builder.delete("\"Index\" = 1").await?;
            builder.commit("persist delete").await?;
            let (path, _) = latest_vectors(case.as_ref()).await?;
            let file = case.writable_file(&path)?;
            match mode {
                "missing" => file.delete().await?,
                "corrupt" => file.write(b"PFA1broken".to_vec().into()).await?,
                "legacy" => file.write(b"BBTOMB02\0\0\0\0".to_vec().into()).await?,
                _ => unreachable!(),
            }
            let result = bundlebase::Bundle::open(case.url().as_str(), None).await;
            assert!(result.is_err(), "{mode} delete files must fail replay");
            let error = result.err().expect("open must fail").to_string();
            assert!(
                error.contains("Puffin") || error.contains("puffin") || error.contains(&path),
                "failure should identify the delete sidecar: {error}"
            );
            if mode == "legacy" {
                assert!(error.to_lowercase().contains("unsupported"), "{error}");
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn concurrent_delete_deltas_union_in_both_replay_orders() -> Result<(), BundlebaseError> {
    let (_temp, stores) = stores().await?;
    for dir in stores {
        let builder = bundlebase::BundleBuilder::create(dir.url().as_str(), None).await?;
        builder
            .attach(test_datafile("customers-0-100.csv"), None)
            .await?;
        builder.commit("base").await?;
        let writer_a = bundlebase::Bundle::open(dir.url().as_str(), None)
            .await?
            .extend(None)
            .await?;
        let writer_b = bundlebase::Bundle::open(dir.url().as_str(), None)
            .await?
            .extend(None)
            .await?;
        assert_eq!(writer_a.delete("\"Index\" IN (1, 2)").await?, 2);
        assert_eq!(writer_b.delete("\"Index\" IN (2, 3)").await?, 2);
        writer_a.commit("writer A").await?;
        let (_, a_vectors) = latest_vectors(dir.as_ref()).await?;
        assert_eq!(a_vectors[0].positions.len(), 2);
        writer_b.commit("writer B").await?;
        let (_, b_vectors) = latest_vectors(dir.as_ref()).await?;
        assert_eq!(
            b_vectors[0].positions.len(),
            2,
            "persist deltas rather than a snapshot"
        );

        let meta = dir.writable_subdir("_bundlebase")?;
        let mut edits = Vec::new();
        for file in meta.list_files().await? {
            if let Some(name) = file.filename() {
                if name.ends_with(".yaml") && name != bundlebase::bundle::INIT_FILENAME {
                    let text = meta.file(name)?.read_str().await?.expect("manifest");
                    if text.contains("message: writer A") || text.contains("message: writer B") {
                        edits.push(name.to_string());
                    }
                }
            }
        }
        assert_eq!(edits.len(), 2);
        // Both manifests are based on the same attachment state. Giving them
        // the same sequence number and distinct hash suffixes forces each
        // legal tie ordering without changing their operation contents.
        meta.rename(&edits[0], "00002aaaaaaaaaaaa.yaml").await?;
        meta.rename(&edits[1], "00002bbbbbbbbbbbb.yaml").await?;
        for reverse in [false, true] {
            if reverse {
                meta.rename("00002aaaaaaaaaaaa.yaml", "swap.tmp").await?;
                meta.rename("00002bbbbbbbbbbbb.yaml", "00002aaaaaaaaaaaa.yaml")
                    .await?;
                meta.rename("swap.tmp", "00002bbbbbbbbbbbb.yaml").await?;
            }
            let reopened = bundlebase::Bundle::open(dir.url().as_str(), None).await?;
            assert_eq!(reopened.num_rows().await?, 97);
            assert_eq!(scanned_rows(reopened.as_ref()).await?, 97);
        }
    }
    Ok(())
}

#[tokio::test]
async fn inherited_block_delete_keeps_parent_location() -> Result<(), BundlebaseError> {
    let (_temp, stores) = stores().await?;
    for dir in stores {
        let parent_dir = dir.writable_subdir("parent")?;
        let child_dir = dir.writable_subdir("child")?;
        let parent = bundlebase::BundleBuilder::create(parent_dir.url().as_str(), None).await?;
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test_data/customers-0-100.csv");
        parent_dir
            .writable_file("parent-customers.csv")?
            .write(std::fs::read(fixture)?.into())
            .await?;
        parent.attach("parent-customers.csv", None).await?;
        parent.commit("parent attachment").await?;
        parent
            .create_view("customers", "SELECT * FROM bundle")
            .await?;
        parent.commit("parent view").await?;
        let original = bundlebase::Bundle::open(parent_dir.url().as_str(), None).await?;
        let child = original.extend(Some(child_dir.url().as_str())).await?;
        assert_eq!(child.delete("\"Index\" = 10").await?, 1);
        child.commit("delete inherited row").await?;
        let (_, vectors) = latest_vectors(child_dir.as_ref()).await?;
        assert_eq!(vectors.len(), 1);
        assert_eq!(
            vectors[0]
                .properties
                .get("referenced-data-file")
                .map(String::as_str),
            Some("parent-customers.csv")
        );
        assert_eq!(original.num_rows().await?, 100);
        let reopened = bundlebase::Bundle::open(child_dir.url().as_str(), None).await?;
        assert_eq!(reopened.num_rows().await?, 99);
        assert_eq!(scanned_rows(reopened.as_ref()).await?, 99);
        // The inherited view is opened from the reopened child, so its scan
        // must replay the child's committed Puffin deletion vector.
        let child_view = reopened.view("customers").await?;
        assert_eq!(child_view.num_rows().await?, 99);
        assert_eq!(scanned_rows(child_view.as_ref()).await?, 99);
        let parent_reopened = bundlebase::Bundle::open(parent_dir.url().as_str(), None).await?;
        assert_eq!(parent_reopened.num_rows().await?, 100);
        let parent_view = parent_reopened.view("customers").await?;
        assert_eq!(parent_view.num_rows().await?, 100);
        assert_eq!(scanned_rows(parent_view.as_ref()).await?, 100);
    }
    Ok(())
}

#[tokio::test]
async fn invalid_later_blob_does_not_partially_apply_delete() -> Result<(), BundlebaseError> {
    let (_temp, stores) = stores().await?;
    for dir in stores {
        let builder = bundlebase::BundleBuilder::create(dir.url().as_str(), None).await?;
        builder
            .attach(test_datafile("customers-0-100.csv"), None)
            .await?;
        builder
            .attach(test_datafile("customers-101-150.csv"), None)
            .await?;
        builder.commit("base").await?;
        let mut blocks: Vec<_> = builder
            .packs()
            .values()
            .flat_map(|pack| pack.blocks())
            .collect();
        assert_eq!(blocks.len(), 2);
        // The codec orders blobs by properties, beginning with block ID.
        // Make the malformed blob come after the valid one on disk too.
        blocks.sort_by_key(|block| block.id().to_string());
        let mut vectors = Vec::new();
        for block in &blocks {
            let properties = BTreeMap::from([
                ("bundlebase.block-id".to_string(), block.id().to_string()),
                ("bundlebase.block-version".to_string(), block.version()),
                (
                    "referenced-data-file".to_string(),
                    "fixture.csv".to_string(),
                ),
                ("cardinality".to_string(), "1".to_string()),
            ]);
            let mut positions = puffin::RoaringTreemap::new();
            positions.insert(0);
            vectors.push(DeletionVector {
                properties,
                positions,
            });
        }
        vectors[1].properties.remove("bundlebase.block-version");
        let bytes = puffin::serialize(&vectors)?;
        dir.writable_file("invalid.puffin")?
            .write(bytes.into())
            .await?;
        let operation = DeleteOp::new("invalid.puffin", "history only");
        assert!(operation.apply(builder.bundle()).await.is_err());
        for block in blocks {
            assert!(
                block.deleted_rows().is_empty(),
                "validate every blob before applying any"
            );
        }
        assert_eq!(builder.num_rows().await?, 150);
    }
    Ok(())
}

#[tokio::test]
async fn unmatched_delete_emits_no_puffin_or_delete_operation() -> Result<(), BundlebaseError> {
    let (_temp, stores) = stores().await?;
    for dir in stores {
        let builder = bundlebase::BundleBuilder::create(dir.url().as_str(), None).await?;
        builder
            .attach(test_datafile("customers-0-100.csv"), None)
            .await?;
        assert_eq!(builder.delete("\"Index\" = 9999").await?, 0);
        builder.commit("no matching deletes").await?;
        let (yaml, commit, _) = common::latest_commit(dir.as_ref()).await?.expect("commit");
        assert!(!commit
            .operations()
            .iter()
            .any(|op| matches!(op, AnyOperation::Delete(_))));
        assert!(!yaml.contains(".puffin"));
        assert!(!dir
            .list_files()
            .await?
            .iter()
            .any(|file| file.url.path().ends_with(".puffin")));
    }
    Ok(())
}
