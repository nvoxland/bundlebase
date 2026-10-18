---
seo_title: "Checkpoint Maintenance - Bundlebase"
---

# Checkpoint Maintenance

Replay checkpoints help bundles with long commit histories open with fewer
manifest reads. They also combine consecutive DELETE and UPDATE runs into fewer
edit files for replay. Your original commits, operation history, and version
identifiers remain available.

## Create a checkpoint

Run this command on a writable bundle:

```sql
CHECKPOINT
```

It caches the manifests already committed to storage. It does not commit pending
changes or add an operation to history. Manual creation bypasses the automatic
commit-count and age thresholds, including the grace period.

Bundlebase retains two checkpoint files after creation, ordered by descending
filename (covered manifest number, then hash). To retain only the first file in
that order:

```sql
CHECKPOINT PRUNE
```

Pruning removes checkpoint files only. It never deletes commit manifests or
DELETE/UPDATE sidecars, including sidecars created for checkpoint replay.

## Configure automatic creation

Automatic maintenance runs after a successful commit. Settings use the
`checkpoint` [configuration scope](configuration.md):

| Key | Default | Meaning |
| --- | --- | --- |
| `every` | `50` | Create after this many uncovered commits; `0` disables automatic creation. |
| `max_age` | `3d` | Also create when the oldest uncovered commit reaches this age. |
| `grace` | `1h` | Leave recent commits outside automatically created checkpoints. |
| `enabled` | `true` | Allow checkpoint loading and automatic creation. Runtime-only. |

Durations accept integers followed by `s`, `m`, `h`, or `d` for seconds, minutes,
hours, or days. For example:

```sql
SET CONFIG every = '100' FOR 'checkpoint';
SET CONFIG max_age = '7d' FOR 'checkpoint';
SET CONFIG grace = '2h' FOR 'checkpoint';
```

The count or age threshold makes creation eligible; the grace period can defer
it further. Automatic coverage stops at the first recent commit to preserve an
ordered prefix. Automatic maintenance failures produce a warning after your
commit is durable; they do not undo the commit. Explicit maintenance commands
report failures directly.

## Diagnose replay

To open a bundle with full manifest replay, pass `enabled=false` in its
`checkpoint` configuration when opening it. For example:

```python
import bundlebase as bb

bundle = await bb.open("my/data", config={"checkpoint": {"enabled": "false"}})
```

`enabled` cannot be saved into the bundle's configuration. It is a runtime
control for diagnostics. Setting `every=0` disables automatic creation while
allowing existing checkpoints to be used.

Checkpoints are validated against the exact listed manifest prefix, the bundle's
init metadata, and the parent version when applicable. An invalid checkpoint is
ignored in favor of another usable checkpoint or full replay. Keep original
manifests available: they remain authoritative.

Checkpoint history still contains the complete original metadata, and ordinary
operations still replay. Checkpoints reduce some storage reads; they do not make
all bundle opens take constant time.

See [Versioning](versioning.md) for commits, history, and extending bundles.
