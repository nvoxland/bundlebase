"""Create an empty Bundlebase bundle for Claude Code transcript history.

The shipped bundle holds:

* A Go connector compiled to a shared library, registered with `IMPORT
  CONNECTOR ... WITH (src = '...')` so a recipient can recover the source.
* A `CREATE SOURCE` definition that points the connector at `~/.claude/projects`
  (or any directory passed via `--source-dir`).
* B-tree indexes on `project_id`, `session_id`, `event_type`, `timestamp` and
  an inverted text index on `search_text`.
* Two views (`message_events`, `tool_events`).

To get the indexes and views into the empty bundle, the script first builds a
*full* working bundle locally (with `fetch=True` against your own transcripts,
which gives the bundle a real schema to resolve indexes / views against),
then runs `EXPORT EMPTY` to strip the data while preserving every structural
op. Recipients of the empty bundle run `FETCH base ADD` to populate it from
their own transcript history under the same shape.

The script also packages the empty bundle as a single ``.tar`` for
distribution. (FFI shared libraries can't be ``dlopen``-ed from inside a tar,
so recipients of the tar must extract it before running ``FETCH``.)

Run from this directory:

    python create_claude_history_bundle.py [--bundle-dir DIR] [--tar PATH]
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

import bundlebase.sync as bb


SCRIPT_DIR = Path(__file__).resolve().parent
DEFAULT_BUNDLE_DIR = SCRIPT_DIR / "claude-history-bundle"
DEFAULT_TAR = SCRIPT_DIR / "claude-history-bundle.tar.gz"
DEFAULT_SOURCE_DIR = Path.home() / ".claude" / "projects"
DEFAULT_PATTERNS = "*.jsonl,**/*.jsonl"
CONNECTOR_NAME = "local.claude_history"
CONNECTOR_SOURCE = SCRIPT_DIR / "claude_history_connector"


def library_filename() -> str:
    """Return the platform-appropriate shared-library filename."""
    suffix = {"darwin": ".dylib", "win32": ".dll"}.get(sys.platform, ".so")
    return f"libclaude_history_connector{suffix}"


def build_connector(output_dir: Path) -> Path:
    """Compile the sibling Go connector to a shared library."""
    output_dir.mkdir(parents=True, exist_ok=True)
    library_path = output_dir / library_filename()
    subprocess.run(
        ["go", "build", "-buildmode=c-shared", "-o", str(library_path), "."],
        cwd=CONNECTOR_SOURCE,
        check=True,
    )
    return library_path.resolve()


def zip_connector_source(output_path: Path) -> Path:
    """Bundle the Go source into a zip the script attaches with WITH (src = ...)."""
    output_path.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(output_path, "w", zipfile.ZIP_DEFLATED) as zf:
        for file in CONNECTOR_SOURCE.rglob("*"):
            if file.is_file():
                zf.write(file, file.relative_to(CONNECTOR_SOURCE.parent))
    return output_path


def create_bundle(
    bundle_dir: Path,
    source_dir: Path,
    patterns: str,
    tar_path: Path | None,
) -> None:
    if bundle_dir.exists():
        raise SystemExit(
            f"Bundle directory already exists: {bundle_dir}\n"
            "Remove it first if you want to recreate."
        )
    if tar_path is not None and tar_path.exists():
        raise SystemExit(
            f"Tar file already exists: {tar_path}\nRemove it first if you want to recreate."
        )

    build_dir = bundle_dir.parent / ".build"
    full_bundle_dir = build_dir / "full-bundle"
    if full_bundle_dir.exists():
        shutil.rmtree(full_bundle_dir)

    library_path = build_connector(build_dir)
    source_zip = zip_connector_source(build_dir / "claude_history_connector_source.zip")

    # ----- Phase 1: build a full working bundle (with data) -----
    #
    # CREATE SOURCE auto-fetches by default, which populates the schema.
    # CREATE INDEX / CREATE VIEW need that schema to resolve column references,
    # so they have to run on the populated bundle. The data we ingest here
    # gets stripped in phase 2 — only the structural ops survive.
    full = bb.create(
        str(full_bundle_dir),
        config={"system": {"allow_external_code": "true"}},
    )

    full.set_name("Claude History")
    full.set_description(
        "Flattened Claude Code transcript history with one row per transcript event. "
        "Optimized for querying sessions, prompts, replies, tool calls, and tool results."
    )

    # Register the compiled connector and ship the Go source alongside it so
    # any recipient can audit, fork, or rebuild via `EXPORT SOURCE`.
    full.import_connector(
        CONNECTOR_NAME,
        f"ffi::{library_path}",
        src=str(source_zip),
    )

    source_args = {"patterns": patterns}
    if source_dir.resolve() != DEFAULT_SOURCE_DIR.resolve():
        source_args["source_dir"] = str(source_dir.resolve())
    # Default fetch=True so the bundle has a real schema for the indexes
    # and views below to resolve against.
    full.create_source(CONNECTOR_NAME, source_args)

    full.create_index("project_id", "btree")
    full.create_index("session_id", "btree")
    full.create_index("event_type", "btree")
    full.create_index("timestamp", "btree")
    full.create_index(["search_text"], "text", name="search_text_idx")

    full.create_view(
        "message_events",
        "SELECT * FROM bundle WHERE message_role IS NOT NULL",
    )
    full.create_view(
        "tool_events",
        "SELECT * FROM bundle WHERE tool_names IS NOT NULL OR tool_result_text IS NOT NULL",
    )

    full.commit("Built full Claude transcript history bundle")

    # ----- Phase 2: export EMPTY to strip data, keep structure -----
    #
    # `EXPORT EMPTY` walks the full bundle's history and re-applies only the
    # structural ops (CREATE SOURCE, CREATE INDEX, CREATE VIEW, column ops,
    # always-update / always-delete rules, EXPECTED SCHEMA) to a fresh bundle
    # at `bundle_dir`. ATTACH/DETACH/REPLACE/DELETE/UPDATE are dropped, so
    # the resulting bundle has no rows but knows how to fetch them.
    full.export_empty(str(bundle_dir))
    print(f"Created empty bundle at {bundle_dir}")

    # ----- Phase 3 (optional): package the empty bundle as a tar -----
    if tar_path is not None:
        empty = bb.open(
            str(bundle_dir),
            config={"system": {"allow_external_code": "true"}},
        ).extend()
        # gzip=True writes a .tar.gz so the downloadable artifact is small.
        # Recipients still need to extract before FETCH because dlopen can't
        # read shared libs from inside an archive.
        empty.export_tar(str(tar_path), gzip=True)
        print(f"Packaged empty bundle as tar at {tar_path}")
        print("Recipients of the tar must extract it (`tar -xf …`) before running FETCH.")

    print(f"Default source directory: {source_dir}")
    print("Run `FETCH base ADD` against the empty bundle to pull in transcript data.")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--bundle-dir",
        type=Path,
        default=DEFAULT_BUNDLE_DIR,
        help=f"Output directory bundle path (default: {DEFAULT_BUNDLE_DIR}).",
    )
    parser.add_argument(
        "--tar",
        type=Path,
        nargs="?",
        const=DEFAULT_TAR,
        default=DEFAULT_TAR,
        help=(
            "Also package the bundle as a single .tar archive at this path "
            f"(default: {DEFAULT_TAR}). Pass --no-tar to skip."
        ),
    )
    parser.add_argument(
        "--no-tar",
        dest="tar",
        action="store_const",
        const=None,
        help="Skip the .tar packaging step.",
    )
    parser.add_argument(
        "--source-dir",
        type=Path,
        default=DEFAULT_SOURCE_DIR,
        help=f"Claude transcript directory baked into the source (default: {DEFAULT_SOURCE_DIR}).",
    )
    parser.add_argument(
        "--patterns",
        default=DEFAULT_PATTERNS,
        help=f"Comma-separated glob patterns to include (default: {DEFAULT_PATTERNS}).",
    )
    args = parser.parse_args()
    create_bundle(
        args.bundle_dir.resolve(),
        args.source_dir.expanduser().resolve(),
        args.patterns,
        args.tar.resolve() if args.tar is not None else None,
    )


if __name__ == "__main__":
    main()
