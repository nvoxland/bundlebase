# ADR-011: Puffin Deletion Vectors for DELETE Persistence

**Status:** Accepted

**Date:** October 2026

## Context

DELETE currently persists row identities in a Bundlebase-specific tombstone
container. Stable `(BlockId, version)` references are already shipped for
DELETE and Parquet UPDATE overlays. The remaining format decision is how to
persist deleted row positions while retaining those identities. Checkpoints,
compaction, conflict reporting, UPDATE storage changes, and in-memory bitmap
refactoring are outside this decision.

## Decision

Persist pending deletes as Apache Iceberg Puffin files containing deletion
vector blobs. Write one content-addressed file per commit with pending deletes,
and one blob per affected block version. Files hold deltas; replay unions the
vectors for each matching block version. Preserve the existing sharded
`ContentAddress` naming. The format is `ContentFormat::Puffin`, category
`Overlay`, subtype `tomb`, yielding the suffix
`.overlay.tomb.puffin`.

Each blob uses the standard `deletion-vector-v1` payload and standard CRC-32
checksum, with the portable 64-bit Roaring representation. Its string
properties include `bundlebase.block-id`, `bundlebase.block-version`,
`referenced-data-file`, and `cardinality`. Bundlebase applies a vector only
when both its stable block ID and version identify a live block. A stale
version is skipped; an absent or corrupt referenced file is an error.

The manifest operation remains `DeleteOp` with `tombstone` and `where` fields.
The `where` expression remains history metadata. UPDATE continues to persist
as Parquet overlays keyed by the same stable block version. REPLACE changes a
block version, so deletes for its previous contents do not affect the new
contents.

There is no legacy reader, migration, fallback, format bump, or new version
operation. Old tombstone files and missing or corrupt Puffin files must fail
clearly during replay; they must never cause deleted rows to reappear through
silent omission. Checkpoints are not implemented by this decision.

## Consequences

### Positive

- DELETE persistence uses a standardized container and deletion-vector
  encoding, which can support interoperability work later.
- Roaring vectors offer disk compression potential for suitable row-position
  patterns.
- Concurrent delete commits remain mergeable because replay unions vectors
  by stable block version.

### Negative

- Bundles containing legacy tombstone files require a future migration or
  cannot be opened by versions implementing this decision. No compatibility
  path is provided.
- Puffin does not promise faster queries. Query performance depends on the
  existing scan and in-memory delete handling.

### Neutral

- This adopts the standard container and payload but does not make a bundle an
  Iceberg table or promise that external engines can query it.
- A Puffin file is written only for commits with pending deletes; empty
  selections produce no deletion file or `DeleteOp`.

## References

- [ADR-009: Block-Level Tombstones/Overlays Are the Sole Representation of DELETE/UPDATE](009-block-level-edit-representation.md)
- [Puffin specification](https://iceberg.apache.org/puffin-spec/)
- [Puffin deletion-vector design revision](../../ideas/mergeable-deletes-and-updates.md)
