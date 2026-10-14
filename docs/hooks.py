"""MkDocs hooks for Bundlebase docs."""

import os
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

    _write_redirects(config)


# Old page -> new page, both relative to docs_dir. Each old URL gets a small
# HTML page that forwards to the new one, so links and search results that
# point at a renamed or merged page keep working.
_REDIRECTS = {
    "guide/semantic-layer.md": "guide/semantic-model.md",
    "use-cases/data-scientist.md": "use-cases/sharing-datasets.md",
    "use-cases/analyst.md": "use-cases/sharing-datasets.md",
    "use-cases/backend-developer.md": "use-cases/sharing-datasets.md",
    "use-cases/data-engineer.md": "use-cases/scheduled-collection.md",
    "use-cases/etl-developer.md": "use-cases/scheduled-collection.md",
}


def _page_dir(md_path: str) -> str:
    """Directory a page is served from with use_directory_urls (a/b.md -> a/b/)."""
    return md_path[: -len(".md")] + "/"


def _write_redirects(config):
    site_dir = Path(config["site_dir"])
    site_url = config.get("site_url") or ""
    for old, new in _REDIRECTS.items():
        old_dir = _page_dir(old)
        new_dir = _page_dir(new)
        target = os.path.relpath(new_dir, old_dir) + "/"
        canonical = site_url + new_dir if site_url else target
        out = site_dir / old_dir / "index.html"
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(
            "<!doctype html>\n"
            '<html lang="en"><head><meta charset="utf-8">\n'
            f"<title>Moved</title>\n"
            f'<link rel="canonical" href="{canonical}">\n'
            f'<meta http-equiv="refresh" content="0; url={target}">\n'
            f'<meta name="robots" content="noindex">\n'
            f'</head><body>This page moved to <a href="{target}">{target}</a>.</body></html>\n',
            encoding="utf-8",
        )
    print(f"  Generated {len(_REDIRECTS)} redirect pages")
