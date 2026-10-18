//! Optional, self-validating caches of a manifest prefix. Original commits remain authoritative.
use super::commit::{manifest_version, BundleCommit};
use super::init::InitCommit;
use super::operation::{AnyOperation, BundleChange};
use super::{resolve_attach_sidecars, Bundle, BundleBuilder, META_DIR};
use crate::io::{read_yaml, IOReadWriteDir};
use crate::BundlebaseError;
use bundlebase_common::checkpoint_config::CheckpointConfig;
use bundlebase_common::file_info::FileInfo;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(crate) const DIRECTORY: &str = "checkpoints";

#[derive(Debug)]
pub(crate) struct ReplayError(String);
impl std::fmt::Display for ReplayError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}
impl std::error::Error for ReplayError {}
pub(crate) fn replay_error(error: BundlebaseError) -> BundlebaseError {
    Box::new(ReplayError(error.to_string()))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct CoveredCommit {
    pub file: String,
    pub commit: BundleCommit,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Checkpoint {
    format: u32,
    dependency: String,
    pub covers: Vec<String>,
    pub version: String,
    pub history: Vec<CoveredCommit>,
    pub operations: Vec<AnyOperation>,
    pub min_version: Option<String>,
    pub max_version: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct CheckpointFile {
    checkpoint: Checkpoint,
    checksum: String,
}

fn digest<T: Serialize>(value: &T) -> Result<String, BundlebaseError> {
    // serde_json's default map representation sorts keys, including operation maps.
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(
        &serde_json::to_value(value)?,
    )?)))
}

pub(crate) fn dependency(init: &InitCommit, parent_head: &str) -> Result<String, BundlebaseError> {
    digest(&(init, parent_head))
}

fn filename(covers: &[String], dependency: &str) -> Result<String, BundlebaseError> {
    let last = covers
        .last()
        .ok_or("Cannot checkpoint an empty manifest prefix")?;
    Ok(format!(
        "{:05}{}.yaml",
        manifest_version(last),
        &digest(&(covers, dependency))?[..12]
    ))
}

fn checkpoint_files(listing: &[FileInfo], manifest_dir: &dyn IOReadWriteDir) -> Vec<FileInfo> {
    let prefix = format!(
        "{}/{DIRECTORY}/",
        manifest_dir.url().as_str().trim_end_matches('/')
    );
    let mut files: Vec<_> = listing
        .iter()
        .filter(|file| {
            file.url
                .as_str()
                .strip_prefix(&prefix)
                .is_some_and(|relative| !relative.contains('/') && is_checkpoint_name(relative))
        })
        .cloned()
        .collect();
    files.sort_by(|a, b| b.filename().cmp(&a.filename()));
    files
}

fn is_checkpoint_name(name: &str) -> bool {
    name.len() == 22
        && name.ends_with(".yaml")
        && name.as_bytes()[..5].iter().all(u8::is_ascii_digit)
        && name.as_bytes()[5..17].iter().all(u8::is_ascii_hexdigit)
}

pub(crate) async fn select(
    listing: &[FileInfo],
    manifests: &[FileInfo],
    manifest_dir: &dyn IOReadWriteDir,
    expected_dependency: &str,
) -> Option<Checkpoint> {
    select_candidate(listing, manifests, manifest_dir, Some(expected_dependency)).await
}

/// An absent dependency is only suitable for scheduling maintenance, never replay.
async fn select_candidate(
    listing: &[FileInfo],
    manifests: &[FileInfo],
    manifest_dir: &dyn IOReadWriteDir,
    expected_dependency: Option<&str>,
) -> Option<Checkpoint> {
    for file in checkpoint_files(listing, manifest_dir) {
        let result: Result<Checkpoint, BundlebaseError> = async {
            let checkpoint_file = manifest_dir.file(&format!(
                "{DIRECTORY}/{}",
                file.filename().ok_or("Missing checkpoint filename")?
            ))?;
            let stored: CheckpointFile = read_yaml(checkpoint_file.as_ref())
                .await?
                .ok_or("Checkpoint disappeared")?;
            let cp = stored.checkpoint;
            if cp.format != 1
                || expected_dependency.is_some_and(|expected| cp.dependency != expected)
                || digest(&cp)? != stored.checksum
            {
                return Err("Checkpoint format, dependency, or checksum mismatch".into());
            }
            if cp.covers.is_empty()
                || cp.covers.len() != cp.history.len()
                || cp.covers.windows(2).any(|pair| pair[0] >= pair[1])
                || cp
                    .history
                    .iter()
                    .zip(&cp.covers)
                    .any(|(entry, file)| &entry.file != file)
                || filename(&cp.covers, &cp.dependency)? != file.filename().unwrap_or("")
            {
                return Err("Invalid checkpoint coverage".into());
            }
            let floor = cp.covers.last().ok_or("Empty checkpoint")?;
            let prefix: Vec<_> = manifests
                .iter()
                .filter_map(FileInfo::filename)
                .take_while(|file| *file <= floor.as_str())
                .collect();
            if prefix != cp.covers.iter().map(String::as_str).collect::<Vec<_>>() {
                return Err("Checkpoint does not cover the exact listed manifest prefix".into());
            }
            if hex::decode(&cp.version)?.len() != 32 {
                return Err("Invalid checkpoint version head".into());
            }
            Ok(cp)
        }
        .await;
        match result {
            Ok(cp) => {
                log::debug!("Using checkpoint {}", file.url);
                return Some(cp);
            }
            Err(error) => log::debug!("Ignoring checkpoint {}: {error}", file.url),
        }
    }
    None
}

impl Checkpoint {
    /// Restore original commits, hydrating the same sidecars used by ordinary replay.
    pub(crate) async fn hydrate(
        &mut self,
        data_dir: &dyn IOReadWriteDir,
    ) -> Result<(), BundlebaseError> {
        for entry in &mut self.history {
            entry.commit.url = Some(data_dir.subdir(META_DIR)?.file(&entry.file)?.url().clone());
            entry.commit.data_dir = Some(data_dir.url().clone());
            resolve_attach_sidecars(&mut entry.commit, data_dir).await?;
        }
        let mut replay = self.replay_commit();
        resolve_attach_sidecars(&mut replay, data_dir).await?;
        self.operations = replay.operations();
        // Ordinary UPDATE replay tolerates missing source overlays. A missing
        // compacted cache overlay must instead reject this cache, so the reader
        // retries the original manifest operations and their original files.
        // Validate all cache edit payloads before any replay mutates bundle state.
        for op in &self.operations {
            match op {
                AnyOperation::Delete(delete) => {
                    let bytes = data_dir
                        .file(&delete.tombstone)?
                        .read_bytes()
                        .await?
                        .ok_or_else(|| {
                            BundlebaseError::from(format!(
                                "Checkpoint DELETE file not found: {}",
                                delete.tombstone,
                            ))
                        })?;
                    super::tombstone::deserialize(&bytes)?;
                }
                AnyOperation::UpdateData(update) => {
                    let bytes = data_dir
                        .file(&update.overlay)?
                        .read_bytes()
                        .await?
                        .ok_or_else(|| {
                            BundlebaseError::from(format!(
                                "Checkpoint UPDATE file not found: {}",
                                update.overlay,
                            ))
                        })?;
                    super::update_overlay::read_overlay_parquet(&bytes)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub(crate) fn replay_commit(&self) -> BundleCommit {
        BundleCommit {
            url: None,
            data_dir: None,
            author: String::new(),
            message: String::new(),
            timestamp: String::new(),
            changes: vec![BundleChange {
                id: uuid::Uuid::nil(),
                description: "checkpoint".into(),
                operations: self.operations.clone(),
                suppress_auto_reindex: false,
            }],
        }
    }
}

async fn parent_head(
    data_dir: &dyn IOReadWriteDir,
    init: &InitCommit,
    config: &crate::BundleConfig,
) -> Result<String, BundlebaseError> {
    let parent = if init.view.is_some() {
        Some(
            url::Url::parse(&format!(
                "{}/",
                data_dir.url().as_str().trim_end_matches('/')
            ))?
            .join("../")?,
        )
    } else {
        init.from.clone()
    };
    match parent {
        Some(url) => Ok(Box::pin(Bundle::open(
            url.as_str(),
            Some(config.passed_config().as_ref().clone()),
        ))
        .await?
        .version_head()),
        None => Ok("00".repeat(32)),
    }
}

async fn read_commit(
    file: &FileInfo,
    data_dir: &dyn IOReadWriteDir,
) -> Result<BundleCommit, BundlebaseError> {
    let manifest_dir = data_dir.subdir(META_DIR)?;
    let mut commit: BundleCommit = read_yaml(
        manifest_dir
            .file(file.filename().ok_or("Invalid manifest filename")?)?
            .as_ref(),
    )
    .await?
    .ok_or("Manifest disappeared while checkpointing")?;
    commit.url = Some(file.url.clone());
    commit.data_dir = Some(data_dir.url().clone());
    resolve_attach_sidecars(&mut commit, data_dir).await?;
    Ok(commit)
}

fn old_enough(commit: &BundleCommit, age: std::time::Duration, now: DateTime<Utc>) -> bool {
    DateTime::parse_from_rfc3339(&commit.timestamp)
        .ok()
        .is_some_and(|time| {
            now.signed_duration_since(time)
                .to_std()
                .is_ok_and(|elapsed| elapsed >= age)
        })
}

impl BundleBuilder {
    /// Create a cache of all currently listed durable manifests. Does not commit pending changes.
    pub async fn checkpoint(&self) -> Result<&Self, BundlebaseError> {
        self.write_checkpoint(true).await?;
        Ok(self)
    }

    /// Keep the newest checkpoint. Manifests and edit sidecars are never removed.
    pub async fn checkpoint_prune(&self) -> Result<&Self, BundlebaseError> {
        prune(self.bundle().data_dir().as_ref(), 1).await?;
        Ok(self)
    }

    pub(crate) async fn maybe_checkpoint(&self) {
        if let Err(error) = self.write_checkpoint(false).await {
            // Manifest publication already succeeded. A cache failure cannot undo that commit.
            log::warn!("Commit succeeded, but checkpoint maintenance failed: {error}");
        }
    }

    async fn write_checkpoint(&self, force: bool) -> Result<(), BundlebaseError> {
        let config = self.bundle().config();
        let policy = CheckpointConfig::from_provider(config.as_ref())?;
        if !force && (!policy.enabled || policy.every == 0) {
            return Ok(());
        }
        let data_dir = self.bundle().data_dir();
        let manifest_dir = data_dir.writable_subdir(META_DIR)?;
        let listing = manifest_dir.list_files().await?;
        let manifests = super::commit::manifest_files_from_listing(&listing, manifest_dir.as_ref());
        if manifests.is_empty() {
            return if force {
                Err("Cannot checkpoint a bundle before its first commit".into())
            } else {
                Ok(())
            };
        }
        if !force {
            // A structurally valid cache is a scheduling hint even if its parent
            // dependency has changed. Never use this candidate to seed replay:
            // once maintenance is due, resolve the current parent and select
            // again with its validated dependency below.
            let hint = select_candidate(&listing, &manifests, manifest_dir.as_ref(), None).await;
            let covered = hint.as_ref().map_or(0, |cp| cp.covers.len());
            if covered == manifests.len() {
                return Ok(());
            }
            if ((manifests.len() - covered) as u64) < policy.every {
                // Eligibility only needs the timestamp; do not hydrate sidecars.
                let first: BundleCommit = read_yaml(
                    manifest_dir
                        .file(
                            manifests[covered]
                                .filename()
                                .ok_or("Invalid manifest filename")?,
                        )?
                        .as_ref(),
                )
                .await?
                .ok_or("Manifest disappeared while checkpointing")?;
                if !old_enough(&first, policy.max_age, Utc::now()) {
                    return Ok(());
                }
            }
        }
        let init: InitCommit = read_yaml(manifest_dir.file(super::INIT_FILENAME)?.as_ref())
            .await?
            .ok_or("Cannot checkpoint an uncommitted bundle")?;
        let base_head = parent_head(data_dir.as_ref(), &init, config.as_ref()).await?;
        let dependency = dependency(&init, &base_head)?;
        let previous = select(&listing, &manifests, manifest_dir.as_ref(), &dependency).await;
        let covered = previous.as_ref().map_or(0, |cp| cp.covers.len());
        if !force {
            if covered == manifests.len() {
                return Ok(());
            }
            if ((manifests.len() - covered) as u64) < policy.every {
                let first = read_commit(&manifests[covered], data_dir.as_ref()).await?;
                if !old_enough(&first, policy.max_age, Utc::now()) {
                    return Ok(());
                }
            }
        }
        let now = Utc::now();
        let mut history = Vec::new();
        for file in &manifests {
            let commit = read_commit(file, data_dir.as_ref()).await?;
            // Coverage is a prefix, even when timestamps are out of order.
            if !force && !old_enough(&commit, policy.grace, now) {
                break;
            }
            history.push(CoveredCommit {
                file: file.filename().ok_or("Missing manifest name")?.to_owned(),
                commit,
            });
        }
        if history.is_empty() || (!force && history.len() <= covered) {
            return Ok(());
        }
        let mut version = base_head;
        let mut min_version = init.min_version.clone();
        let mut max_version = init.max_version.clone();
        let mut original_ops = Vec::new();
        for entry in &history {
            version = Bundle::advance_manifest_head(&version, &entry.commit)?;
            for op in entry.commit.operations() {
                match &op {
                    AnyOperation::SetMinVersion(op) => min_version = Some(op.version.clone()),
                    AnyOperation::SetMaxVersion(op) => max_version = Some(op.version.clone()),
                    _ => {}
                }
                original_ops.push(op);
            }
        }
        let operations =
            super::checkpoint_edits::consolidate_operations(&original_ops, data_dir.as_ref())
                .await?;
        let cp = Checkpoint {
            format: 1,
            dependency,
            covers: history.iter().map(|entry| entry.file.clone()).collect(),
            version,
            history,
            operations,
            min_version,
            max_version,
        };
        let name = filename(&cp.covers, &cp.dependency)?;
        let checksum = digest(&cp)?;
        let file = CheckpointFile {
            checkpoint: cp,
            checksum,
        };
        // Canonicalize all nested maps before YAML serialization. No writer-specific metadata.
        let yaml = serde_yaml_ng::to_string(&serde_json::to_value(file)?)?;
        let directory = manifest_dir.writable_subdir(DIRECTORY)?;
        directory
            .writable_file(&name)?
            .write(bytes::Bytes::from(yaml))
            .await?;
        prune(data_dir.as_ref(), 2).await?;
        Ok(())
    }
}

async fn prune(data_dir: &dyn IOReadWriteDir, keep: usize) -> Result<(), BundlebaseError> {
    let manifest_dir = data_dir.writable_subdir(META_DIR)?;
    let files = checkpoint_files(&manifest_dir.list_files().await?, manifest_dir.as_ref());
    for file in files.into_iter().skip(keep) {
        manifest_dir
            .writable_file(&format!(
                "{DIRECTORY}/{}",
                file.filename().ok_or("Missing checkpoint filename")?
            ))?
            .delete()
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::operation::SetNameOp;
    use crate::bundle_config::{PassedBundleConfig, Scope};
    use crate::test_utils::random_memory_dir;
    use crate::BundleFacade;
    use bundlebase_common::checkpoint_config::{EVERY_CFG, GRACE_CFG};

    async fn commit_name(builder: &BundleBuilder, name: &str) -> Result<(), BundlebaseError> {
        let name = name.to_owned();
        builder
            .do_change("set name", move |bundle| {
                Box::pin(
                    async move { bundle.apply_operation(SetNameOp::setup(&name).into()).await },
                )
            })
            .await?;
        builder.commit("metadata change").await?;
        Ok(())
    }

    #[tokio::test]
    async fn ineligible_maintenance_does_not_read_parent_after_previous_checkpoint(
    ) -> Result<(), BundlebaseError> {
        let parent_dir = random_memory_dir();
        let child_dir = random_memory_dir();
        let mut config = PassedBundleConfig::new();
        let scope = Scope::try_from("checkpoint")?;
        config.set(&scope, EVERY_CFG.key, "3");
        config.set(&scope, GRACE_CFG.key, "0s");
        let parent = BundleBuilder::create(parent_dir.url().as_str(), Some(config)).await?;
        commit_name(&parent, "parent").await?;
        let child = parent.extend(Some(child_dir.url().as_str())).await?;
        for name in ["one", "two", "three"] {
            commit_name(&child, name).await?;
        }
        let meta = child_dir.writable_subdir(META_DIR)?;
        assert_eq!(
            checkpoint_files(&meta.list_files().await?, meta.as_ref()).len(),
            1
        );

        // An unavailable parent makes any attempted reopen observable as an error.
        parent_dir
            .writable_subdir(META_DIR)?
            .writable_file(super::super::INIT_FILENAME)?
            .delete()
            .await?;
        for name in ["four", "five"] {
            commit_name(&child, name).await?;
            child.write_checkpoint(false).await?;
        }
        assert!(child.write_checkpoint(true).await.is_err());

        // Age eligibility must resolve the parent even below the count threshold.
        let listing = meta.list_files().await?;
        let manifests = super::super::commit::manifest_files_from_listing(&listing, meta.as_ref());
        let tail = manifests[3].filename().ok_or("Missing tail filename")?;
        let file = meta.writable_file(tail)?;
        let mut commit: BundleCommit = read_yaml(file.as_ref()).await?.ok_or("Missing tail")?;
        commit.timestamp = "2000-01-01T00:00:00Z".into();
        file.write(bytes::Bytes::from(serde_yaml_ng::to_string(&commit)?))
            .await?;
        assert!(child.write_checkpoint(false).await.is_err());

        commit.timestamp = Utc::now().to_rfc3339();
        file.write(bytes::Bytes::from(serde_yaml_ng::to_string(&commit)?))
            .await?;
        commit_name(&child, "six").await?;
        assert!(child.write_checkpoint(false).await.is_err());
        Ok(())
    }
}
