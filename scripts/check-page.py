#!/usr/bin/env python3
"""Check that the browser page still keeps the promises it makes.

The page is the one part of this repository a reader can *run*, so the claims
it makes have to survive an edit that is only meant to change something else.
Three of them are cheap to state and easy to break:

1. **The button always answers.** It is never disabled -- a browser drops a
   click on a disabled button entirely, so a reader who pressed it while the
   page was measuring would get nothing at all. See ADR-0026.
2. **The report is only claimed when it was written.** The POST's status is
   checked, because a static host answers with 405 and `fetch` does not
   throw. See spec 018, R3.
3. **The commit is checked, not trusted.** A host that answers an unknown
   path with its index page would otherwise pass a whole document off as a
   commit. See ADR-0025.

These are checked against the **source**, because the source is what the
build copies into the page: `just web-build` copies `index.html` and
substitutes the commit into a copy of `app.js`, so a check against the
built page would only ever see the build's own output.

Usage:

    python3 scripts/check-page.py
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WEB = ROOT / "crates" / "columnar-web" / "web"
APP = WEB / "app.js"
PAGE = WEB / "index.html"

# A line comment, or a block comment. The checks below are about the code,
# not about the prose around it: this file talks about disabled buttons, and
# a check that could not tell a comment from a statement would be useless.
LINE_COMMENT = re.compile(r"//[^\n]*")
BLOCK_COMMENT = re.compile(r"/\*.*?\*/", re.DOTALL)


def strip_comments(source: str) -> str:
    """Removes the comments, so the checks see the code."""
    return BLOCK_COMMENT.sub("", LINE_COMMENT.sub("", source))


def main() -> int:
    """Checks the page's promises. Returns 0 when they all hold."""
    if not APP.is_file() or not PAGE.is_file():
        print(f"the page's source is missing: {APP} or {PAGE}", file=sys.stderr)
        return 1

    app_source = APP.read_text(encoding="utf-8")
    page_source = PAGE.read_text(encoding="utf-8")
    app_code = strip_comments(app_source)

    failures: list[str] = []
    checks: list[tuple[str, bool, str]] = []

    # 1. The button answers.
    #
    # A disabled button is a button that does not work: the browser drops the
    # click without a sound, and the reader concludes there is no button.
    # Nothing in the page may disable it.
    disabled = re.search(r"\.disabled\b", app_code)
    checks.append(
        (
            "the page never disables the button",
            disabled is None,
            "the page sets `.disabled`, so a press during the run is dropped"
            if disabled
            else "",
        )
    )

    # The label has to say what is true: the run is in flight, a press is
    # waiting, or the last run is over.
    for text, why in (
        ("Running…", "the label while a run is in flight"),
        ("Running… — it will run again", "the label while a press is waiting"),
        ("Run the kernel again", "the label after a run has finished"),
    ):
        checks.append((f"the page says {text!r}", text in app_code, why))

    # The page runs on open, so the first paint must not promise a run that
    # has already started.
    button = re.search(r'<button id="run"[^>]*>([^<]*)</button>', page_source)
    if button is None:
        checks.append(("the page has a run button", False, "no <button id=\"run\">"))
    else:
        checks.append(
            (
                "the button's first text is Running…",
                button.group(1).strip() == "Running…",
                f"the button starts as {button.group(1).strip()!r}",
            )
        )

    # 2. The report is only claimed when it was written.
    checks.append(
        (
            "the page checks the report's status",
            "response.ok" in app_code,
            "the POST's status is not checked, so a 405 would be reported as a success",
        )
    )
    checks.append(
        (
            "the page says when there is no report server",
            "there is no report server at this address" in app_code,
            "the page does not tell the reader that the host took no report",
        )
    )

    # 3. The commit is checked, not trusted.
    #
    # A commit is a short token; a document is not. The host answers every
    # unknown path with its index page, with a 200.
    checks.append(
        (
            "the page checks the commit's length",
            re.search(r"\.length\s*>\s*40", app_code) is not None,
            "a host could pass its index page off as a commit",
        )
    )
    checks.append(
        (
            "the page rejects a document as a commit",
            '.includes("<")' in app_code,
            "a host could pass its index page off as a commit",
        )
    )

    for name, passed, why in checks:
        if passed:
            print(f"ok  {name}")
        else:
            print(f"FAIL {name}: {why}")
            failures.append(name)

    if failures:
        print(f"\n{len(failures)} page check(s) failed", file=sys.stderr)
        return 1
    print(f"\n{len(checks)} page checks ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
