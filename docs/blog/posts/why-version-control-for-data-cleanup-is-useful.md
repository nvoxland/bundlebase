---
date: 2026-04-16
categories:
  - Why
---

# Why I want version control for data cleanup

The painful part of data cleanup is usually not writing the transformation. It is explaining later why the transformation exists and whether the current output still matches the original intent.

This is why I keep wanting version control semantics for data work.

## The normal failure mode

A developer gets a CSV from somewhere, removes bad columns, patches a few values, and exports a new file. A week later somebody asks:

- which rows changed?
- why was this column dropped?
- did we already fix this bug once?

If the answer lives in notebook cells, chat messages, or memory, the workflow is broken.

## Commits are not just for source code

Bundlebase treats data transformations as operations that can be committed. That means the cleanup work has a history instead of just an output.

```python
import bundlebase

c = await bundlebase.create("./customer-bundle")
await c.attach("./customers.csv")
await c.drop_column("internal_notes")
await c.rename_column("fname", "first_name")
await c.commit("Remove internal-only data and normalize names")
```

That commit message looks boring, which is good. Boring audit trails are useful.

## Why this beats "just keep old files"

Keeping `customers-clean.csv`, `customers-clean-v2.csv`, and `customers-clean-real-final.csv` is not version control. It is artifact hoarding.

A real history gives you:

- a reason for each change
- a readable sequence of operations
- a stable snapshot you can reopen
- the option to extend from an existing committed state

That last part matters more than it sounds. Often I do not want to start over. I want to build on yesterday's cleaned data and make one more change without flattening everything into a new anonymous export.

## This is especially useful when the cleanup is small

The weird thing is that versioning helps most when the transformations are simple.

If the cleanup is complicated, everybody expects documentation. If it is "just drop three columns and fix a date format," people skip the documentation because it feels obvious. Then six months later nobody remembers why those exact three columns disappeared.

Version history is cheap insurance against that.

## It also changes team conversations

Instead of saying "here is the cleaned file," you can say:

1. open this bundle
2. inspect the commits
3. extend it if you need your own variant

That is a much better handoff for developers because it turns cleanup into a process instead of a blob.
