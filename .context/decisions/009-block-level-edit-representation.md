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
  commit writes the overlay parquet/`UpdateDataOp`. Each flush rebuilds the
  in-session overlay from the **full** pending accumulator and stores it in a
  single per-block session slot (`set_session_overlay`), so repeated UPDATEs
  in one session replace rather than accumulate — keeping the merge cost flat
  in edit count (bb-plf; a prior push-per-flush grew it O(N²)). After a
  successful commit the session slot is cleared, since the committed
  `UpdateDataOp` overlay now durably represents those edits.
- No query-level FilterOp is emitted by DELETE/UPDATE — including the
  always-rule immediate applications and their per-ATTACH re-applications.
  The `AlwaysDeleteOp`/`AlwaysUpdateOp` **rules** themselves are unchanged:
  they persist and re-execute against newly attached data; that is what makes
  "always" semantics live long-term. On ATTACH a rule is applied to the
  **newly attached block(s) only** (`select_row_ids_in_blocks` /
  `evaluate_update_cols_in_blocks`) — re-running a rule against blocks it
  already processed would re-tombstone deleted rows and double-apply
  non-idempotent SET expressions like `SET n = n + 1` (bb-c6l).
- Explicit user `filter()` operations are untouched — `FilterOp` remains the
  representation for user-authored SELECT transformations.

### Edits apply beneath every user `filter()`

Tombstones and overlays live in the blocks, so they take effect at scan time,
*below* every query-level operation — including user `filter()`s recorded
**before** the edit. An earlier filter is therefore evaluated against the
updated values:

```sql
FILTER WITH SELECT * FROM bundle WHERE id <= 10   -- 10 rows visible
UPDATE bundle SET id = 500 WHERE id = 3           -- now 9 rows visible
```

Row 3 disappears: the filter sees `id = 500`. Likewise, DELETE/UPDATE WHERE
clauses are evaluated against the base blocks (with prior edits applied), not
against the filtered/renamed DataFrame, so they can match rows an earlier
filter hides. The same holds for `CAST COLUMN`: an edit predicate sees the
base type, so a numeric comparison on a CSV (text) column needs an inline
`CAST(col AS DOUBLE)`; the text-vs-number ordering check rejects the bare form,
as it does for queries. Pinned by `test_filter_then_update_filter_sees_updated_values`.

### Edits are keyed by `(BlockId, version)`, not block position

Tombstone files (`BBTOMB02`) and overlay parquet row groups (`block` metadata)
name the block as `<block_id>@<version>`, plus the row number within it; the
builder's pending edits use the same key. Block *positions* in the base pack
are not stable: DETACH removes a block, REPLACE re-appends one, and since
commits are numbered from a fresh listing (bff1a16) another writer's DETACH or
REPLACE can be replayed before this writer's tombstone. A positional key then
silently drops the edit or applies it to the wrong block.

Keying by version also defines REPLACE: a replaced block gets a new version,
and edits recorded against the old version (committed or still pending) no
longer apply — row numbers in old content can't be assumed to line up with
new content. Edits for block versions no longer attached are dropped at
commit.

Undo/rollback of pending edits is delta-based (bb-8lk): each change records
only the rows it newly tombstoned, the prior value of each update cell it
wrote, and the WHERE clauses it added, and undo reverses exactly that.

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

**Behavioral correction (intentional):** DELETE now follows SQL
three-valued logic — rows where the predicate evaluates to NULL are NOT
deleted. The old `WHERE NOT (pred)` filter also excluded NULL-predicate rows
from query results (NOT(NULL) is NULL), silently over-deleting relative to
the recorded tombstones; e.g. `DELETE WHERE salary >= 0` on data with NULL
salaries now correctly leaves the NULL rows in place.

**Negative / accepted:**
- In-session edit visibility depends on the block flush discipline — new
  block-touching paths must honor `deleted_rows`/`update_overlays` (the
  `overlay_correctness_e2e` suite pins this).
- Changes that record no operations (a DELETE's change is empty until the
  commit attaches the `DeleteOp`) rely on the pending-snapshot bookkeeping
  for undo/rollback.
- **Index fast-path limitation:** the btree index is built on base values, so
  `DataBlock::scan` skips the index lookup (and stats/page pruning) for any
  block with overlays and falls back to overlay-applying scans — correct, but a
  selective indexed query loses its speedup on updated blocks until an
  overlay-aware index lookup is built (bb-dkf). `search()` handles the text
  index differently: hits on rows whose *indexed* column was UPDATEd are
  dropped and the query is re-run over those rows' current text with a small
  throwaway index (those rows' BM25 scores come from that small corpus). Text
  indexes over computed columns are not re-evaluated.

## References

- Profiling data and decision history: `ideas/filterop-vs-block-overlays.md`
- Regression suite: `rust/bundlebase/tests/overlay_correctness_e2e.rs`
- Benchmark: `rust/bundlebase/benches/filterop_vs_blockdata.rs`
