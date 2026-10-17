# ADR-012: Native Semantic Metadata with an Ossie View

**Status:** Accepted

**Date:** October 2026

## Context

Bundlebase already records column descriptions and named metric definitions as
operations. Fields use stable `ColumnId` references. Metrics currently use
names, and `DefineMetricOp` establishes an expression, while separate property operations
set descriptions and AI context.
Python, Arrow metadata, metric computation, and MCP consume resolved metadata.

The initial bb-b3b proposal centered on importing and exporting Ossie files.
The intended workflow is now direct semantic reads and updates through the
same change and commit machinery as other bundle operations. Independent
metadata edits should remain independent when commit streams are combined.

Semantic changes resolve as all other operations replay in manifest filename
order, including changes published by other writers. Later writes to a
property win. There is no separate user-triggered merge step or conflict
resolution protocol.

## Decision

### Design priorities

Optimize for a clean current implementation and a pleasant user experience.
Previous bundle formats, persisted operation shapes, Python signatures,
command spelling, and MCP response fields impose no compatibility requirement.
Do not add migrations, legacy readers, aliases, deprecation periods, or parallel
representations to preserve them. Remove superseded implementation paths.

Follow Bundlebase's operation and command architecture while choosing the best
current public interface. Preserve useful capabilities such as descriptions
and metric computation; their old API shapes need not survive. Choose concise,
consistent names, clear omission-versus-clear behavior, actionable validation
errors, and equivalent command/Python/MCP behavior. Review those workflows in
the contract task before implementation. Internal IDs, replay machinery, and
Ossie dialect details should not burden routine user edits.

### State and interfaces

Maintain one resolved semantic model derived from operations. Use Ossie's
concepts for its supported fields, metrics, AI context, and declared keys.
Keep identity, operation addressing, and revision information in Bundlebase's
internal model; project a valid Ossie document at the interchange boundary.
No document blob is authoritative and reads do not record operations.

The v1 property inventory is explicit:

| Level | Mutable semantic properties | Derived properties |
|---|---|---|
| Model | Name, description, AI context | Version and provenance |
| Selected dataset | Description, AI context, primary key, unique keys | Name, source |
| Field | Description, label, AI context | Name, identity expression, datatype |
| Metric | Name at creation, expression, description, AI context | Planned datatype where supported |

Model name and description use existing bundle name/description operations.
Dataset description and AI context are distinct properties; do not alias them
silently to model properties. Field renaming and type changes continue through
existing schema operations. Metric rename is outside v1; delete/create is
explicit. Unsupported dimension/relationship metadata is rejected on document
application rather than discarded.

Expose a semantic snapshot on `BundleFacade` and typed semantic updates on
`BundleBuilder`. A snapshot represents committed plus pending state when read
from a builder. It is immutable and does not become a second mutable store.
Clients reread after updates. Concrete public method names and return shapes
are settled in the contract task before bindings are implemented.

Provide one update path shared by Rust, commands, Python, and MCP. Updates
contain typed actions with named properties. Do not address fields or metrics
by array indices or accept unrestricted JSON document mutation.

### Identity and operation granularity

Address a dataset by its bundle identity, a field by `ColumnId`, and a metric
by its exact name as the simple logical identity for v1. This choice is based
on current behavior and scope, not compatibility with old histories. Revisit
it if a cleaner current design requires a different identity. No migration is
required. Metric rename
remains explicit delete/create. Field names are convenient public selectors,
resolved to stable column IDs before recording operations.

Create and drop are entity lifecycle operations. Property changes separately
set or clear descriptions, labels, expressions, and AI context. An expression
update cannot also overwrite a metric description. A convenience call that
changes several properties emits several operations inside one change.

Definition and update operations follow the existing operation conventions:
named Rust structs in named module files, serde camelCase fields, `setup`
construction where needed, and registration through `define_any_operation!`.
Each implements the normal `Operation` contract, including `describe()`,
`check()`, `apply()`, and the existing content-based version hash. Metadata
operations follow `DescribeColumnOp` and `DefineMetricOp`: `apply()` can be a
no-op when the shared resolver derives state from the operation list, and
`apply_dataframe()` retains the default unchanged DataFrame behavior.

Commands follow existing parsing, SQL/name translation, `run_command`, and
`apply_operation` patterns. Builder methods use `&self` and the established
return convention; batch commands group ordinary operations in one change.
Use the normal definition-command style; exact API names and syntax may
change to make the current interface coherent. Granular semantic
updates add ordinary operation variants; they do not introduce a second
operation trait, nested patch interpreter, or a whole-document
`DefineSemanticModelOp`. Public update requests lower into these same variants
before persistence. Serialization/version, parser/display, and replay tests
follow existing operation tests.

Use explicit clear actions. An omitted property means unchanged. AI context
objects support updates to individual top-level members; arrays such as
synonyms are atomic property values. Replacing an entire AI context is an
explicit action whose write scope overlaps its member updates. A string AI
context is an atomic value. Avoid arbitrary recursive patch semantics in v1.
An absent member in an update means unchanged; explicit removal clears it.
JSON null is accepted only where the pinned property's schema permits it,
and is never an implicit removal instruction.

Represent the primary key as one ordered composite declaration. Represent
unique keys as separate declarations identified by stable member IDs, allowing
independent additions/removals. Keys describe intent and are not enforced.
Specify canonical declaration identity in the contract: preserve primary-key
column order and normalize unique-key member sets for duplicate/removal
matching while preserving declared display order.
No-op document application produces no operation or empty change. Explicit
native commands record their write intent even if the local value matches;
those assignments must participate in normal last-write-wins replay with
changes from other writers. Do not add semantic deduplication to the builder
operation pipeline.

### Validation and ordered replay

Validate user actions against prospective local state before applying a batch
in one outer `do_change`. Validate targets, names, key members, AI context,
and metric SQL using the existing planner and function registry. Preserve
stable column references and public SQL rendering. No data scan or implicit
commit is needed. Invalid input preserves pending changes. Undo reverses the
complete batch.

Resolution is a deterministic fold over the normal manifest and operation
order. A property setter affects only that property. There are no expected
previous values, base revisions, three-way merge, or stale-write conflicts in
the semantic update protocol. A snapshot revision may describe provenance;
it is not a write precondition. The manifest filename hash breaks equal-number
ties exactly as it does for other operations; within a change, operation order
is authoritative. Last means last in replay order, not client wall-clock time.

| Operations in replay order | Result |
|---|---|
| Set different fields or independent metric properties | Both changes survive |
| Set the same property twice | Later value wins |
| Set then clear a property | Property is absent |
| Clear then set a property | Later value is present |
| Add different unique-key declarations | Both survive |
| Add/remove the same unique-key declaration | Later membership operation wins |
| Replace primary key twice | Later declaration wins |
| Replace AI context, then set one member | Member changes the replacement object |
| Set an AI context member, then replace context | Replacement supersedes prior members |

A context-member set after a string or cleared context establishes an object
containing that member. A member removal against a string or absent context
is a no-op. Arrays remain atomic member values. These rules keep every
validated action replayable without a type conflict.

Metric creation/removal changes existence in the same ordered fold. Removing
a metric clears its properties; a later explicit creation starts fresh.
Property updates alone do not recreate a removed metric. An update targeting
an absent metric or a dropped field is ignored during replay, while new local
commands still validate that their target exists. A later edit by metric name
addresses whichever live metric currently has that name. Concurrent creates
of the same name resolve by replay order for the expression. A definition
ensures the metric exists and writes its expression; descriptions and AI
context are separate operations and survive unrelated definitions. A prior
drop clears that state, so an explicit definition after a drop starts fresh.

Apply these existence rules in the resolver without making ordinary combined
metadata histories fail to open. Ignore field metadata for dropped ColumnIds
and omit key declarations whose member columns are no longer live. Preserve
metric definitions whose references become invalid after other schema/function
operations; surface that invalidity when planning computation or strict
export, without blocking unrelated metadata reads. Authored expression updates
are validated locally. Malformed persisted operations and corrupt files remain
errors; normal concurrent overwrite is not corruption.

Tests force both commit tie orders and assert the expected winner for each
order, plus identical results across readers replaying the same ordered files.
Independent property writes survive either order. This provides deterministic
ordered resolution, not order-independent convergence for overlapping writes.

### Consolidating existing functionality

Carry column descriptions, metric definition/removal, metadata reads, and
metric computation into the shared semantic model. Refactor or replace the
current operation variants and convenience APIs wherever that produces a
cleaner implementation. Keep convenience methods when they make common tasks
easier, with one canonical implementation underneath. A definition/update
call must write only the properties it owns; settle consistent omitted and
explicit-clear semantics without preserving historical signatures or behavior.
Schema validity does not prove a metric's business meaning or portability.

Arrow descriptions, MCP describe/analyze, and Python accessors consume the
same resolved model. Design one coherent current MCP response; retain useful
schema information where it serves users, and remove redundant legacy fields
instead of scheduling a compatibility window. Update affected callers,
examples, stubs, and tests together. Do not introduce duplicate resolution.

### Ossie adapters

Vendor an immutable schema revision and attributed upstream examples in a
standalone `bundlebase-ossie` crate. Validate JSON/YAML against that schema and
test upstream round trips without field loss. Builds and reads remain offline.
Use the pinned schema's required version, expression dialect structure, and
vendor extension encoding. Keep schema validation separate from the subset
Bundlebase can apply.

Produce the Ossie view from current state without requiring an export step.
An Ossie document need not represent all DataFusion SQL; existing metrics
remain valid internally even when a strict portable export is unsupported.
Document a strict export error or explicit vendor representation for such
expressions; never silently label them ANSI SQL or break metric computation.
Provenance is generated, credential-safe, and ignored as mutation input.

Optional document application defaults to updating supplied metadata; omission
does not remove entities or clear metadata. Explicit replacement requires a
declared scope and exposes a preview of the diff against current local state.
It lowers to explicit property setters, clears, creations, and removals.
Those operations replay normally with changes from other writers; omission
is never persisted as a wildcard instruction to erase future additions.
No base snapshot or conflict protocol is required. Export/import commands and
Python path helpers are adapters over these capabilities.

### Scope

V1 handles the currently opened bundle, including a view opened directly when
its normal builder mutation rules permit metadata changes. A parent's update
does not mutate multiple child histories. No inferred foreign keys from
arbitrary join expressions, computed-field creation, physical schema changes,
join creation, dialect translation, or key enforcement is included.

## Consequences

### Positive

- Users and agents can read and edit semantics without files.
- Property operations preserve independent changes and useful history.
- Native interfaces and Ossie tools use the same metadata state.
- Optional document workflows reuse atomic validation, diff, and undo.

### Negative

- Overlapping property writes can overwrite another writer's value by design.
- Metric names identify logical metrics; name reuse has normal ordered semantics.
- Ossie SQL portability is narrower than the DataFusion execution surface.
- Schema changes can leave a metric unplannable until a later correction;
  unrelated semantic reads remain available.

### Neutral

- Ossie is the public vocabulary and exchange format; Bundlebase defines
  operation, identity, revision, and history semantics.
- The detailed implementation sequence and acceptance criteria are tracked in
  bb-b3b and its children.

## Decision History

- 2026-10-16: Replaced the file-first bb-b3b plan with native semantic reads,
  granular updates, and optional document adapters at the user's request.
- 2026-10-16: Corrected concurrency design at the user's direction: all semantic
  operations resolve during normal commit-file replay with property-level
  last-write-wins. Removed checked updates, base preconditions, three-way
  reconciliation, and the proposed metric-lifetime identity requirement.
- 2026-10-16: Made existing definition-operation conventions an explicit
  requirement; semantic updates use the normal operation and command pipeline.
- 2026-10-16: Removed compatibility requirements at the user's direction.
  Current operation/API shapes may change; prioritize one clean implementation
  and a pleasant native workflow, without migrations or compatibility layers.
- The original `ideas/ossie-semantic-model.md` is superseded where it conflicts
  with this proposal and the revised epic design.

### Implementation contract

The native state is `BundleFacade::semantic_state()`, with an async
`semantic_model()` Ossie projection. Granular commands use `ALTER COLUMN`,
`ALTER METRIC`, `ALTER SEMANTIC MODEL`, and `ALTER DATASET bundle`. Explicit
`DROP` clauses clear properties; `SET AI CONTEXT MEMBER` changes one top-level
member. `DECLARE PRIMARY KEY`, `DECLARE UNIQUE KEY`, and corresponding `DROP`
commands edit key declarations without data validation.

The document adapter uses one dataset named and sourced as `bundle`.
`SHOW SEMANTIC MODEL` reads the current projection; optional JSON/YAML
`EXPORT SEMANTIC MODEL TO` and `IMPORT SEMANTIC MODEL FROM` commands share that
adapter. Import defaults to MERGE. REPLACE explicitly selects all supported
metadata in the opened model. A diff is lowered to the concrete setters,
clears, creations, and removals and applied in one ordinary change. For
example, changing a metric expression while omitting its description in MERGE
emits only an expression operation; removing that description in REPLACE emits
a separate clear. Reapplying an identical document emits no change.

The pinned Ossie revision is `64a1ae44035f5360dcd53401a29bcd900fdce349`,
specification version `0.2.0.dev0`. The standalone crate preserves full standard
documents, while the Bundlebase application adapter rejects supported-schema
features outside its v1 subset. Provenance in the generated BUNDLEBASE vendor
extension is informational and ignored as mutation input. Strict Ossie views
validate metric expressions against the supported ANSI SQL syntax, reject
imported-function and dangling-column references, and do not translate
dialects. Native metric reads and computation remain independent of this
export boundary; schema validation does not prove business meaning or full
SQL portability.

- 2026-10-16: Accepted the native state, concrete property operations, shared
  Ossie projection, and MERGE/REPLACE document-diff implementation contract.

- Explicit native commands use the normal operation pipeline without local
  value deduplication. Whole-document diffs alone omit unchanged properties.
  CREATE METRIC always emits a definition so replay can recreate a metric
  removed by another writer earlier in commit order.
