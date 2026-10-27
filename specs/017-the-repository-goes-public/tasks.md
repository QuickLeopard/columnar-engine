# Spec 017 — tasks

The order matters: the history is rewritten **before** anything is measured, so that the reports
name a commit that still resolves.

## R1 — The three files are out, tree and history

- [x] Copy the three files outside the repository, and check the copies byte for byte.
- [x] `git rm --cached` the three files; they leave the tree.
- [x] Rewrite the history with `git filter-branch --index-filter`, dropping the three paths from
      **every** commit; prune the empty ones; expire the reflog and garbage-collect, so the blobs
      are gone from the object store and not merely unreferenced.
- [x] Verify: `git log --all --diff-filter=A` names no commit that introduced any of the three, and
      no blob in the object store matches their content.
- [x] Force-push the rewritten history to `origin/main` with `--force-with-lease`.

## R2 — The licence and the contribution guide

- [x] `LICENSE`: Apache-2.0, with the appendix and the copyright line.
- [x] `CONTRIBUTING.md`: the method, the gate, the rules, and the two checks that rot silently
      (`just check-x86`, `just check-wasm`).

## R3 — The evidence chain stays whole

- [x] Re-measure the reports that name a commit, after the rewrite:
  - [x] `materialised-run{1,2,3}.json` — the materialised column, three columns, three campaigns
  - [x] `rle-expansion-run{1,2,3}.json` — the RLE expansion, three campaigns
  - [x] `batch-size-run{1,2,3}.json` — the reader's batch limit over both shapes
  - [x] `row-group-cost-{6,12,24}-run{1,2,3}.json` — the per-row-group cost, nine campaigns
  - [x] `row-groups-{6,12,24}-run{1,2,3}.json` — the row-group probe, nine campaigns
  - [x] `kernel-run{1..6}.json` — the kernel A/B, six campaigns of eight iterations
  - [x] `browser-run{1,2,3}.json` — the browser's kernel, three campaigns, in the in-app browser
- [x] Re-measure **the whole published set** as one campaign — the comparison, every sweep, every
      probe — so that the file's figures are one set with one set of conditions rather than
      several, taken on different days from a differently-loaded host.
- [x] Re-derive the tables with `python3 scripts/medians.py`, and republish the figures that
      moved, old and new, with the reason.
- [x] `python3 scripts/check-published-figures.py` guards them again.

## R4 — The repository is public, and CI is the gate

- [x] Flip the visibility to **public** with the API.
- [x] Verify with the API that the repository is public and that the workflows are listed.
- [x] Confirm the CI runs: format, lint, test, differential, cross-check, wasm-check,
      dependency-policy, no-generated-data, published-figures.
- [x] Confirm the **absolute** performance budgets stay off the shared runners, and that the
      repository says why.

## R5 — The x86-64 measurement is published as its own set

- [x] Dispatch `bench-avx2.yml` — twenty-one runs, so that the fleet's processors each have at
      least three campaigns of their own.
- [x] Fetch every run's report and commit it, named by its workflow run id, so a figure can be
      traced to the run that produced it.
- [x] Derive the ratios with a checked-in script, and guard them.
- [x] **Group the reports by the processor that produced them, and publish a processor only if it
      ran at least three campaigns.** The ratio turned out to be a property of the CPU as much as
      of the kernel — 1.69× on an EPYC 9V45, 2.23× on an EPYC 7763, 2.56× on an EPYC 9V74 — so a
      pooled median across the fleet would have been a number about no machine at all.
- [x] Publish the ratios — and only the ratios — with the processor's name, its SIMD path, its
      thread count and its spread attached, and never compared with an ARM64 figure.
- [x] Name the three processors that ran once and are therefore **not** published, and say why.
- [x] Update `PERF.md` §5 and the README's "what is measured" rows: the x86-64 measurement is no
      longer "not measured".

## Out of scope

- The absolute x86-64 figures — they describe a shared runner.
- The engine's ARM64 figures — they were measured on the M4 Max and they stay there.
- The comparison against DuckDB, Polars and DataFusion on the runner — a second machine's
  incumbents would double the tables and answer nobody's question.
- The portfolio document as a product. It leaves the repository; it does not become a README
  section, a site, or a set of slides.
