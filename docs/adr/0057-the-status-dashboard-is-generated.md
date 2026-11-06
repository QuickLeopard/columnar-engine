# ADR-0057: The status dashboard is generated

**Status**: accepted
**Date**: 2026-11-06
**Reviewers**: TBD

## Context

The project's status lives in [STATUS.md](../../STATUS.md): the three comparison tables against the
incumbents, the shape of the tree, the increments, and — as the last section, which is the house's
rule — the open questions. It is the house's single account of the state of the work, and forty-three
increments have kept it true.

**And it is a wall of prose.** The comparisons are three tables in the middle of the file, the counts
are in a sentence near the top, and the questions are under two hundred lines of dense argument. A
reader who wants to know where the project stands has to know the document's shape first.

So there was a page to write. **And the page was where the project's own principles were at risk.**

This repository holds one rule above the rest of its habits: **a number is a claim about a
measurement, and it is owned by the document that publishes it.**
`scripts/check-published-figures.py` fails the build when a quoted figure stops agreeing with the
committed reports — that is the guard that makes a published number checkable, and it has caught real
drift more than once.

**A hand-written status page breaks that rule silently.** The day the engine's `q6` moves by a
millisecond, the page still says the old number, and nothing in the tree notices: the number on the
page has no owner, no evidence behind it, and no gate. The house's own record says what that is worth
— spec 043's probe published a wrong "first differing byte" for one campaign, and the wrong number
survived until a second campaign disagreed with it.

## Decision

1. **The page is generated.** `scripts/build-dashboard.py` reads `STATUS.md`, `PERF.md` and the specs,
   and writes `docs/status.html`. **It holds no figure of its own**: every number on it is rendered
   from the document that publishes it. The page is an artefact and not a source, and it says so at
   the foot.

2. **The gate is what keeps it true.** The same script with `--check` rebuilds the page and compares
   it with the committed one, and **fails when they differ**. It runs in `just check`, in the CI and
   in `CONTRIBUTING.md`'s table of gates. **A change that moves the status and does not rebuild the
   page does not merge** — that is the whole mechanism, and it is the same shape as the figures guard:
   the artefact is derived, the derivation is checked, and the check is a build failure.

3. **The page is a dashboard and not a copy of the document.** The three comparisons stand open with
   the prose that explains what moved folded under each; the open questions stand open and the ledger
   of the work not done is folded; the tree's shape, the gates and the increments are compact. **What
   a reader comes for is at the top.**

4. **It is self-contained.** One file, no external asset, no script, no network. It opens from the
   repository, from a `file://` URL and from a static host, and it works with JavaScript turned off —
   which is what makes it a page and not an application.

5. **The browser page is not touched.** `crates/columnar-web/web/index.html` is the one page in this
   repository a reader can *run*, and it has its own promises and its own gate (ADR-0026, ADR-0025).
   **The dashboard renders the documents; it does not replace them, and it does not replace that
   page.**

## Consequences

**What becomes easier.** A reader can see where the project stands in one screen — the headline, the
tree's shape, the gates, the increments, the open questions — without knowing the shape of a Markdown
document, and without a stale number on it.

**What becomes harder.** **Every change that moves the status has to rebuild the page**, and the gate
says so. That is a cost, and it is the point: the cost of a stale page is a reader's trust, and a
trust once spent on a wrong number is not spent twice.

**What we gave up.** A hand-written page could have said things the documents do not. It could also
have said them wrong, for as long as nobody checked, and nothing in the tree would have said so. **The
page says what the documents say** — and that is the whole claim.

## Confidence

**high** — the page is a pure function of the tree, the check is a byte comparison, and the gate runs
in CI and locally. The one thing that could go wrong is the page claiming something the documents do
not, and that is prevented by construction: the generator reads the documents and the check compares
the page with what the generator produces.
