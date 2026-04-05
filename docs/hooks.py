"""MkDocs hooks for Bundlebase docs."""

import re
from pathlib import Path

# Directories to exclude from llms-full.txt
_EXCLUDE_DIRS = {"blog", "assets", "overrides", "stylesheets"}
# Files to exclude
_EXCLUDE_FILES = {"llms.txt", "hooks.py"}


def _strip_frontmatter(content: str) -> str:
    return re.sub(r"^---\s*\n.*?\n---\s*\n", "", content, flags=re.DOTALL).strip()


def on_post_build(config, **kwargs):
    """Generate llms-full.txt — all docs concatenated as clean markdown."""
    docs_dir = Path(config["docs_dir"])
    site_dir = Path(config["site_dir"])
    site_url = config.get("site_url", "https://nvoxland.github.io/bundlebase/")

    parts = [
        f"# Bundlebase — Complete Documentation\n\n"
        f"Source: {site_url}\n"
        f"Quick reference: {site_url}llms.txt\n\n"
        f"This file contains the full Bundlebase documentation concatenated for LLM context.\n"
    ]

    # Collect markdown files, excluding noise dirs
    md_files = []
    for md_file in sorted(docs_dir.rglob("*.md")):
        rel = md_file.relative_to(docs_dir)
        parts_path = rel.parts
        if any(p in _EXCLUDE_DIRS for p in parts_path):
            continue
        if md_file.name in _EXCLUDE_FILES:
            continue
        md_files.append((rel, md_file))

    for rel, md_file in md_files:
        content = _strip_frontmatter(md_file.read_text(encoding="utf-8"))
        if not content:
            continue
        parts.append(f"\n\n{'=' * 60}\n\n{content}")

    output = "\n".join(parts)
    output_path = site_dir / "llms-full.txt"
    output_path.write_text(output, encoding="utf-8")
    size_kb = output_path.stat().st_size // 1024
    print(f"  Generated llms-full.txt ({size_kb} KB, {len(md_files)} pages)")
