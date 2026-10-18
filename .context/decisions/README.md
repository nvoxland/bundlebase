# Architecture Decision Records (ADRs)

## Purpose

This directory contains Architecture Decision Records documenting significant architectural and design choices made in the Bundlebase project. ADRs capture the **context**, **decision**, and **consequences** of each choice to help current and future developers understand why decisions were made.

## When to Create an ADR

Create an ADR when:
- **Selecting among multiple viable technical approaches** - e.g., choosing between different libraries, frameworks, or architectural patterns
- **Introducing new dependencies** - especially major ones like DataFusion or Arrow
- **Modifying core architectural patterns** - changes to the three-tier architecture, operation pipeline, etc.
- **Making decisions difficult to reverse** - choices that would be expensive to change later
- **Documenting constraints** - hard rules like "no `.unwrap()`" or "streaming only"

**Don't create ADRs for:**
- Trivial or self-evident choices
- Easily reversible decisions (can try it and change if needed)
- Code style preferences handled by linters/formatters
- Implementation details within an already-decided approach

## ADR Template

```markdown
# ADR-XXX: [Title]

**Status:** [Proposed | Accepted | Deprecated | Superseded by ADR-YYY]

## Context

What is the issue or situation that motivates this decision?

- What problem are we solving?
- What constraints do we face?
- What alternatives did we consider?

## Decision

What is the change we're proposing/implementing?

- What approach did we choose?
- How will it be implemented?
- What are the key technical details?

## Consequences

### Positive

What benefits does this decision provide?

### Negative

What drawbacks or limitations does this introduce?

### Neutral

What are the implications that aren't clearly positive or negative?
```

## Numbering Convention

ADRs are numbered sequentially starting from 001. Use three digits with leading zeros:
- `001-rust-core.md`
- `002-datafusion-arrow.md`
- `010-some-future-decision.md`

**Next available number:** 015

## Process

1. **Propose**: Create a new ADR file with status "Proposed"
2. **Discuss**: Open a pull request for team review and discussion
3. **Accept**: Merge the PR with status changed to "Accepted"
4. **Supersede**: If a decision changes, create a new ADR that supersedes the old one

**Important**: ADRs are **immutable once accepted**. If a decision changes, create a new ADR (with status "Superseded by ADR-XXX" on the old one) rather than editing the original. Exception: when the implemented design drifted from the recorded decision without a new decision being made, the ADR may be **amended** — keep the original context and add a "Decision History" section recording what changed and why, with the status marked "Amended <date>".

## Index of Decisions

| ADR | Title | Status |
|-----|-------|--------|
| [001](001-rust-core.md) | Rust Core Library | Accepted |
| [002](002-datafusion-arrow.md) | DataFusion and Apache Arrow | Accepted |
| [003](003-streaming-only.md) | Streaming-Only Execution | Accepted |
| [004](004-three-tier-architecture.md) | Three-Tier Architecture | Accepted (amended 2026-06) |
| [005](005-mutable-operations.md) | Modification Methods Use Interior Mutability | Accepted (amended 2026-06) |
| [006](006-lazy-evaluation.md) | Lazy Operation Evaluation | Accepted (amended 2026-06) |
| [007](007-no-unwrap.md) | No .unwrap() Allowed | Accepted |
| [008](008-no-mod-rs.md) | No mod.rs Files | Accepted |
| [009](009-block-level-edit-representation.md) | Block-Level Tombstones/Overlays Are the Sole Edit Representation | Accepted |
| [010](010-trust-model.md) | Trust Model: Trusted Client, Untrusted Bundle Data | Accepted |
| [011](011-puffin-deletion-vectors.md) | Puffin Deletion Vectors for DELETE Persistence | Accepted |
| [012](012-native-semantic-model.md) | Native Semantic Metadata with an Ossie View | Accepted |
| [013](013-arrow-stream-export.md) | Standard Arrow Stream Export | Accepted |
| [014](014-self-validating-checkpoints.md) | Self-Validating Replay Checkpoints | Accepted (amended 2026-10) |

## References

- [Architecture Overview](../architecture.md)
- [AI Rules](../ai-rules.md) - Hard constraints derived from ADRs
- [Development Guide](../development.md) - Workflows influenced by architectural decisions
