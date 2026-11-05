#!/usr/bin/env python3
"""Check that every link in the tree's documents resolves.

A link in a document is a claim about the tree: it says that the thing it
names is there. **A broken link is a wrong claim, and nothing else in this
repository's gates would catch it** -- the figures guard checks numbers, the
page guard checks the browser's promises, and neither reads a sentence's
target. So a link can rot in place for eight increments before anyone looks.

That is what happened: the ADRs' links to their specs, the older specs' links
to each other, and PERF.md's link to STATUS.md all pointed at paths that do
not exist. They are repaired, and this script is what keeps them repaired.

**What is checked.** Every markdown link in the tree whose target is a
**relative path to a file in the repository** -- a link to another document.
Those are the ones this repository owns and this script can decide. A link to
a **URL** is left alone: it is a claim about another host, and a check that
fetched it would be a network call in a gate that is supposed to be fast and
offline. A link to a **heading** (`#section`) is left alone as well, for the
same reason: the anchor is checked by the page that renders it, not here.

**What is NOT checked.** Nothing outside the repository, and nothing that is
not a document. An image, a data file or a script that a document names by
path is not followed -- this script is about the documents' own links, and a
file that is deliberately absent (a report that is written by a probe) would
otherwise be a permanent failure.

Usage:

    python3 scripts/check-links.py
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# The directories that are not the tree's documents: the build output, the
# vendored dependencies and the version control's own directory.
SKIP = {".git", "target", "node_modules", ".venv"}

# A markdown link whose target is a relative path to a document:
#   [the label](the/path.md)  or  [the label](the/path.md#the-heading)
LINK = re.compile(r"\]\(([^)\s]+?)(?:#[^)\s]*)?\)")


def documents() -> list[Path]:
    """Every markdown document in the tree."""
    found = []
    for path in sorted(ROOT.rglob("*.md")):
        if any(part in SKIP for part in path.relative_to(ROOT).parts):
            continue
        found.append(path)
    return found


def is_ours(target: str) -> bool:
    """Whether the target is a path in this repository, and not a URL."""
    if target.startswith(("http://", "https://", "mailto:", "ftp://")):
        return False
    if target.startswith(("/", "~")):
        return False
    if not target.endswith(".md"):
        return False
    return True


def main() -> int:
    documents_found = documents()
    broken: list[tuple[Path, str]] = []

    for path in documents_found:
        base = path.parent
        for match in LINK.finditer(path.read_text(encoding="utf-8", errors="ignore")):
            target = match.group(1)
            if not is_ours(target):
                continue
            resolved = (base / target).resolve()
            if not resolved.exists():
                broken.append((path.relative_to(ROOT), target))

    if broken:
        print("FAIL: the tree's documents have links that do not resolve:", file=sys.stderr)
        for path, target in broken:
            print(f"  {path} -> {target}", file=sys.stderr)
        return 1

    print(f"ok  {len(documents_found)} documents, every link to a document in the tree resolves")
    return 0


if __name__ == "__main__":
    sys.exit(main())
