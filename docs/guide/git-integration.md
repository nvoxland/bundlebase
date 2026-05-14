# Storing bundles in git

A bundlebase bundle is a directory of small YAML manifests next to whatever
data files were attached. That layout fits git well: the manifests are
small text files that show up cleanly in diffs and PR review, alongside
the data files they describe.

## Using git-LFS for large data files

If your attached data files are large (multi-MB CSVs, parquet, arrow,
etc.), git-LFS is a good fit. It keeps the manifests in normal git
history and stores the data files separately, so clones don't drag every
historical revision of every file.

```bash
git lfs install
git lfs track "*.csv" "*.parquet" "*.arrow"
git add .gitattributes
git commit -m "Track large data formats in LFS"
```

Adjust the `git lfs track` patterns to match the formats you actually
attach. Keep `_bundlebase/*.yaml` out of LFS — those are small and you
want them in plain git for review.

LFS is a recommendation, not a requirement: small bundles (a few
hand-built CSVs, JSON dumps under a megabyte or two) work fine in plain
git. Reach for LFS when individual data files start showing up as
multi-MB blobs in your history.

## Git versioning

For all attached files, bundlebase records a `version`
string used to detect when the source has changed. 
By default, the version is filesystem-dependent and for local files is often derived from the
file's last-modified time.

When `system.git_versioning` is set to true, Bundlebase will instead ask git for the blob OID of the working-tree contents.

Improvements:
- The version is more stable across machines and re-clones.
- It works the same whether the data file is materialized or is still an
  unmaterialized LFS pointer. The pointer file's OID changes if and only
  if the underlying data changes, so change detection is correct without
  any LFS-specific code.

### Git integration is opt-in

Bundlebase does not assume a bundle lives in git just because it happens
to be inside a working tree. Git integration is **opt-in via the
`system.git_versioning` config flag**. Off by default, behavior is
identical to bundles outside any git repo.

## Enabling git versioning

`system.git_versioning` is a **stored-only** config: it must be saved to
the bundle manifest, not passed at runtime. This is by design — the flag
affects how `version` strings are recorded on attached files, so it has
to travel with the bundle. Passing it via `config={...}` or `SET CONFIG`
is rejected with an error pointing you at the right path.

```python
import bundlebase as bb
c = await bb.create("my-bundle")
await c.save_config("system", "git_versioning", "true")
await c.attach("data.csv")
await c.commit("Enable git-based version tracking")
```

```sql
SAVE CONFIG 'git_versioning' = 'true' FOR 'system'
```

After it's saved, every `bb.open()` of the bundle picks up the flag from
the manifest automatically. No per-session config to remember.

### Bundles that already have attached files

If a bundle already has attached files when you save the flag, every
recorded `version` gets refreshed in the same commit. The save_config
call walks each attached block, recomputes the version under the new
policy (git OID when enabling, mtime hash when disabling), and emits an
`UpdateVersionOp` for every block whose version changed. The result is
one atomic commit containing both the SaveConfigOp and the version
refreshes, so the manifest stays internally consistent and the next
query doesn't see version-mismatch errors.

### When the flag is on but git can't answer

If `system.git_versioning=true` but git can't produce an OID (file outside
any working tree, or `git` not on PATH), bundlebase **errors out** rather
than silently falling back. Opting into git versioning means you expect
git to be there — a missing git context is a configuration problem to
fix, not something to paper over.

Resolutions:

- Make sure `git` is installed and on PATH for the process running bundlebase.
- Place the bundle and any attached files inside a git working tree.
- Unset `system.git_versioning` (or set it to `"false"`) if you actually
  want the mtime-hash fallback for some files.

## Branching workflow

Git branches give you a free way to try alternate cleanups without forking
the bundle:

```bash
git checkout -b stricter-validation
```

```python
import bundlebase
c = await bundlebase.open(".")
await c.filter("email LIKE '%@%.%'")
await c.commit("Drop rows with malformed emails")
```

```bash
git add . && git commit -m "Stricter email validation"
```

Merge back to `main` if it works; throw the branch away if it doesn't. The
bundlebase commit history on the branch is preserved as part of the git
history either way.

## Notes

- **No three-way merge of data.** If two branches edit the same data
  file in incompatible ways, git (with or without LFS) gives you a
  conflict and resolution is "pick one." Treat data branches like
  feature branches: rebase early, merge often.
- **Cloning large bundles takes time.** Whether the data is in plain git
  or LFS, the first clone has to fetch it. With LFS plus
  `git clone --filter=blob:none`, the deferred fetch is per-file
  on first use. New contributors should know this so they don't think
  the bundle is broken.
- **`git diff` on binary data is opaque.** You see that a file
  changed, not what changed inside it. Use `bundlebase log` and
  `bundlebase show` to inspect what an operation did to the data; git
  just tracks that something moved.

