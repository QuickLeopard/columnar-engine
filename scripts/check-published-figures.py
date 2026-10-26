#!/usr/bin/env python3
"""Check that the published tables still agree with the committed reports.

`PERF.md` and `README.md` quote the medians of the committed comparison
reports. This script derives those medians and asserts that the headline
figures appear in the prose, so a hand-edited number cannot drift away from
the evidence without the gate noticing.

It is deliberately narrow: it checks the figures that the documents lead with,
not every number in them. A drift check that tried to parse all of the prose
would break on a reworded sentence, and a check that never fires is worth
nothing.

Usage:

    python3 scripts/check-published-figures.py
"""

from __future__ import annotations

import json
import statistics
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REPORTS = sorted((ROOT / "bench-results").glob("compare-run*.json"))
SCALING_REPORTS = sorted((ROOT / "bench-results").glob("scaling-run*.json"))
BROWSER_REPORTS = sorted((ROOT / "bench-results").glob("browser-run*.json"))
RESIDENT_SCALING_REPORTS = sorted(
    (ROOT / "bench-results").glob("scaling-resident-run*.json")
)
LOAD_SCALING_REPORTS = sorted((ROOT / "bench-results").glob("scaling-load-run*.json"))
DOCS = [ROOT / "PERF.md", ROOT / "README.md"]

# (document, label, figure as it appears in the prose)
# The cold `q6` row is the headline of both documents; the load figures and
# the eager/lazy medians are the ones a reader is most likely to quote.
CHECKS = [
    ("PERF.md", "lazy q6 cold", "engine", "lazy", "cold", "q6", 1),
    ("PERF.md", "eager q6 cold", "engine", "eager", "cold", "q6", 1),
    ("PERF.md", "duckdb q6 cold", "duckdb", "-", "cold", "q6", 1),
    ("PERF.md", "polars q6 cold", "polars", "-", "cold", "q6", 1),
    ("PERF.md", "lazy q6 resident-free scan", "engine", "lazy", "cold", "scan", 1),
    ("PERF.md", "duckdb q6 resident", "duckdb", "-", "resident", "q6", 1),
    ("README.md", "lazy q6 cold", "engine", "lazy", "cold", "q6", 1),
    ("README.md", "eager q6 cold", "engine", "eager", "cold", "q6", 1),
    ("README.md", "duckdb q6 cold", "duckdb", "-", "cold", "q6", 1),
    ("README.md", "polars q6 cold", "polars", "-", "cold", "q6", 1),
    ("PERF.md", "lazy q6-revenue cold", "engine", "lazy", "cold", "q6-revenue", 1),
    ("PERF.md", "duckdb q6-revenue cold", "duckdb", "-", "cold", "q6-revenue", 1),
    ("PERF.md", "polars q6-revenue cold", "polars", "-", "cold", "q6-revenue", 1),
    ("README.md", "lazy q6-revenue cold", "engine", "lazy", "cold", "q6-revenue", 1),
    # `filter-range` is the query the aggregate reads in place, so its two
    # figures are the ones a reader would quote to check that the fusion is
    # still there.
    ("PERF.md", "filter-range resident", "engine", "eager", "resident", "filter-range", 1),
    ("PERF.md", "filter-range lazy cold", "engine", "lazy", "cold", "filter-range", 1),
    ("PERF.md", "polars filter-range resident", "polars", "-", "resident", "filter-range", 1),
    ("PERF.md", "polars filter-range cold", "polars", "-", "cold", "filter-range", 1),
    ("README.md", "filter-range resident", "engine", "eager", "resident", "filter-range", 1),
]

# (document, label, thread count, system, places)
#
# The sweep is the second table, and its headline is the **engine at its
# cap** against **DuckDB at the same thread count**. Those are the two
# figures a reader is most likely to quote, and the two that would be most
# misleading if they drifted away from the reports.
SCALING_CHECKS = [
    ("PERF.md", "sweep engine q6 at 6 threads", 6, "engine", 1),
    ("PERF.md", "sweep duckdb q6 at 6 threads", 6, "duckdb", 1),
    ("PERF.md", "sweep engine q6 at 1 thread", 1, "engine", 1),
    ("PERF.md", "sweep duckdb q6 at 1 thread", 1, "duckdb", 1),
]

# The **resident** sweep is the table the parallel operators show up in, and
# it is the one this increment added. The same rule applies: the engine at its
# best point against DuckDB at the same thread count, and the one-thread
# figures at both ends.
RESIDENT_SCALING_CHECKS = [
    ("PERF.md", "resident sweep engine q6 at 6 threads", 6, "engine", 1),
    ("PERF.md", "resident sweep duckdb q6 at 6 threads", 6, "duckdb", 1),
    ("PERF.md", "resident sweep engine q6 at 1 thread", 1, "engine", 1),
    ("PERF.md", "resident sweep duckdb q6 at 1 thread", 1, "duckdb", 1),
]

# The **load** sweep is the table the parallel eager decode shows up in.
LOAD_SCALING_CHECKS = [
    ("PERF.md", "load sweep engine q6 at 6 threads", 6, "engine", 1),
    ("PERF.md", "load sweep duckdb q6 at 6 threads", 6, "duckdb", 1),
    ("PERF.md", "load sweep engine q6 at 1 thread", 1, "engine", 1),
    ("PERF.md", "load sweep duckdb q6 at 1 thread", 1, "duckdb", 1),
]


def collect(runs: list[dict]) -> dict[tuple[str, str, str, str], list[float]]:
    out: dict[tuple[str, str, str, str], list[float]] = {}
    for run in runs:
        for entry in run.get("engine_queries", []):
            key = ("engine", entry["mode"], "cold", entry["query"])
            out.setdefault(key, []).append(entry["cold_ms"])
            key = ("engine", entry["mode"], "resident", entry["query"])
            out.setdefault(key, []).append(entry["elapsed_ms"])
        for entry in run.get("external", []):
            key = (entry["comparator"], "-", entry["mode"], entry["query"])
            out.setdefault(key, []).append(entry["query_ms"])
    return out


def collect_scaling(runs: list[dict]) -> dict[int, dict[str, list[float]]]:
    out: dict[int, dict[str, list[float]]] = {}
    for run in runs:
        for point in run.get("points", []):
            threads = point["threads"]
            for key, field in (
                ("engine", "engine_ms"),
                ("duckdb", "duckdb_ms"),
                ("polars", "polars_ms"),
            ):
                value = point.get(field)
                if value is not None:
                    out.setdefault(threads, {}).setdefault(key, []).append(value)
    return out


def main() -> int:
    if not REPORTS:
        print("no comparison reports in bench-results/", file=sys.stderr)
        return 1

    with REPORTS[0].open() as handle:
        runs = [json.load(handle)]
    for path in REPORTS[1:]:
        with path.open() as handle:
            runs.append(json.load(handle))

    agg = collect(runs)
    texts = {doc.name: doc.read_text() for doc in DOCS}

    failures: list[str] = []
    for doc, label, system, mode, metric, query, places in CHECKS:
        values = agg.get((system, mode, metric, query))
        if not values:
            failures.append(f"{doc}: no figure for {label} in the reports")
            continue
        figure = statistics.median(values)
        rendered = f"{figure:.{places}f}"
        if rendered not in texts[doc]:
            failures.append(
                f"{doc}: {label} is {rendered} in the reports but that figure "
                f"does not appear in the document"
            )
        else:
            print(f"ok  {doc:<10} {label:<28} {rendered}")

    if SCALING_REPORTS:
        scaling_runs = []
        for path in SCALING_REPORTS:
            with path.open() as handle:
                scaling_runs.append(json.load(handle))
        sweep = collect_scaling(scaling_runs)

        for doc, label, threads, system, places in SCALING_CHECKS:
            values = sweep.get(threads, {}).get(system)
            if not values:
                failures.append(f"{doc}: no figure for {label} in the sweep reports")
                continue
            figure = statistics.median(values)
            rendered = f"{figure:.{places}f}"
            if rendered not in texts[doc]:
                failures.append(
                    f"{doc}: {label} is {rendered} in the reports but that figure "
                    f"does not appear in the document"
                )
            else:
                print(f"ok  {doc:<10} {label:<28} {rendered}")

    if RESIDENT_SCALING_REPORTS:
        resident_runs = []
        for path in RESIDENT_SCALING_REPORTS:
            with path.open() as handle:
                resident_runs.append(json.load(handle))
        resident_sweep = collect_scaling(resident_runs)

        for doc, label, threads, system, places in RESIDENT_SCALING_CHECKS:
            values = resident_sweep.get(threads, {}).get(system)
            if not values:
                failures.append(f"{doc}: no figure for {label} in the resident sweep reports")
                continue
            figure = statistics.median(values)
            rendered = f"{figure:.{places}f}"
            if rendered not in texts[doc]:
                failures.append(
                    f"{doc}: {label} is {rendered} in the reports but that figure "
                    f"does not appear in the document"
                )
            else:
                print(f"ok  {doc:<10} {label:<28} {rendered}")

    if LOAD_SCALING_REPORTS:
        load_runs = []
        for path in LOAD_SCALING_REPORTS:
            with path.open() as handle:
                load_runs.append(json.load(handle))
        load_sweep = collect_scaling(load_runs)

        for doc, label, threads, system, places in LOAD_SCALING_CHECKS:
            values = load_sweep.get(threads, {}).get(system)
            if not values:
                failures.append(f"{doc}: no figure for {label} in the load sweep reports")
                continue
            figure = statistics.median(values)
            rendered = f"{figure:.{places}f}"
            if rendered not in texts[doc]:
                failures.append(
                    f"{doc}: {label} is {rendered} in the reports but that figure "
                    f"does not appear in the document"
                )
            else:
                print(f"ok  {doc:<10} {label:<28} {rendered}")

    if BROWSER_REPORTS:
        # The browser's measurement is a **browser's** number and is never
        # compared with the native one. What it can be checked against is the
        # one thing the two runs share: the number of rows that match. The
        # browser rebuilds the benchmark's column from a histogram, so its
        # match count must be the native's -- and the documents quote it.
        browser_runs = [json.loads(path.read_text()) for path in BROWSER_REPORTS]
        counts = {run.get("matches") for run in browser_runs}
        if len(counts) != 1:
            failures.append(
                f"the browser campaigns disagree on the match count: {sorted(counts)}"
            )
        else:
            count = counts.pop()
            for doc in DOCS:
                rendered = f"{count:,}"
                if rendered not in texts[doc.name]:
                    failures.append(
                        f"{doc.name}: the browser's match count is {rendered} in the "
                        f"reports but that figure does not appear in the document"
                    )
                else:
                    print(f"ok  {doc.name:<10} {'browser match count':<28} {rendered}")
        for run in browser_runs:
            if not run.get("masks_agree"):
                failures.append(
                    "a browser campaign reports that the two masks did not agree"
                )

    if failures:
        print()
        for failure in failures:
            print(f"FAIL  {failure}")
        print(
            "\nThe published table and the committed reports disagree. Either "
            "the document is stale or the reports are. Re-derive with "
            "`python3 scripts/medians.py`."
        )
        return 1

    print(f"\nall published figures agree with {len(REPORTS)} committed reports")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
