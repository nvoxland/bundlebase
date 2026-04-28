#!/usr/bin/env python3
"""Wipe and rebuild every example bundle in docs/examples/scripts/.

Each subdirectory holds one example dataset and is expected to ship a
sibling `create_<name>_bundle.py` whose default invocation produces the
bundle artifact(s) in-place. This script:

  1. Discovers every subdirectory of `docs/examples/scripts/` (today
     only `claude_history`; tomorrow more).
  2. Deletes the previous build outputs so the create scripts don't
     bail out on "already exists" guards. Treats any `*-bundle/`
     directory, any `*-bundle.tar.gz` file, and any `.build/`
     intermediate dir as disposable artifacts.
  3. Invokes the create script with the current Python interpreter,
     streaming its output, and reports a per-dataset pass/fail summary
     at the end.

Usage:

    python docs/examples/scripts/rebuild_all.py
    python docs/examples/scripts/rebuild_all.py --only claude_history
    python docs/examples/scripts/rebuild_all.py --skip-clean

Exit code is non-zero if any dataset fails so this is safe to wire into
CI / a pre-release sweep.
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

SCRIPTS_ROOT = Path(__file__).resolve().parent


@dataclass
class Dataset:
    name: str
    dir: Path
    create_script: Path

    @classmethod
    def discover(cls, root: Path) -> list["Dataset"]:
        out: list[Dataset] = []
        for child in sorted(root.iterdir()):
            if not child.is_dir() or child.name.startswith("."):
                continue
            # Convention: each subdir has a `create_<name>_bundle.py`.
            # Allow any `create_*.py` so future datasets can pick a more
            # natural name without us having to teach this script every
            # variation.
            candidates = sorted(child.glob("create_*.py"))
            if not candidates:
                continue
            if len(candidates) > 1:
                raise SystemExit(
                    f"{child}: multiple create_*.py scripts found "
                    f"({', '.join(c.name for c in candidates)}); "
                    "each dataset directory must have exactly one."
                )
            out.append(cls(name=child.name, dir=child, create_script=candidates[0]))
        return out


def clean_dataset(ds: Dataset) -> list[Path]:
    """Delete previous build outputs for this dataset. Returns removed paths."""
    removed: list[Path] = []
    for child in ds.dir.iterdir():
        # Disposable: anything matching the build-output conventions.
        # Source files (the create script, connector source, helper
        # scripts, fixtures) live alongside but never match these.
        is_artifact = (
            (child.is_dir() and child.name.endswith("-bundle"))
            or child.name == ".build"
            or (child.is_file() and child.name.endswith("-bundle.tar"))
            or (child.is_file() and child.name.endswith("-bundle.tar.gz"))
        )
        if not is_artifact:
            continue
        if child.is_dir():
            shutil.rmtree(child)
        else:
            child.unlink()
        removed.append(child)
    return removed


def rebuild(ds: Dataset, *, clean: bool) -> bool:
    print(f"\n=== {ds.name} ===")
    if clean:
        removed = clean_dataset(ds)
        if removed:
            print(f"  cleaned: {', '.join(p.name for p in removed)}")
        else:
            print("  cleaned: (nothing to remove)")
    print(f"  running: {ds.create_script.relative_to(SCRIPTS_ROOT)}")
    proc = subprocess.run(
        [sys.executable, str(ds.create_script)],
        cwd=ds.dir,
    )
    return proc.returncode == 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--only",
        action="append",
        metavar="NAME",
        help="Restrict to the named dataset(s); repeat for more than one.",
    )
    parser.add_argument(
        "--skip-clean",
        action="store_true",
        help="Don't delete previous build outputs before invoking the create script.",
    )
    args = parser.parse_args()

    datasets = Dataset.discover(SCRIPTS_ROOT)
    if not datasets:
        print(f"No datasets found under {SCRIPTS_ROOT}")
        return 0
    if args.only:
        wanted = set(args.only)
        unknown = wanted - {d.name for d in datasets}
        if unknown:
            print(
                f"Unknown dataset name(s): {', '.join(sorted(unknown))}. "
                f"Available: {', '.join(d.name for d in datasets)}"
            )
            return 2
        datasets = [d for d in datasets if d.name in wanted]

    failed: list[str] = []
    for ds in datasets:
        if not rebuild(ds, clean=not args.skip_clean):
            failed.append(ds.name)

    print("\n=== summary ===")
    for ds in datasets:
        status = "FAILED" if ds.name in failed else "ok"
        print(f"  {ds.name}: {status}")

    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
