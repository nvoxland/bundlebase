# ADR-014: Self-Validating Replay Checkpoints

**Status:** Accepted

**Date:** October 2026

## Context

Opening a bundle reads its ordered manifest history and applies its operations.
Long histories add manifest reads and repeated loads of DELETE tombstones and
UPDATE overlays. Replacing history with a reduced operation log would change
public history, version identity, or the state observed by intervening operations.
A checkpoint must remain an optional cache of authoritative manifests.

## Decision

Store checkpoint files separately in `_bundlebase/checkpoints/`. Keep every
original manifest and the complete original commits, changes, and operations in
checkpoint history. Store a separate replay operation list. Consolidate only
consecutive runs of DELETE and UPDATE operations, combining tombstones and
ordered overlays into content-addressed edit sidecars. Every other operation is
a barrier. Do not blindly deduplicate ATTACH, metadata, configuration, or other
operations: their checks and effects can depend on preceding state.

A checkpoint identifies an exact ordered manifest prefix, including full
filenames and hash suffixes. On open, validate its format, checksum, filename,
coverage/history consistency, and equality with the currently listed prefix.
Include the init commit and the parent's full committed head in the dependency
digest, including the parent dependency of a view. A late manifest inserted
inside the covered range invalidates that checkpoint. Invalid or unusable cache
files fall back to another candidate or ordinary replay.

Versioning folds each original manifest's persisted operation sequence into a
full 32-byte committed digest. The public version is its 12-character hexadecimal
prefix. Checkpoints retain the full head and resume the chain from it; compacted
replay operations never enter the original version chain. Original history is
restored separately from replay state.

Automatic maintenance runs after durable commit publication. The `checkpoint`
configuration scope defaults to `every=50`, `max_age=3d`, `grace=1h`, and
`enabled=true`. A new checkpoint is eligible after the commit-count threshold or
when the oldest uncovered commit reaches `max_age`; automatic coverage stops at
the first commit younger than `grace`. It always covers a prefix. `every=0`
disables automatic creation. Runtime-only `enabled=false` also disables checkpoint
use on open, allowing diagnostic full replay.

`CHECKPOINT` explicitly caches currently durable manifests without committing
pending changes and bypasses automatic interval/grace eligibility. Creation
retains the newest two checkpoint files by descending filename.
`CHECKPOINT PRUNE` retains the newest one by that ordering. Neither command records an operation. Pruning deletes only recognized
checkpoint files, never manifests or edit sidecars. Automatic maintenance errors
warn after the commit has succeeded; explicit maintenance reports its errors.

## Consequences

Checkpoint loading avoids individual manifest reads for its prefix, and edit-run
consolidation can reduce sidecar loads. Public history still contains the entire
original log, so checkpoint metadata remains proportional to history. Arbitrary
non-edit operations still replay; opening is not guaranteed constant time.

Caches duplicate historical metadata, and compacted sidecars remain after pruning.
Per-manifest hashing changes existing bundles' version strings once, without
rewriting manifests or adding a compatibility path.
Sidecar garbage collection requires a separate reachability design covering
original manifests, checkpoints, branches, and views.

## Decision History

- October 2026: Chose exact-prefix, dependency-validated caches and separate
  original-history/replay lists to preserve observable operation order and
  committed version identity. Limited compaction to consecutive edits rather
  than introducing unproven reductions of arbitrary operations.

## References

- [Commit-based versioning](../versioning.md)
- [Block-level edit representation](009-block-level-edit-representation.md)
- [Puffin deletion vectors](011-puffin-deletion-vectors.md)
