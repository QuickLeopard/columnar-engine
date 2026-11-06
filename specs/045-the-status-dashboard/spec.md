# Spec 045 — The status dashboard

**Status**: planned
**Started**: 2026-11-06
**Predecessor**: [spec 044 — the rows as they lie](../044-the-rows-as-they-lie/spec.md)

## The problem

**The project's status is a document, and a document is not a dashboard.**

[STATUS.md](../../STATUS.md) holds where the work stands: the three comparison tables against the
incumbents, the shape of the tree, the increments, and the open questions. It is the house's single
account of the state of the work, and its last section is the open questions — which is the house's
rule, and the right one.

**And it is a wall of prose.** The comparisons are three tables in the middle of the file, the counts
are in a sentence near the top, and the questions are under two hundred lines of dense argument. A
reader who wants to know where the project stands has to know the document's shape first.

### The failure this increment refuses

**A hand-written page is a copy, and a copy drifts.** The day the engine's `q6` moves by a
millisecond, the page still says the old number, and **nothing in the tree notices** — because the
number on the page has no owner and no evidence behind it, and every other figure in this repository is
owned by the document that publishes it.

That is the failure the house's own guard was built against: `scripts/check-published-figures.py`
fails the build when a quoted figure stops agreeing with the committed reports. **A page without that
guard is a figure nobody can check.**

## The requirements

**R1. The page holds no figure of its own.** Every number on it is rendered from the document that
publishes it: the comparisons from `STATUS.md`, the defect count from `PERF.md`, the counts from the
tree, the increments from the specs. **One owner per figure**, the rule the guard already enforces
for the prose.

**R2. It is generated, and the generator is in the repository.** `scripts/build-dashboard.py` reads the
documents and writes `docs/status.html`. The page is an artefact and not a source: it is not edited by
hand, and it says so at the foot.

**R3. It cannot go stale.** The same script with `--check` rebuilds the page and compares it with the
committed one, and **fails when they differ**. The check is in `just check`, in the CI and in
`CONTRIBUTING.md`'s table of gates. **A change that moves the status and does not rebuild the page
does not merge.**

**R4. It is a dashboard and not a copy of the document.** The three comparisons stand open, with the
prose that explains what moved folded under each; the open questions stand open and the ledger of the
work not done is folded; the tree's shape, the gates and the increments are compact. **What a reader
comes for is at the top**, and everything else is one click away.

**R5. It is self-contained.** One file, no external asset, no script, no network: it opens from the
repository, from a `file://` URL and from a static host, and it works with JavaScript turned off.

## The measurement

**None, and that is the point.** The page publishes no figure of its own, so there is nothing new to
measure: the figures on it are the house's, and the guard that checks them is the house's. **The badge
on the page that says the figures agree with the evidence is the guard's own verdict**, counted from
the guard's own output — not a claim about it.

## The scope

- **The generator**: `scripts/build-dashboard.py`, and the page it writes, `docs/status.html`.
- **The gate**: the script's `--check`, the `just dashboard` and `just check-dashboard` recipes, the CI
  job, and the line in `CONTRIBUTING.md`.
- **Not in scope**: the browser benchmark page (`crates/columnar-web/web/index.html`), which is the
  one page in this repository a reader can *run*; and `STATUS.md` itself, which stays the house's
  document. **The dashboard renders them; it does not replace them.**

## The references

- [ADR-0057](../../docs/adr/0057-the-status-dashboard-is-generated.md) — the decision: the page is
  generated, and the gate is what keeps that true.
- [STATUS.md](../../STATUS.md) — the document the page renders, and the house's single account of the
  state of the work.
- [constitution.md](../../constitution.md), principle **IV — Evidence Over Claims**: a number is a
  claim about a measurement, and a claim without its evidence is not published.
- [constitution.md](../../constitution.md), principle **V — The Comparison Is Reproducible by a
  Stranger**: a stranger clones the repository, runs the gate, and gets the same verdict.
