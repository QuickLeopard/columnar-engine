#!/usr/bin/env python3
"""Build the status dashboard: the project's status as one HTML page.

**What this is.** A page that says where the project stands: the headline
comparison against the incumbents, the shape of the tree, the increments and
what is still open. **It is generated**, and the generator reads the project's
own documents -- `STATUS.md`, the specs, `PERF.md` -- so the page never holds a
figure of its own.

**Why it is generated and not written.** A hand-written status page is a copy,
and a copy drifts: the day the engine's `q6` moves, the page still says the
old number, and nothing in the tree notices. **So the page owns no figures.**
Every number on it is rendered from the document that publishes it, and the
page is rebuilt whenever those documents change.

**And it cannot go stale.** `--check` rebuilds the page and compares it with
the committed one, and the build fails when they differ. That is the whole
mechanism: a change that moves the status and does not rebuild the page is a
change the gate rejects. See ADR-0057.

Usage:

    python3 scripts/build-dashboard.py            # write docs/status.html
    python3 scripts/build-dashboard.py --check    # fail when it is stale
"""

from __future__ import annotations

import hashlib
import html
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUTPUT = ROOT / "docs" / "status.html"


# ---------------------------------------------------------------------------
# The documents
# ---------------------------------------------------------------------------


def read(path: Path) -> str:
    """A document's text."""
    return path.read_text(encoding="utf-8")


def section_of(text: str, heading: str) -> str | None:
    """The text of a section, from its heading to the next one.

    The heading is the prefix of the heading line -- "### 3n" -- and the
    section runs to the next heading **of the same level or above it**, so a
    section may hold the sections under it.
    """
    lines = text.splitlines()
    start = next((index for index, line in enumerate(lines) if line.startswith(heading)), None)
    if start is None:
        return None
    level = len(lines[start]) - len(lines[start].lstrip("#"))
    end = len(lines)
    for index in range(start + 1, len(lines)):
        candidate = lines[index]
        if candidate.startswith("#"):
            candidate_level = len(candidate) - len(candidate.lstrip("#"))
            if candidate_level <= level:
                end = index
                break
    return "\n".join(lines[start:end])


def subsections(section: str) -> list[tuple[str, str]]:
    """A section's subsections: the heading, and the text under it."""
    lines = section.splitlines()
    out: list[tuple[str, str]] = []
    heading = ""
    body: list[str] = []
    for line in lines:
        if line.startswith("### "):
            if heading:
                out.append((heading, "\n".join(body).strip()))
            heading = line
            body = []
        elif heading:
            body.append(line)
    if heading:
        out.append((heading, "\n".join(body).strip()))
    return out


def body_of(section: str) -> str:
    """A section without its own heading.

    The page puts the heading where the card's own title is, so the section
    does not need to repeat it.
    """
    lines = section.splitlines()
    if lines and lines[0].startswith("#"):
        lines = lines[1:]
    return "\n".join(lines).strip()


def defect_count(text: str) -> int:
    """How many defect rows PERF.md's fourth section publishes."""
    section = section_of(text, "## 4.")
    if section is None:
        return 0
    rows = [
        line
        for line in section.splitlines()
        if line.startswith("|") and not re.fullmatch(r"\|[\s\-|]+\|", line)
    ]
    return max(len(rows) - 2, 0)


def count_tree(pattern: str) -> int:
    """How many files in the tree match a glob."""
    return len(sorted(ROOT.glob(pattern)))


def rust_lines() -> int:
    """How many lines of Rust the tree holds."""
    total = 0
    for path in ROOT.glob("crates/**/*.rs"):
        if "target" in path.parts:
            continue
        total += len(path.read_text(encoding="utf-8").splitlines())
    return total


# ---------------------------------------------------------------------------
# Markdown
# ---------------------------------------------------------------------------


def escape(text: str) -> str:
    """The text, safe to put in HTML."""
    return html.escape(text, quote=False)


def inline(text: str, base: str = "../") -> str:
    """The inline markup of one line: links, code, bold, italic.

    The links in the documents are relative to the **repository root**, and the
    page lives in `docs/`, so a relative link is rewritten to reach the same
    file from there. A link to another host is left alone.
    """
    out = escape(text)
    out = re.sub(
        r"\[([^\]]+)\]\(([^)]+)\)",
        lambda match: _link(match.group(1), match.group(2), base),
        out,
    )
    out = re.sub(r"`([^`]+)`", r"<code>\1</code>", out)
    out = re.sub(r"\*\*([^*]+)\*\*", r"<strong>\1</strong>", out)
    out = re.sub(r"(?<![*\w])\*([^*\n]+)\*(?![*\w])", r"<em>\1</em>", out)
    return out


def _link(label: str, target: str, base: str) -> str:
    """One link, with a relative target rewritten for the page's own place."""
    if target.startswith(("http://", "https://", "#", "mailto:")):
        return f'<a href="{target}">{label}</a>'
    return f'<a href="{base}{target}">{label}</a>'


def render_table(rows: list[str]) -> str:
    """One pipe table, as HTML."""
    cells = [[cell.strip() for cell in row.strip().strip("|").split("|")] for row in rows]
    # The second row is the alignment row -- the separators -- and not a row of
    # the table.
    head, _, *body = cells
    out = ["<table>", "<thead><tr>"]
    out.extend(f"<th>{inline(cell)}</th>" for cell in head)
    out.append("</tr></thead><tbody>")
    for row in body:
        out.append("<tr>")
        out.extend(f"<td>{inline(cell)}</td>" for cell in row)
        out.append("</tr>")
    out.append("</tbody></table>")
    return "".join(out)


def markdown(text: str, base: str = "../") -> str:
    """The documents' markdown, as HTML.

    The subset the house's documents use: headings, paragraphs, pipe tables,
    ordered and unordered lists, fenced code, rules, and the inline markup.
    """
    out: list[str] = []
    lines = text.splitlines()
    index = 0
    while index < len(lines):
        line = lines[index]
        stripped = line.strip()

        if not stripped:
            index += 1
            continue

        # A fenced code block.
        if stripped.startswith("```"):
            index += 1
            block: list[str] = []
            while index < len(lines) and not lines[index].strip().startswith("```"):
                block.append(lines[index])
                index += 1
            index += 1
            out.append(f"<pre><code>{escape(chr(10).join(block))}</code></pre>")
            continue

        # A heading.
        heading = re.match(r"^(#{1,6})\s+(.*)$", stripped)
        if heading:
            level = len(heading.group(1))
            out.append(f"<h{level}>{inline(heading.group(2))}</h{level}>")
            index += 1
            continue

        # A rule.
        if re.fullmatch(r"(-{3,}|\*{3,}|_{3,})", stripped):
            out.append("<hr>")
            index += 1
            continue

        # A table: a pipe row, and the alignment row under it.
        if stripped.startswith("|") and index + 1 < len(lines) and re.fullmatch(
            r"\|[\s\-|:]+\|", lines[index + 1].strip()
        ):
            rows = [lines[index], lines[index + 1]]
            index += 2
            while index < len(lines) and lines[index].strip().startswith("|"):
                rows.append(lines[index])
                index += 1
            out.append(render_table(rows))
            continue

        # A list: ordered or unordered, with its continuation lines.
        bullet = re.match(r"^([-*]|\d+\.)\s+(.*)$", stripped)
        if bullet:
            ordered = not bullet.group(1).startswith("-") and not bullet.group(1).startswith("*")
            items: list[str] = []
            while index < len(lines):
                current = lines[index].strip()
                item = re.match(r"^([-*]|\d+\.)\s+(.*)$", current)
                if item:
                    items.append([item.group(2)])
                elif current and items and not current.startswith(("|", "#", "`")):
                    items[-1].append(current)
                elif not current:
                    break
                else:
                    break
                index += 1
            tag = "ol" if ordered else "ul"
            out.append(f"<{tag}>")
            for item in items:
                out.append(f"<li>{inline(' '.join(item))}</li>")
            out.append(f"</{tag}>")
            continue

        # A paragraph: the lines until the next block.
        paragraph = [stripped]
        index += 1
        while index < len(lines):
            following = lines[index].strip()
            if not following:
                break
            if re.match(r"^(#{1,6}\s|[-*]\s|\d+\.\s|```|>|\|)", following):
                break
            paragraph.append(following)
            index += 1
        out.append(f"<p>{inline(' '.join(paragraph))}</p>")
    return "\n".join(out)


# ---------------------------------------------------------------------------
# The project's state
# ---------------------------------------------------------------------------


def the_specs() -> list[dict[str, str]]:
    """Every spec: its number, its title, its status and its dates."""
    out: list[dict[str, str]] = []
    for path in sorted(ROOT.glob("specs/*/spec.md")):
        text = read(path)
        title = next(
            (line[2:].strip() for line in text.splitlines() if line.startswith("# ")),
            path.parent.name,
        )
        number = next(
            (word for word in title.split() if word.isdigit()),
            path.parent.name[:3],
        )
        status = next(
            (
                line.split(":", 1)[1].strip()
                for line in text.splitlines()
                if line.startswith("**Status**")
            ),
            "unknown",
        )
        finished = next(
            (
                line.split(":", 1)[1].strip()
                for line in text.splitlines()
                if line.startswith("**Finished**")
            ),
            "",
        )
        out.append(
            {
                "number": number,
                "title": title.split("—", 1)[-1].strip() if "—" in title else title,
                "status": status,
                "finished": finished,
                "path": str(path.relative_to(ROOT)),
            }
        )
    return out


def the_gates() -> list[tuple[str, str]]:
    """The gates, and what each one is for."""
    return [
        ("the published figures match the evidence", "scripts/check-published-figures.py"),
        ("the documents' links resolve", "scripts/check-links.py"),
        ("the browser page keeps its promises", "scripts/check-page.py"),
        ("the four crates package", "scripts/check-package.sh"),
        ("the status dashboard is up to date", "scripts/build-dashboard.py --check"),
    ]


def the_figures_state() -> tuple[int, bool]:
    """How many published figures agree with the reports, and whether they do.

    The guard is the house's own script and it says so in its own output: a
    line per check that passed. The dashboard counts them, so the badge on the
    page is the guard's own verdict and not a claim about it.
    """
    try:
        result = subprocess.run(
            [sys.executable, str(ROOT / "scripts" / "check-published-figures.py")],
            capture_output=True,
            text=True,
            cwd=ROOT,
            check=False,
        )
    except OSError:
        return (0, False)
    checks = sum(1 for line in result.stdout.splitlines() if line.startswith("ok "))
    return (checks, result.returncode == 0)


# ---------------------------------------------------------------------------
# The page
# ---------------------------------------------------------------------------

CSS = """
:root {
  color-scheme: light dark;
  --bg: #fbfaf8;
  --fg: #1d1b19;
  --muted: #6b6560;
  --line: #e3ded7;
  --card: #ffffff;
  --accent: #7a4b1e;
  --good: #2f6b3a;
  --code-bg: #f2eee8;
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg: #171614;
    --fg: #ece7e0;
    --muted: #9c948a;
    --line: #2f2b26;
    --card: #1e1c19;
    --accent: #d9a066;
    --good: #7fc48d;
    --code-bg: #232019;
  }
}
* { box-sizing: border-box; }
body {
  margin: 0;
  padding: 2.2rem 1.4rem 4rem;
  background: var(--bg);
  color: var(--fg);
  font: 16px/1.62 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
}
main { max-width: 1180px; margin: 0 auto; }
h1 { font-size: 1.9rem; margin: 0 0 .2rem; letter-spacing: -.01em; }
h2 { font-size: 1.15rem; margin: 2.4rem 0 .8rem; letter-spacing: .01em; text-transform: uppercase; color: var(--muted); }
h3 { font-size: 1rem; margin: 1.6rem 0 .5rem; }
p { margin: .6rem 0; }
a { color: var(--accent); text-decoration-thickness: 1px; text-underline-offset: 2px; }
code { background: var(--code-bg); padding: .1em .35em; border-radius: 4px; font-size: .88em; }
pre { background: var(--code-bg); padding: .9rem 1rem; border-radius: 8px; overflow-x: auto; }
pre code { background: none; padding: 0; }
hr { border: 0; border-top: 1px solid var(--line); margin: 2rem 0; }
.tagline { color: var(--muted); margin: 0 0 1.4rem; max-width: 62ch; }
.badges { display: flex; flex-wrap: wrap; gap: .5rem; margin: 0 0 1.6rem; }
.badge {
  border: 1px solid var(--line); background: var(--card); border-radius: 999px;
  padding: .28rem .8rem; font-size: .84rem; color: var(--muted);
}
.badge strong { color: var(--fg); font-weight: 600; }
.badge.good { border-color: var(--good); color: var(--good); }
.grid { display: grid; gap: 1rem; grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)); }
.panel { border: 1px solid var(--line); background: var(--card); border-radius: 10px; padding: .9rem 1.1rem 1rem; margin-bottom: 1rem; }
.panel h3 { margin: 0 0 .5rem; }
details { margin-top: .6rem; }
summary { cursor: pointer; color: var(--muted); font-size: .9rem; }
details[open] summary { margin-bottom: .4rem; }
.card { border: 1px solid var(--line); background: var(--card); border-radius: 10px; padding: 1rem 1.1rem; }
.card h3 { margin: 0 0 .5rem; }
.kv { display: grid; grid-template-columns: 1fr auto; gap: .3rem .8rem; margin: 0; }
.kv dt { color: var(--muted); }
.kv dd { margin: 0; text-align: right; font-variant-numeric: tabular-nums; }
table { border-collapse: collapse; width: 100%; margin: .4rem 0 1rem; font-size: .92rem; }
th, td { border-bottom: 1px solid var(--line); padding: .42rem .6rem; text-align: right; white-space: nowrap; }
td:first-child { white-space: normal; }
th:first-child, td:first-child { text-align: left; }
th { color: var(--muted); font-weight: 600; font-size: .84rem; text-transform: uppercase; letter-spacing: .03em; }
td strong { color: var(--accent); }
.specs { display: grid; gap: .1rem; font-size: .9rem; }
.spec { display: grid; grid-template-columns: 3.4rem 1fr auto; gap: .6rem; padding: .22rem 0; border-bottom: 1px solid var(--line); }
.spec .n { color: var(--muted); font-variant-numeric: tabular-nums; }
.spec .s { color: var(--good); font-size: .82rem; }
.questions { font-size: .95rem; }
.questions li { margin: .35rem 0; }
footer { margin-top: 3rem; padding-top: 1rem; border-top: 1px solid var(--line); color: var(--muted); font-size: .85rem; }
footer code { font-size: .82em; }
"""


def table_and_prose(section_html: str) -> tuple[str, str]:
    """A section's table, and the prose that explains it.

    The table is what a reader comes for and the prose is why, so the table
    stands open and the prose is folded.
    """
    if "</table>" not in section_html:
        return ("", section_html)
    table, _, prose = section_html.partition("</table>")
    return (table + "</table>", prose)


def panel(title: str, section_html: str) -> str:
    """One panel of the headline: the table, and the prose folded under it."""
    table, prose = table_and_prose(section_html)
    folded = ""
    if prose.strip():
        folded = f"<details><summary>what moved, and why</summary>{prose}</details>"
    return f'<div class="panel"><h3>{title}</h3>{table}{folded}</div>'


def card(title: str, body: str) -> str:
    """One card of the grid."""
    return f'<div class="card"><h3>{title}</h3>{body}</div>'


def kv(pairs: list[tuple[str, str]]) -> str:
    """A definition list of facts."""
    rows = "".join(f"<dt>{name}</dt><dd>{value}</dd>" for name, value in pairs)
    return f'<dl class="kv">{rows}</dl>'


def render() -> str:
    """The whole page, as one string."""
    status = read(ROOT / "STATUS.md")
    perf = read(ROOT / "PERF.md")
    specs = the_specs()

    where = section_of(status, "## Where it stands") or ""
    cold = body_of(section_of(where, "### Cold") or "")
    resident = body_of(section_of(where, "### Resident") or "")
    load = body_of(section_of(where, "### The load") or "")
    open_section = section_of(status, "## The open questions") or ""
    parts = subsections(open_section)
    questions = body_of(parts[0][1]) if parts else ""
    # The work the project has not done is a long ledger, and the page keeps
    # it whole but folded: the questions are what a reader comes for.
    ledger = ""
    if len(parts) > 1:
        items = sum(
            1
            for line in parts[1][1].splitlines()
            if line.strip().startswith(("- ", "* "))
        )
        ledger = (
            f"<details><summary>{inline(parts[1][0].lstrip('# '))} "
            f"({items} items)</summary>{markdown(parts[1][1])}</details>"
        )

    checks, agree = the_figures_state()

    # The latest finished spec's date, which is the date the status was last
    # closed. It comes from the document, so the page and the spec cannot
    # disagree about it.
    finished = sorted(spec["finished"] for spec in specs if spec["finished"])
    as_of = finished[-1] if finished else ""

    spec_rows = "".join(
        '<div class="spec"><span class="n">{number}</span>'
        '<span><a href="../{path}">{title}</a></span>'
        '<span class="s">{status}</span></div>'.format(
            number=spec["number"],
            path=spec["path"],
            title=spec["title"],
            status=spec["status"],
        )
        for spec in reversed(specs)
    )

    body = f"""
<h1>Columnar Engine</h1>
<p class="tagline">A vectorised columnar query engine in Rust, with a comparison kernel in C++ behind a
thin C ABI, a Parquet-native embedded store, and a measurement harness willing to say the engine loses.
<b>This page is generated</b> from the project's own documents, and it holds no figure of its own.</p>

<div class="badges">
  <span class="badge"><strong>{len(specs)}</strong> specs</span>
  <span class="badge"><strong>{count_tree("docs/adr/*.md")}</strong> ADRs</span>
  <span class="badge"><strong>{defect_count(perf)}</strong> defects</span>
  <span class="badge"><strong>{count_tree("crates/*/Cargo.toml")}</strong> crates</span>
  <span class="badge"><strong>{rust_lines():,}</strong> lines of Rust</span>
  <span class="badge"><strong>{count_tree("bench-results/*.json")}</strong> committed reports</span>
  <span class="badge">as of <strong>{as_of}</strong></span>
  <span class="badge {'good' if agree else ''}">the figures: <strong>{checks}</strong> checks
    {'agree with the evidence' if agree else 'DO NOT AGREE'}</span>
</div>

<h2>The headline</h2>
{panel("Cold &mdash; the file is read and the query answered in one go", markdown(cold))}
{panel("Resident &mdash; the data is already in memory", markdown(resident))}
{panel("The load &mdash; the file into memory", markdown(load))}

<h2>The tree</h2>
<div class="grid">
  {card(
      "The shape",
      kv(
          [
              ("crates", str(count_tree("crates/*/Cargo.toml"))),
              ("lines of Rust", f"{rust_lines():,}"),
              ("test files", str(count_tree("crates/*/tests/*.rs"))),
              ("probes", str(count_tree("crates/*/examples/*.rs"))),
              ("committed reports", str(count_tree("bench-results/*.json"))),
              ("specs", str(len(specs))),
              ("ADRs", str(count_tree("docs/adr/*.md"))),
              ("defects", str(defect_count(perf))),
          ]
      ),
  )}
  {card(
      "The gates",
      "<ul>"
      + "".join(
          f"<li>{name} &mdash; <code>{command}</code></li>" for name, command in the_gates()
      )
      + "</ul>",
  )}
</div>

<h2>The increments</h2>
<div class="specs">{spec_rows}</div>

<h2>The open questions, and what is not done</h2>
<div class="questions">{markdown(questions)}</div>
{ledger}

<footer>
  Generated by <code>scripts/build-dashboard.py</code> from <code>STATUS.md</code>,
  <code>PERF.md</code> and the specs. <b>Do not edit this file by hand</b> &mdash; the gate
  <code>python3 scripts/build-dashboard.py --check</code> rebuilds it and fails when it differs, so a
  change that moves the status cannot merge with a stale page. Rebuild it with
  <code>just dashboard</code>.
</footer>
"""
    return f"<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n" \
           f"<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n" \
           f"<title>Columnar Engine &mdash; the status dashboard</title>\n<style>{CSS}</style>\n" \
           f"</head>\n<body>\n<main>\n{body}</main>\n</body>\n</html>\n"


# ---------------------------------------------------------------------------
# The act
# ---------------------------------------------------------------------------


def fingerprint(html_text: str) -> str:
    """The page's own fingerprint, over everything but the fingerprint."""
    return hashlib.sha256(html_text.encode("utf-8")).hexdigest()[:12]


def main() -> int:
    """Write the page, or check that the committed one is current."""
    page = render()
    if "--check" in sys.argv:
        if not OUTPUT.exists():
            print(f"FAIL  {OUTPUT.relative_to(ROOT)} does not exist -- run `just dashboard`")
            return 1
        committed = OUTPUT.read_text(encoding="utf-8")
        if committed == page:
            print(f"ok  the status dashboard is up to date ({OUTPUT.relative_to(ROOT)})")
            return 0
        print(
            f"FAIL  {OUTPUT.relative_to(ROOT)} is stale -- the project's status has moved and the "
            f"page has not. Run `just dashboard` and commit the page with the change."
        )
        return 1
    OUTPUT.write_text(page, encoding="utf-8")
    print(f"wrote {OUTPUT.relative_to(ROOT)} ({len(page):,} bytes, fingerprint {fingerprint(page)})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
