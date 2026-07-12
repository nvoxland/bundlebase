# ADR-009: Block-Level Tombstones/Overlays Are the Sole Representation of DELETE/UPDATE

**Status:** Accepted

**Date:** July 2026

## Context

From their introduction, `DELETE` and `UPDATE` recorded **two redundant
representations** of each edit:

1. **Block-level row-identity data** — delete tombstones (`DeleteOp`, row
   numbers applied to `block.deleted_rows`) and update overlays
   (`UpdateDataOp`, row-number-keyed replacement values), applied inside
   `DataBlock::scan`.
2. **A query-level `FilterOp`** — `SELECT * FROM bundle WHERE NOT (pred)` for
   DELETE, a full-schema `CASE WHEN (pred) THEN (val) ELSE col END` projection
   for UPDATE — persisted in the manifest and replayed as a nested DataFrame
   plan layer above every scan, forever, one layer per edit.

The FilterOp originally provided in-session visibility (block tombstones were
only populated at commit) and DataFrame-level composition (a later
filter/delete evaluating against updated values). In practice it silently
became the layer carrying almost all correctness, masking several bugs in the
under-exercised block-level machinery.

### The Problem

Profiling (see `ideas/filterop-vs-block-overlays.md` for full data) showed the
FilterOp representation is a per-query planning cost that grows with edit
count and cannot be helped by caching or data locality:

- **UPDATE CASE projections stack exponentially** (~2× per layer): 16 updates
  = 3s per query, 20 = 57s, 100 never finishes. Twenty UPDATE statements in
  one session made a 10K-row bundle effectively unqueryable.
- DELETE `WHERE NOT` layers stack linearly (~2.7ms/layer) — bounded only by
  edit count, paid on every query.
- The block-level representation is **flat in edit count** on every measured
  axis (SELECT, selective WHERE, COUNT(*), open+plan): 100 edits cost the
  same as 1.
- COUNT(*) via tombstones is metadata-speed (`num_rows − deleted.len()`);
  via FilterOps it must stream the predicate columns.
- The dual representation paid **both** costs additively on every query.

No compact/optimize step exists, and none could fix in-session accumulation.

## Decision

**`DELETE` and `UPDATE` record only the block-level representation.**

- DELETE: `select_row_ids` → `mark_deleted`, which stores RowIds for the
  commit-time tombstone/`DeleteOp` AND mirrors the row numbers into the
  blocks immediately (`flush_pending_deletes_to_blocks`) for in-session
  visibility.
- UPDATE: `evaluate_update_cols` → `flush_pending_updates_to_blocks`; the
  commit writes the overlay parquet/`UpdateDataOp`.
- No query-level FilterOp is emitted by DELETE/UPDATE — including the
  always-rule immediate applications and their per-ATTACH re-applications.
  The `AlwaysDeleteOp`/`AlwaysUpdateOp` **rules** themselves are unchanged:
  they persist and re-execute against newly attached data; that is what makes
  "always" semantics live long-term.
- Explicit user `filter()` operations are untouched — `FilterOp` remains the
  representation for user-authored SELECT transformations.

### Invariants this relies on (enforced by tests)

- Predicate evaluation for edit selection reads **updated values**:
  `select_row_ids`/`evaluate_update_cols` apply block overlays to batch
  values (positions untouched) before evaluating WHERE clauses.
- Every scan path applies tombstones and overlays or defers to one that does:
  the index fast-path and stats/page pruning defer when overlays exist; the
  page-filtered read defers when the block has deletes; `search()` applies
  both itself.
- **Wrapper order**: the overlay applier wraps the raw stream (values change,
  positions don't); the delete filter wraps above it. Both are ordinal-based,
  so deleting first would shift the ordinals the overlay sees.
- Overlay values are coerced to the base column's Arrow type at evaluation
  (e.g. `SET salary = 999` on a Float64 column, `SET x = NULL`) and the
  appliers cast when representations drift (Utf8 vs Utf8View).
- `undo()`/failed commands restore per-change pending snapshots and rebuild
  block state, so an undone or failed edit never leaks into a commit.
- Statistics account for edits: the delete filter subtracts tombstones from
  `num_rows` (exact stays exact — this is the metadata-speed COUNT), and the
  overlay applier degrades column min/max to inexact.

## Consequences

**Positive:**
- Query cost is independent of edit count; ~20-update sessions went from
  unusable to sub-millisecond.
- One representation to reason about; the manifest stays small.
- COUNT(*) after deletes is metadata-speed.

**Negative / accepted:**
- In-session edit visibility depends on the block flush discipline — new
  block-touching paths must honor `deleted_rows`/`update_overlays` (the
  `overlay_correctness_e2e` suite pins this).
- Changes that record no operations (a DELETE's change is empty until the
  commit attaches the `DeleteOp`) rely on the pending-snapshot bookkeeping
  for undo/rollback.
- The index fast-path is skipped for blocks with overlays until an
  overlay-aware index lookup is built.

## References

- Profiling data and decision history: `ideas/filterop-vs-block-overlays.md`
- Regression suite: `rust/bundlebase/tests/overlay_correctness_e2e.rs`
- Benchmark: `rust/bundlebase/benches/filterop_vs_blockdata.rs`
