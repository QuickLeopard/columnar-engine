# Spec 017 — The repository goes public

**Status**: in progress
**Started**: 2026-10-27
**ADR**: [ADR-0024](../../docs/adr/0024-the-repository-goes-public.md)
**Predecessor**: [016-the-per-row-group-cost](../016-the-per-row-group-cost/spec.md)

## The problem

This repository has three files in it that have nothing to do with it, and it has never been
public:

- `PORTFOLIO_PROJECTS_ANALYSIS.md` — a career-planning document about **other** projects: their
  goals, their risks, their estimated line counts, and which of them would survive a job
  interview.
- `PORTFOLIO_PROJECTS_DEEP_REVIEW.md` — the deep review of the same.
- `Portfolio_Projects_Blueprint.pdf` — 2.4 MB of the same, as a PDF.

They have been tracked since the **first commit** (`19e061e`), so removing them from the tree
would not remove them from the repository: a clone would still carry them in its history, and a
public repository would hand them to anyone who asks for the history.

The repository is also **private**, and that has a cost that is not about privacy at all:

> **Every GitHub Actions job fails before a runner starts.** The account's spending limit is
> exhausted, and a private repository's minutes come out of that limit. A public repository's
> runners are free.

So the two workflows that were written for this purpose — `ci.yml` and `bench-avx2.yml` — cannot
run, and the project has been carrying its own gate by hand. And the AVX2 half of the kernel,
which is the whole point of a dual-architecture project, has never been **executed** on native
x86-64 hardware: `scripts/verify-avx2.sh` runs it under Rosetta 2 on the author's ARM64 Mac, which
proves the code is correct and says nothing about its speed.

`PERF.md` §5 has carried the row *"The x86-64 half of the SIMD story"* as **not measured** since
spec 002, with the reason: *"there is no x86-64 hardware"*. The reason is not the hardware. The
reason is that nobody can run a job for free.

## The requirements

### R1 — The three files are out of the repository, and out of its history

- [x] The three files are removed from the working tree.
- [x] They are removed from **every commit**, not only from the tip: the history is rewritten and
      the rewritten history is force-pushed.
- [x] The rewrite is verified: after it, `git log --all --diff-filter=A` names no commit that
      introduced any of the three, and the object store holds no blob whose content matches them.
- [x] **A copy is kept outside the repository**, so nothing is lost. It is a career document and
      it is the author's; it does not belong in an engine's tree, and that is a different thing
      from it not existing.

### R2 — The repository carries a licence and a contribution guide

- [x] `LICENSE` — **Apache-2.0**, the licence every dependency in the lockfile is compatible with.
      The C++ baseline in `crates/columnar-kernels/src/` is this repository's own work and carries
      the same licence; there is no vendored third-party source in the tree.
- [x] `CONTRIBUTING.md` — the method, the gate, and the rules that are not negotiable, written for
      a stranger rather than for the author.

### R3 — The evidence chain stays whole

- [x] **Every committed report names a commit that still resolves.** A rewrite changes every hash,
      and 34 committed reports name the commit they were taken at. A reader who cannot check that
      commit out cannot reproduce the number, which is exactly the failure Principle V exists to
      prevent.
- [x] Therefore the reports that name a commit are **re-measured** after the rewrite: the three
      materialised-column campaigns, the three RLE-expansion campaigns, the three batch-limit
      campaigns, the nine per-row-group-cost campaigns, the nine row-group campaigns, and the three
      browser campaigns.
- [x] The published tables are **re-derived** from the new reports by `scripts/medians.py`, and
      `scripts/check-published-figures.py` guards them again. The figures move; the derivation does
      not.

### R4 — The repository is public, and the CI is the gate

- [x] The repository's visibility is **public**.
- [x] `ci.yml` runs on every push and every pull request: format, lint, test, differential,
      cross-check, wasm-check, dependency-policy, no-generated-data, published-figures.
- [x] The **absolute** performance budgets stay off the shared runners. The constitution requires
      pinned runners for the budget gate; a GitHub-hosted runner is a shared machine, so the
      budget gate does not run there and the repository says so rather than publishing a budget
      measured on a neighbour's noise.

### R5 — The x86-64 measurement is published as its own set

- [x] The kernel A/B runs on a native x86-64 runner, and the **ratios** are published.
- [x] The absolute figures from that runner are **not** published, and the published ratio is
      never compared with an ARM64 figure: a different machine, a different clock and a different
      compiler make the ratio between them a claim about two engines rather than about one kernel.
- [x] The runner's machine, its SIMD path, its thread count and its warm-up are published with the
      ratio, because a number without its conditions is not a number.
- [x] It is the **median of three runs** with the raw reports committed and the ratios derived by a
      checked-in script, like every other table in the file.

## What was measured

- **Nothing about the engine changed.** This spec is about the repository, not the code: no
  behaviour, no number, no plan, no output. The engine's tree is byte-identical before and after,
  which is checked by the differential suite and by the published figures agreeing with their
  reports.
- **The three files carried no secrets.** No credentials, no keys, no personal paths; they are a
  written document about planned projects. The risk of publishing them was not a leak, it was
  publishing unreleased plans and career positioning in an engine's repository.
- **The rewrite is 26 commits on one branch**, with no tags, no other remotes and no other clone,
  so a force-push costs nothing that is not already paid.
- **The x86-64 ratios, once they land, are the project's first native AVX2 numbers.** The
  correctness half was already checked, under Rosetta 2, on the author's own machine; the workflow
  checks it again on native hardware, and measures it there for the first time.

## The non-goals

- **The absolute x86-64 figures are not published.** They describe a shared runner, and the
  repository does not publish a number it cannot reproduce. The `avx2-self-hosted` job in
  `bench-avx2.yml` is already wired for the day a pinned x86-64 runner exists, and it is the only
  job whose absolute figures would be worth publishing.
- **The engine's ARM64 figures do not change.** They were measured on the M4 Max and they are
  published with the M4 Max. The x86-64 set is a second set, on a second machine, and the two are
  never compared.
- **The portfolio document is not turned into a product.** It leaves the repository; it does not
  become a README section, a site, or a set of slides. That is a different piece of work with a
  different audience.
- **The repository is not renamed, restructured, or re-launched.** It is the same repository, the
  same history's content, with three files removed and a licence added.
- **The comparison against DuckDB, Polars and DataFusion is not re-run on the runner.** The
  incumbents' figures are published from the machine the engine's are published from; adding a
  second machine's incumbents would double the tables without answering a question anybody asked.

## References

| requirement | principle | why |
|---|---|---|
| R1 | **V** — the comparison is reproducible by a stranger | a repository that carries an unrelated document about unreleased plans is not a repository a stranger can read |
| R2 | **V** | a stranger needs to know what they may do with the code, and how to contribute |
| R3 | **IV** — evidence over claims | a published number whose commit cannot be resolved is a claim, not evidence |
| R4 | **V** | the gate is `just check` and CI; CI that cannot run is not a gate |
| R5 | **IV**, **V** | the x86-64 numbers carry their machine, their SIMD path and their conditions, and are derived by a checked-in script from committed reports |

See [ADR-0024](../../docs/adr/0024-the-repository-goes-public.md) and
[`PERF.md`](../PERF.md).
