# `bench-results/` — the raw evidence

The numbers in [`PERF.md`](../PERF.md) are derived from the reports in this
directory, not typed into it by hand. The derivation is one command:

```bash
python3 scripts/medians.py
```

## The files

| file | what it is |
|---|---|
| `kernel.json` | the comparison kernel's A/B: four flavours of one predicate over 20 million rows |
| `compare.json` | the **representative** comparison run — the run whose figures sit nearest the medians |
| `compare-run1.json` | the first of the three runs the published table is the median of |
| `compare-run2.json` | the second |
| `compare-run3.json` | the third, and the one `compare.json` is a copy of |
| `scaling-run1.json` | the first of the three **thread-count sweeps** of `q6` the §2e table is the median of |
| `scaling-run2.json` | the second |
| `scaling-run3.json` | the third, and the one `scaling.json` is a copy of |
| `scaling-revenue-run1.json` | the first of the three **thread-count sweeps of the real Q6** the §2e table is the median of |
| `scaling-revenue-run2.json` | the second, and the one `scaling-revenue.json` is a copy of |
| `scaling-revenue-run3.json` | the third |
| `scaling-resident-run1.json` | the first of the three **resident sweeps of `q6`** the §2f table is the median of |
| `scaling-resident-run2.json` | the second |
| `scaling-resident-run3.json` | the third |
| `scaling-resident-revenue-run1.json` | the first of the three **resident sweeps of the real Q6** the §2f table is the median of |
| `scaling-resident-revenue-run2.json` | the second |
| `scaling-resident-revenue-run3.json` | the third |
| `scaling-load-run1.json` | the first of the three **load sweeps** the §2g table is the median of |
| `scaling-load-run2.json` | the second |
| `scaling-load-run3.json` | the third |
| `compare.json` | a copy of `compare-run3.json`, the run whose figures sit nearest the medians |
| `scaling.json` | a copy of `scaling-run3.json`, the sweep run nearest the medians |
| `scaling-revenue.json` | a copy of `scaling-revenue-run2.json`, the sweep run nearest the medians |
| `browser-run1.json` | the first of the three **browser campaigns** the §1a table is the median of |
| `browser-run2.json` | the second |
| `browser-run3.json` | the third |
| `browser.json` | a copy of `browser-run3.json`, the campaign whose figures sit nearest the medians |
| `rle-expansion-run1.json` | the first of the three **RLE expansion campaigns** the §3g table is the median of |
| `rle-expansion-run2.json` | the second |
| `rle-expansion-run3.json` | the third |
| `materialised-run1.json` | the first of the three **materialised column campaigns** the §3h table is the median of |
| `materialised-run2.json` | the second |
| `materialised-run3.json` | the third |
| `curve-run1.json` | the first of the three **size-curve campaigns** the §3i table is the median of |
| `curve-run2.json` | the second |
| `curve-run3.json` | the third |
| `experiment-dictionary-copy.json` | **not a normal run.** A measurement taken with a deliberate perturbation of the decode path — see below |

## The experiment file

`experiment-dictionary-copy.json` was taken with the old per-batch dictionary copy
deliberately put back into `columnar-engine/src/dataset.rs`, so that the cost of that copy could
be attributed rather than argued about. It shows the eager path's cold `q6` at 1889.9 ms against
1181.1 ms for the committed code, and the lazy path unmoved.

It is committed because it is the evidence for a published claim. It is **not** one of the three
reports the published table is derived from, and `scripts/medians.py` does not read it — the file
name does not match `compare-run*.json`, deliberately, so it cannot be swept into a median by
accident.

The working tree is the unperturbed code. The perturbation was applied, measured, and reverted;
`git diff` after the revert was empty.

## The published tables

There is **one headline table** and **three sweeps**, and they answer different questions.

| table | question | reports |
|---|---|---|
| §2a, §2b | *which is faster when every system is given one thread* | `compare-run*.json` |
| §2e, `q6` | *how does each system's number move with the thread count* | `scaling-run*.json` |
| §2e, `q6-revenue` | the same, on the real Q6 | `scaling-revenue-run*.json` |
| §2f, `q6` | the same, with the data already in memory | `scaling-resident-run*.json` |
| §2f, `q6-revenue` | the same, on the real Q6 | `scaling-resident-revenue-run*.json` |
| §2g | *how long does each system take to get the file into memory* | `scaling-load-run*.json` |
| §1a | *what does the kernel do in a browser* | `browser-run*.json` |
| §3g | *what does the expansion of the RLE indices cost* | `rle-expansion-run*.json` |

The first is the like-for-like **single-threaded** comparison, and it has been the headline since
the first set of numbers in this repository. The sweeps exist because a one-thread table on its
own is a misleading picture of what a user sees: `DuckDB` uses every core by default. The **load**
sweep is the odd one out: it is the only measurement here that is about the *file* rather than the
*query*.

**All of them are comparisons, not measurements.** At every point of a sweep the engine and both
incumbents run at the *same* thread count, on the same file, in the same harness, and the answers
are cross-checked at every point — except in the load sweep, which times the load and so has no
answer to check. The load sweep's incumbents are still measured in the harness's resident mode,
because that is the mode that times a load separately.

The sweep's report records the thread count that was **asked for** and the workers that
**actually ran**, side by side, because the engine's read splits the file's *row groups* across
the workers: this file has six, so six is the ceiling however many threads are requested.

## Why three runs and a median

This machine is shared. A single run of the comparison is an estimate of a
number that moves when the machine is busy, so the published table is **the
median of three full runs**, and each run is itself the median of five timed
iterations.

All three reports are committed, so the published table is re-derivable
without re-running anything:

```bash
python3 scripts/medians.py bench-results/compare-run1.json \
                            bench-results/compare-run2.json \
                            bench-results/compare-run3.json
```

The sweep's table is derived the same way, and `scripts/medians.py` picks the
`scaling-run*.json` files up automatically when they are in this directory.

The script prints each figure as `median (min-max)`. The spread is part of the
evidence: `PERF.md` quotes the median and states the spread where the spread
matters.

`compare.json` is a copy of the run nearest the medians — it is the report a
reader would see if they opened one — but it is **not** the source of the
published table. The three `compare-run*.json` files are.

## Reproducing

```bash
just dataset     # 6,000,000 rows, deterministic, about 2.8 seconds
just compare     # writes bench-results/compare.json
just sweep       # writes bench-results/scaling.json
just web         # serves the page; the page posts its measurement back
just web-report  # copies that measurement into bench-results/browser.json
```

The RLE expansion's reports are written by the probe itself:

```bash
cargo run --release -p columnar-engine --example rle_expansion_probe -- \
    data/lineitem.parquet --out bench-results/rle-expansion-run1.json
```

`just compare` needs `DuckDB` on `PATH` or in `tools/bin/`, and the `polars`
Python package installed. Both versions are recorded in the report, in
`machine` and in each comparator's `version` field.

The browser's reports are written by the **page**, not by hand: the page posts
its result back to `scripts/web-server.py`, which adds the host's load average
— a condition of a measurement taken on it, and one the page cannot see — and
writes the file. Publishing it into `bench-results/` is a separate, deliberate
step (`just web-report`), because running the page to see it work should not
silently overwrite a committed number.

Every report also records the machine it was taken on: the CPU, the
architecture, the core count, the OS, the SIMD path in use, whether the SIMD
probe was overridden, the toolchain, and the thread count. A report whose
`machine.forced_simd` is anything but `auto` was **not** produced by the
benchmark harness's own refusal logic — the harness exits with an
explanation rather than measuring under an override — so a report that says
otherwise was hand-made, and should be treated as suspect.
