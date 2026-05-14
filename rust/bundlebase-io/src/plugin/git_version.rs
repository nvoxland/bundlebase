//! Compute git blob OIDs for files in a git working tree.
//!
//! Used by `ObjectStoreFile::version()` for local files when the
//! `system.git_versioning` config is enabled. The git blob OID is a better
//! change-detection token than the mtime-hash fallback: stable across
//! machines, stable across re-clones, unchanged when a file is touched
//! without content changes, and unchanged across the untracked → `git add`
//! → `git commit` transitions. It also works uniformly for materialized
//! data files and unmaterialized git-LFS pointer files — a pointer file's
//! OID changes iff the data it references changes, so callers don't need
//! to special-case LFS.
//!
//! Implemented by shelling out to the `git` CLI to avoid pulling in a git
//! library; the calls are cheap (no network, no full-tree scans).
//!
//! # xxh3 content hash is unaffected
//!
//! Bundlebase records a content hash per attached file as xxh3-128. That
//! hash is what storage paths and dedup are keyed on. This module only
//! affects the `version` change-detection field — swapping in git OIDs for
//! the content hash would invalidate every existing bundle.

use std::path::Path;
use tokio::process::Command;

/// Returns the git blob OID of the file at `path` if and only if the file
/// exists inside a git working tree.
///
/// The OID is computed from the current bytes on disk, so:
/// - Untracked, tracked-clean, and tracked-dirty files all get an OID.
/// - The OID is stable across `git add` / `git commit` (none of those
///   change the file's bytes), so adding a previously-untracked file does
///   not spuriously invalidate the version.
/// - Two files with identical content always get the same OID.
///
/// Returns `None` if any of:
/// - `git` is not available on PATH
/// - The path doesn't exist
/// - The path is not inside a git working tree
///
/// The returned OID is git's object hash (sha1 by default; sha256 in
/// sha256-formatted repositories) as a lowercase hex string.
pub async fn working_tree_oid(path: &Path) -> Option<String> {
    let canonical = tokio::fs::canonicalize(path).await.ok()?;
    let parent = canonical.parent()?;
    let path_str = canonical.to_string_lossy().into_owned();

    // Confirm the file is inside a git working tree. Without this gate,
    // bundles outside any repo would still get git-flavored versions
    // whenever git is on PATH, which would surprise users not opting in
    // to git as a storage layer.
    run_git(parent, &["rev-parse", "--is-inside-work-tree"]).await?;

    // Hash the working-tree contents. This is `git hash-object` of whatever
    // is on disk right now — independent of whether the file has been
    // `git add`ed or committed.
    run_git(parent, &["hash-object", &path_str]).await
}

async fn run_git(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    Some(stdout.trim().to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command as StdCommand;
    use tempfile::tempdir;

    fn git(dir: &Path, args: &[&str]) {
        let status = StdCommand::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .expect("git command failed to spawn");
        assert!(status.success(), "git {:?} failed", args);
    }

    fn init_repo(dir: &Path) {
        git(dir, &["init", "--quiet"]);
        git(dir, &["config", "user.email", "t@example.com"]);
        git(dir, &["config", "user.name", "test"]);
    }

    #[tokio::test]
    async fn returns_none_for_nonexistent_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("missing.txt");
        assert_eq!(working_tree_oid(&path).await, None);
    }

    #[tokio::test]
    async fn returns_none_outside_repo() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("file.txt");
        std::fs::write(&path, b"hello").unwrap();
        assert_eq!(working_tree_oid(&path).await, None);
    }

    #[tokio::test]
    async fn returns_oid_for_untracked_file_in_repo() {
        let dir = tempdir().unwrap();
        init_repo(dir.path());
        let path = dir.path().join("file.txt");
        std::fs::write(&path, b"hello").unwrap();

        let oid = working_tree_oid(&path).await.expect("expected an OID");
        assert_eq!(oid.len(), 40);
    }

    #[tokio::test]
    async fn oid_unchanged_across_git_add() {
        // The whole point of using the git OID for change detection: it is
        // a content hash, so `git add`ing a file does not flip the version.
        let dir = tempdir().unwrap();
        init_repo(dir.path());
        let path = dir.path().join("file.txt");
        std::fs::write(&path, b"hello").unwrap();

        let before_add = working_tree_oid(&path).await.expect("before add");
        git(dir.path(), &["add", "file.txt"]);
        let after_add = working_tree_oid(&path).await.expect("after add");
        git(dir.path(), &["commit", "-m", "add file", "--quiet"]);
        let after_commit = working_tree_oid(&path).await.expect("after commit");

        assert_eq!(before_add, after_add);
        assert_eq!(after_add, after_commit);
    }

    #[tokio::test]
    async fn returns_oid_for_tracked_clean_file_matching_hash_object() {
        let dir = tempdir().unwrap();
        init_repo(dir.path());
        let path = dir.path().join("file.txt");
        std::fs::write(&path, b"hello").unwrap();
        git(dir.path(), &["add", "file.txt"]);

        let oid = working_tree_oid(&path).await.expect("expected an OID");

        // Cross-check against `git hash-object` directly.
        let expected = StdCommand::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["hash-object", path.to_str().unwrap()])
            .output()
            .unwrap();
        let expected_oid = String::from_utf8(expected.stdout).unwrap().trim().to_string();
        assert_eq!(oid, expected_oid);
    }

    #[tokio::test]
    async fn returns_oid_when_working_tree_dirty() {
        // A dirty file still gets an OID — the OID reflects what's on disk,
        // not the index. Modifying the file changes the OID, which is the
        // correct change-detection signal.
        let dir = tempdir().unwrap();
        init_repo(dir.path());
        let path = dir.path().join("file.txt");
        std::fs::write(&path, b"hello").unwrap();
        git(dir.path(), &["add", "file.txt"]);
        let clean_oid = working_tree_oid(&path).await.expect("clean OID");

        std::fs::write(&path, b"hello world").unwrap();
        let dirty_oid = working_tree_oid(&path).await.expect("dirty OID");

        assert_ne!(clean_oid, dirty_oid);
    }

    #[tokio::test]
    async fn oid_unchanged_after_touch_with_same_content() {
        let dir = tempdir().unwrap();
        init_repo(dir.path());
        let path = dir.path().join("file.txt");
        std::fs::write(&path, b"hello").unwrap();
        git(dir.path(), &["add", "file.txt"]);

        let first = working_tree_oid(&path).await.expect("first OID");
        // Re-write same bytes (mtime changes, content doesn't).
        std::fs::write(&path, b"hello").unwrap();
        let second = working_tree_oid(&path).await.expect("second OID");
        assert_eq!(first, second);
    }
}
