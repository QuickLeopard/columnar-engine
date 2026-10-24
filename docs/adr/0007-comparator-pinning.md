# ADR-0007: The incumbents are pinned, versioned, and driven from the repository

**Status**: accepted
**Date**: 2026-10-24
**Reviewers**: TBD

## Context

The project's comparison baseline is two external systems: `DuckDB` and `Polars` (review finding
2.1: "same harness, thread-pinned, external"). Both are run as subprocesses, which keeps them
honest -- they are the real products, not a reimplementation of them inside this project.

That leaves three things to decide:

1. **How each is invoked.** `DuckDB` ships a command line client that can read a Parquet file from
   SQL. `Polars` ships as a Python library; it has no console script, no `__main__` entry point,
   and no file-reading CLI.
2. **What is measured.** A wall clock around a subprocess measures three different costs at once:
   process start-up, the file becoming queryable, and the query. For `Polars`, the Python
   interpreter and the `polars` import alone cost about 90 ms, which is more than the query.
3. **How the thread count is pinned.** The engine is single-threaded in this increment, so both
   incumbents are pinned to one thread. `DuckDB` defaults to the core count, and `Polars` defaults
   to the core count.

An earlier version of the driver measured the wall clock around the whole subprocess and compared
it against the engine's *query* time. That made the engine look 25x faster than `DuckDB` on a
query where `DuckDB`'s own timer showed it was slightly slower. The number was wrong because the
two sides were answering different questions.

## Decision

**Pin the comparators, record their versions, and measure the load and the query separately.**

### How each is invoked

| | invocation | pinned by |
|---|---|---|
| `DuckDB` | the official CLI, `-csv` output, `.timer on` | `SET threads TO 1;` in the same session, before the query |
| `Polars` | `tools/polars-run.py`, checked into this repository | `POLARS_MAX_THREADS=1` in the environment |

`DuckDB`'s plan cache and object cache are switched off (`SET enable_object_cache=false`) so that
each measured run does the work rather than reusing a previous run's result.

`Polars` has no client that can open a file, so the driver is a checked-in script rather than an
ad-hoc command line. That is a feature: the script *is* the comparison, and a reader can see
exactly what was run. A `polars-cli` binary was built from source and thrown away -- it could not
read a Parquet file at all (`relation not found`).

### What is measured

Both incumbents are measured in two modes, and the engine is measured the same way:

- **cold** -- one query against the file, starting from nothing. The file is read as part of the
  query; there is no separate load phase.
- **resident** -- the file is loaded into memory first, then the query runs against the in-memory
  data. The load and the query are timed separately.

The resident mode is the like-for-like comparison, because the engine's own dataset is loaded once
and then queried. The cold mode is what a user pays for a one-shot command.

`DuckDB`'s own `.timer` provides the split for free: it prints one timing line per statement, so
the `CREATE TEMP TABLE` and the query are told apart by their order. The `Polars` driver prints
`LOAD_MS=`, `QUERY_MS=` and `VALUE=` markers for the same purpose.

### The answers are cross-checked

Each incumbent's answer is compared against the engine's. A comparison where the two sides
computed different values is not a benchmark, it is two systems answering different questions.
The `SUM` over 555,594 rows differs in the last bits between systems -- floating-point addition
is not associative and the engines sum in different orders -- so the check uses a relative
tolerance rather than exact equality.

## Consequences

- `DuckDB` and `Polars` versions are recorded in every report, so a number can be traced to the
  version that produced it.
- The comparators are pinned to one thread, which penalises them on the parallel-friendly parts
  of the work. The single-threaded `Polars` load is about 5.6x slower than the multi-threaded one.
  That is the price of a like-for-like comparison, and it is the right price: the alternative is
  comparing a one-thread engine against a fourteen-thread one and calling it a result.
- `tools/bin/` is not committed. `DuckDB`'s CLI is a 66 MB binary and `Polars` is a Python
  package; neither belongs in the repository. The scripts that matter are checked in.
- A reader who clones the repository needs `DuckDB` on `PATH` or in `tools/bin`, and `polars`
  installed for Python. The driver says so when it cannot find them, rather than silently
  skipping the comparison.

## Measurement

The numbers this decision produced are in `PERF.md`. The ones that shaped it:

- `DuckDB`'s process start-up is about 20 ms; `Polars`' is about 90 ms, of which the `polars`
  import is most of it. Both are excluded from the query numbers, and both are reported.
- `DuckDB`'s own `.timer` reports 31 ms for the Q6 predicate set against the Parquet file; the
  same query against a resident table takes 47 ms. The engine's query against resident data takes
  47 ms, and its Parquet decode takes 1930 ms.

## Confidence

**high** for the mechanics -- the timings come from the tools themselves, not from an external
stopwatch, and the answers are cross-checked. **medium** for the comparison as a whole, because
the incumbents and the engine do not have the same architecture: the engine decodes the whole
Parquet file eagerly into its own column chunks, while both incumbents push the predicate into
the Parquet reader and read only the columns they need. That difference is reported rather than
hidden, but it means the cold comparison is really a comparison of two different designs, and the
blueprint's own framing rule -- "kernel X on query Y" -- is the honest one to read it by.

## Revisit when

- The engine learns to read only the columns a query needs, at which point the cold comparison
  becomes a comparison of engines rather than of architectures.
- The x86-64 runner is available and the comparators are re-measured there.
