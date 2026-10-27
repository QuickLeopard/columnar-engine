# ADR-0025: The page on a static host

**Status**: accepted
**Date**: 2026-10-27
**Reviewers**: TBD

## Context

Spec 013 built the browser page as the hook for a reader who has a link rather than a terminal.
It is the one part of this repository a reader can *run*: it loads the engine's own WASM module,
runs the kernel over 20 million rows, compares the `simd128` mask with the oracle's bit for bit,
and prints nothing at all if they differ.

It also needed a server. `just web` starts `scripts/web-server.py`, which serves `target/web` and
accepts the page's `POST /report`. Two of the page's behaviours depend on that server:

- the **commit** it displays, which the server reads from `git` at the moment the page asks;
- the **report** it posts back, which the server writes to `target/web/browser-report.json`.

Spec 017 made the repository public, and the obvious next step — a link a reader can open — means
publishing the page to a static host. A static host runs no Python, accepts no `POST`, and serves
files.

That makes the page's two server-dependent behaviours wrong in a way that only shows up after
publication. A static host answers `GET /commit` with 404 (the page already degrades to
`unavailable`), and answers `POST /report` with 404 — and `fetch` does **not** throw on a 404. The
page's success branch would therefore print *"done · the report was written to
target/web/browser-report.json"* on a host that received nothing, naming a path that does not
exist on the reader's machine.

## Decision

**The page must be correct with no server at all, and it must never claim a report it did not
get.**

1. **The report POST's status is checked, not only its lack of an exception.** The page reports
   what actually happened: on the local server, the report was written; on a static host, there is
   no report server here and the numbers above are this browser's.
2. **The commit travels baked into the built page, and the server's answer is checked.** The build
   writes the tree's commit into the built copy of the script. The page still asks the server
   first — the live commit is better than a build-time one — but it **checks** the answer, because
   the first deployment taught us what a static host does with an unknown path: it returns its
   index page, with a `200`. A commit is a short token; a document is not. The page falls back to
   the commit it was built from when the answer is not a commit, so no host can pass a page off as
   one.
3. **The local server's behaviour is unchanged.** It intercepts `/commit` before the file handler,
   so the live commit still wins locally, with `-dirty` when the tree is not clean. The built page
   is a fallback for hosts with no server, not a replacement for the server.
4. **The deployed site is `target/web` as the build leaves it.** No hand-edited copy, no second
   build, no separate "public" copy of the page that can drift from the one the repository builds.

## Consequences

**Easier.** The page is now publishable: one build, one directory, one command. A reader with a
link opens the page and watches the check run, with no toolchain and no clone. The commit the page
names is the commit it was built from, so the published page is attributable the same way every
published figure is.

**Harder.** The page now has two hosts to be correct on, and the difference between them is a
behaviour the page has to state rather than assume. The build has one more thing to write, and the
build's output has one more file in it.

**Given up.** The deployed page cannot receive reports, so a reader's measurement is not collected
— it is visible on the reader's screen and nowhere else. The published evidence stays in
`bench-results/`, measured on the development machine where the host's load average is a condition
of the measurement. That is a deliberate loss: a static host has nowhere to put a report, and a
measurement collected from an unknown machine with an unknown load is not a published figure.

## Measurement

The page's own output is the evidence: it prints its numbers only after the mask check and the
edge check pass, and the published set in `PERF.md` §1a is the median of three campaigns of that
same page — 12.59× the oracle over the three campaigns, 12,726,115 matches, masks identical bit
for bit in every one.

For the change itself, the evidence is that the page states the truth on both hosts: on the
static host it says there is no report server, and on the local server it says the report was
written, and the report is in `target/web/browser-report.json`.

**The first deployment is the receipt for the second half of decision 2.** The page was published,
and the deployed page's **engine** row read `commit <!doctype html> <html lang="en"> …` — the
host's own index page, returned with a `200` for the path `/commit`, because the host answers
*every* unknown path with its index page. The page had trusted a `200`. It now checks the answer,
and the commit it shows is the one it was built from. A host that answers an unknown path with a
document is not a host that answers an unknown path with a `404`, and the page has to survive
both.

## Confidence

**high** — the change is small, and both hosts are checked directly: the static host by opening
the deployed URL in a browser and reading what the page says, and the local host by running the
campaigns the published §1a figures come from.
