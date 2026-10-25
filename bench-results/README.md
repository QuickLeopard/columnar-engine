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
| `compare-run3.json` | the third |
| `scaling-run1.json` | the first of the three **thread-count sweeps** the §2e table is the median of |
| `scaling-run2.json` | the second, and the one `scaling.json` is a copy of |
| `scaling-run3.json` | the third |
| `compare.json` | a copy of `compare-run1.json`, the run whose figures sit nearest the medians |
| `scaling.json` | a copy of `scaling-run2.json`, the sweep run nearest the medians |
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

## The two tables

There are **two** published comparison tables, and they answer different questions.

| table | question | reports |
|---|---|---|
| §2a, §2b | *which is faster when every system is given one thread* | `compare-run*.json` |
| §2e | *how does each system's number move with the thread count* | `scaling-run*.json` |

The first is the like-for-like **single-threaded** comparison, and it has been the headline since
the first set of numbers in this repository. The second exists because a one-thread table on its
own is a misleading picture of what a user sees: `DuckDB` uses every core by default.

**Both are comparisons, not measurements.** At every point of the sweep the engine and both
incumbents run at the *same* thread count, on the same file, in the same harness, and the answers
are cross-checked at every point.

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
```

`just compare` needs `DuckDB` on `PATH` or in `tools/bin/`, and the `polars`
Python package installed. Both versions are recorded in the report, in
`machine` and in each comparator's `version` field.

Every report also records the machine it was taken on: the CPU, the
architecture, the core count, the OS, the SIMD path in use, whether the SIMD
probe was overridden, the toolchain, and the thread count. A report whose
`machine.forced_simd` is anything but `auto` was **not** produced by the
benchmark harness's own refusal logic — the harness exits with an
explanation rather than measuring under an override — so a report that says
otherwise was hand-made, and should be treated as suspect.
