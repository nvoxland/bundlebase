---
date: 2026-05-13
categories:
  - Guides
---

# Using Bundlebase with Git LFS

A bundlebase bundle is just a directory: small YAML manifests next to whatever
raw data files you imported. That layout happens to fit git + git-lfs almost
exactly, and the combination is more useful than I initially expected.

<!-- more -->

## The shape of a bundle on disk

If you peek inside a bundle, you see something like this:

```
customers/
├── _bundlebase/
│   ├── 00000000000000000.yaml      # commit 0 (root)
│   ├── 0000171cc39dc9a4c.yaml      # commit 1
│   └── 0000171cc39e21bb1.yaml      # commit 2
├── 16/
│   └── 69e1f93b3e9010.csv          # imported data
└── 81/
    └── a4c1f0...parquet
```

Two distinct kinds of files:

- **YAML manifests** — tiny, text, describe the operations that make up each commit
- **Data files** — large, binary, content-addressed by hash

Git is great at the first kind and bad at the second. Git-LFS is built for the
second. So you point each at what it does well.

## Setup

```bash
cd customers/
git init
git lfs install
git lfs track "*.csv" "*.parquet" "*.arrow" "*.json"
git add .gitattributes
git add .
git commit -m "Initial bundle"
```

That's the whole setup. The YAML manifests in `_bundlebase/` go into normal
git history. The data files go into LFS. A `git clone` of the repo pulls down
the manifests immediately and fetches data blobs on demand.

## Why this combination is nice

**Readable diffs in PRs.** Bundlebase commits are recorded as YAML, so a pull
request that adds a `drop_column` and a `rename_column` shows up as a few
lines of YAML somebody can actually review. The data files are referenced by
hash, so a content change shows up too — just as an opaque pointer change,
which is honest about what happened.

**Branches map cleanly.** Git branches give you a free way to try an
alternate cleanup without forking the whole bundle:

```bash
git checkout -b try-stricter-validation
# in python:
#   await c.filter("email LIKE '%@%.%'")
#   await c.commit("Drop rows with malformed emails")
git add . && git commit -m "Try stricter email validation"
```

If it works, merge to main. If not, throw the branch away. The bundlebase
commit history on that branch is preserved as part of the git history, so
"what did we try and abandon" is also recorded.

**One repo for code + data.** If your transformation logic lives in a script
or notebook, and the bundle lives next to it, a single `git checkout <sha>`
puts both the code and the data in a known matching state. That alone solves
a surprising amount of "which version of the data was that script run
against" pain.

**LFS handles the size problem.** A 4GB parquet file in regular git is a
disaster. In LFS it's a 130-byte pointer, and the actual blob is fetched
lazily. `git clone --filter=blob:none` plus `git lfs pull` for just the
commits you need is a reasonable workflow on big repos.

**Git OIDs as the change-detection token (opt-in).** `SAVE CONFIG` the
`system.git_versioning` key to `true` and bundlebase will use the git
blob OID of each local source file as that file's `version` instead of the
mtime-derived hash. The OID is stable across machines and re-clones,
doesn't churn when a file is touched without changing, and stays the same
across the untracked → `git add` → `git commit` transitions. It also works
uniformly for materialized files and unmaterialized LFS pointers — the
pointer file's own OID changes if and only if the data it references
changes. The xxh3 content hash bundlebase records per attached file is
unchanged; this only affects the change-detection `version` field. Off by
default, and stored-only — the flag lives in the bundle manifest and
travels with the bundle, so collaborators don't need to remember a magic
config when they `open()`.

## Branching workflow that works

1. `main` holds the canonical, agreed-upon cleaned bundle.
2. Each ad-hoc question or experiment gets a branch.
3. The branch adds bundlebase commits on top of main.
4. Merge back to main only the experiments that produced something everyone
   agrees on.

```bash
git checkout main
git pull
git checkout -b q2-revenue-cut
```

```python
import bundlebase
c = await bundlebase.open("./customers")
await c.filter("signup_date >= '2026-01-01'")
await c.commit("Restrict to 2026 signups for Q2 revenue review")
```

```bash
git add . && git commit -m "Q2 revenue cut"
git push -u origin q2-revenue-cut
```

A teammate can `git checkout q2-revenue-cut`, open the bundle, and they're
looking at exactly the same filtered state. No "send me the file" step.

## Notes

**Two layers of "commit" is confusing at first.** A bundlebase commit is an
entry in `_bundlebase/`. A git commit is an entry in `.git/`. They overlap
but don't have to — you can stage three bundlebase commits and then make a
single git commit covering all of them, or vice versa. I usually try to keep
them 1:1 but it's not enforced and probably can't be.

**Merging diverged data is "pick one".** If two branches both edit the same
underlying CSV in incompatible ways, git-lfs will give you a conflict on the
pointer file, and resolving it is just choosing which blob wins. There is no
actual three-way merge of tabular data, and bundlebase doesn't try to invent
one. For now, treat data branches like feature branches: rebase early,
merge often, don't let them diverge for weeks.

**Cloning is slower than people expect.** Even with `blob:none` filtering,
the first checkout that actually needs data has to pull it. A new contributor
opening the repo for the first time will wait. Document this so they don't
think it's broken.

**Renames of source files aren't smart.** If you re-import the same logical
CSV with a different filename, git-lfs sees a delete + add, not a rename.
The bundle's internal addressing is by content hash so it's fine, but `git
log --follow` won't help you trace the history of a particular logical
table.

**`git diff` on a binary pointer is useless.** You see the pointer changed,
not what changed in the data. To actually see what a commit did to the data
you have to read the YAML manifest or check out both sides and run a query.
Bundlebase's own commit log is the better tool here — git just tracks that
something moved.


## A small end-to-end example

```bash
mkdir orders-bundle && cd orders-bundle
git init
git lfs install
git lfs track "*.csv" "*.parquet"
git add .gitattributes && git commit -m "Configure LFS"
```

```python
import bundlebase
c = await bundlebase.create(".")
await c.save_config("system", "git_versioning", "true")
await c.attach("../raw/orders-2026-q1.csv")
await c.drop_column("internal_notes")
await c.commit("Import Q1 orders, drop internal notes")
```

The `system.git_versioning` flag is what tells bundlebase to use git blob
OIDs as the change-detection version on local files. It's off by default;
without it you'd still get a working bundle, just with mtime-derived
versions instead of content-addressed ones. The flag is stored-only — it
has to be saved into the bundle manifest, not passed at runtime, because
it affects what gets recorded on every attach.

```bash
git add . && git commit -m "Q1 orders, internal notes removed"
git remote add origin git@github.com:me/orders-bundle.git
git push -u origin main
```

A colleague then does:

```bash
git clone git@github.com:me/orders-bundle.git
cd orders-bundle
git lfs pull
```

```python
import bundlebase
c = await bundlebase.open(".")
df = await c.to_pandas()
```

No special config needed on `open` — `git_versioning` is stored in the
bundle manifest, so it applies automatically.

That's it. No "which version of the file did you mean", no Slack attachments,
no `orders-clean-v2-FINAL-actually.csv`. The data, its history, and the
operations that produced it are all in one place that everyone already knows
how to clone.

## Should you do this?

For datasets in the megabytes-to-low-gigabytes range with a small team, yes,
this is a good fit. The combination is more than the sum of the parts: git
gives you the history and branching of the *operations*, LFS gives you a
practical place to put the *bytes*, and bundlebase gives you a meaningful
diff between the two.

For terabyte-scale data, a shared object store with bundlebase pointing at it
is going to scale better than git-lfs. But I'd still keep the YAML manifests
in regular git — the audit trail is worth it on its own.

Let me know if you try this and hit something I missed.
