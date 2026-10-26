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
RLE_EXPANSION_REPORTS = sorted((ROOT / "bench-results").glob("rle-expansion-run*.json"))
MATERIALISED_REPORTS = sorted((ROOT / "bench-results").glob("materialised-run*.json"))
CURVE_REPORTS = sorted((ROOT / "bench-results").glob("curve-run*.json"))
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


# (document, label, measurement, decimal places)
#
# Spec 012's T3 is a **refusal**, and a refusal is only worth something if
# the numbers that refused it are checkable. These are the five figures the
# decision rests on: what the expansion costs, what the replacement costs,
# what Arrow's decode costs, what the engine's scan costs, and what the
# query costs.
RLE_EXPANSION_CHECKS = [
    ("PERF.md", "the expansion", "expansion", 2),
    ("PERF.md", "the mask from the indices", "mask_from_indices", 2),
    ("PERF.md", "Arrow's decode", "arrow_decode", 2),
    ("PERF.md", "the engine's scan of the column", "engine_scan", 2),
    ("PERF.md", "the whole query", "engine_query", 2),
]


# Spec 014's claim is that the **materialised** column's cost is bytes, and
# these are the figures the claim rests on: what the column costs to decode,
# what the counterfactual -- the same column, written the other way -- costs,
# what the column's share of the query is, and what the query costs.
#
# The keys are paths into the report: `column.measurement` for a column's
# figure, and `query.measurement` for the query's.
MATERIALISED_CHECKS = [
    ("PERF.md", "the materialised column's decode", "l_extendedprice.arrow_decode", 2),
    ("PERF.md", "the counterfactual's decode", "l_extendedprice.counterfactual_arrow_decode", 2),
    ("PERF.md", "the materialised column's share", "query.materialised_share", 2),
    ("PERF.md", "the whole query", "query.q6_revenue", 2),
]


def materialised_figure(runs: list[dict], key: str) -> float | None:
    """The median of one figure in the materialised reports."""
    if key.startswith("query."):
        name = key[len("query.") :]
        if name == "materialised_share":
            values = [
                run["query"]["measurements_ms"]["predicates_and_price"]
                - run["query"]["measurements_ms"]["predicates_only"]
                for run in runs
                if "query" in run
            ]
        else:
            values = [
                run["query"]["measurements_ms"][name]
                for run in runs
                if name in run.get("query", {}).get("measurements_ms", {})
            ]
        return statistics.median(values) if values else None

    column, measurement = key.split(".", 1)
    values = [
        entry["measurements_ms"][measurement]
        for run in runs
        for entry in run.get("columns", [])
        if entry["name"] == column and measurement in entry.get("measurements_ms", {})
    ]
    return statistics.median(values) if values else None


# Spec 014's curve is a claim about the **size** of the data: this engine and
# every incumbent, at every size. These are the figures the curve leads with:
# the engine's number at each of the three sizes, and the point where the
# curve stops being the engine's.
CURVE_CHECKS = [
    ("PERF.md", "the curve's engine at 6M rows", "engine", 6000000, 1),
    ("PERF.md", "the curve's engine at 12M rows", "engine", 12000000, 1),
    ("PERF.md", "the curve's engine at 24M rows", "engine", 24000000, 1),
    ("PERF.md", "the curve's duckdb at 24M rows", "duckdb", 24000000, 1),
]


def curve_figure(runs: list[dict], system: str, rows: int) -> float | None:
    """The median of one system's figure at one size."""
    field = f"{system}_ms"
    values = [
        point[field]
        for run in runs
        for point in run.get("points", [])
        if point.get("rows") == rows and point.get(field) is not None
    ]
    return statistics.median(values) if values else None


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
                ("datafusion", "datafusion_ms"),
                ("workers", "workers"),
            ):
                value = point.get(field)
                if value is not None:
                    out.setdefault(threads, {}).setdefault(key, []).append(value)
    return out


# Spec 015's claim is that the engine's parallelism is capped by the **file's**
# row-group count rather than by the machine, and the confound is that the
# row-group size changes the file as well. These are the figures both halves
# of the claim rest on: the workers that ran against the threads asked for,
# the engine's number at each end of the sweep, and the incumbents' numbers on
# the same files -- which are the control.
#
# The three files are the same query, the same rows and the same seed; they
# differ in one thing, the row-group size.
ROW_GROUP_SWEEPS = [
    ("A", "scaling-run[0-9].json"),
    ("B", "scaling-rg12-run*.json"),
    ("C", "scaling-rg24-run*.json"),
]

ROW_GROUP_PROBES = [
    ("A", "row-groups-6-run*.json"),
    ("B", "row-groups-12-run*.json"),
    ("C", "row-groups-24-run*.json"),
]

# (document, label, file, threads, system, places)
ROW_GROUP_SCALING_CHECKS = [
    ("PERF.md", "the engine at 14 threads, 6 groups", "A", 14, "engine", 1),
    ("PERF.md", "the engine at 14 threads, 12 groups", "B", 14, "engine", 1),
    ("PERF.md", "the engine at 14 threads, 24 groups", "C", 14, "engine", 1),
    ("PERF.md", "the engine at 1 thread, 6 groups", "A", 1, "engine", 1),
    ("PERF.md", "the engine at 1 thread, 24 groups", "C", 1, "engine", 1),
    ("PERF.md", "duckdb at 1 thread, 6 groups", "A", 1, "duckdb", 1),
    ("PERF.md", "duckdb at 1 thread, 24 groups", "C", 1, "duckdb", 1),
    ("PERF.md", "polars at 1 thread, 6 groups", "A", 1, "polars", 1),
    ("PERF.md", "polars at 1 thread, 24 groups", "C", 1, "polars", 1),
    ("PERF.md", "datafusion at 1 thread, 6 groups", "A", 1, "datafusion", 1),
    ("PERF.md", "datafusion at 1 thread, 24 groups", "C", 1, "datafusion", 1),
]

# (document, label, file, threads, places)
#
# The worker count is the **cap** itself, and the speedup is what the cap
# costs. The worker count is a small integer, so the speedup is checked too:
# it is the figure a reader would quote.
ROW_GROUP_CAP_CHECKS = [
    ("PERF.md", "the workers at 14 threads, 6 groups", "A", 14, 0),
    ("PERF.md", "the workers at 14 threads, 12 groups", "B", 14, 0),
    ("PERF.md", "the workers at 14 threads, 24 groups", "C", 14, 0),
    ("PERF.md", "the workers at 8 threads, 6 groups", "A", 8, 0),
    ("PERF.md", "the workers at 8 threads, 12 groups", "B", 8, 0),
    ("PERF.md", "the workers at 8 threads, 24 groups", "C", 8, 0),
]

# (document, label, file, threads, places)
ROW_GROUP_SPEEDUP_CHECKS = [
    ("PERF.md", "the engine's speedup at 14 threads, 6 groups", "A", 14, 2),
    ("PERF.md", "the engine's speedup at 14 threads, 12 groups", "B", 14, 2),
    ("PERF.md", "the engine's speedup at 14 threads, 24 groups", "C", 14, 2),
]

# (document, label, file, key, places)
#
# The probe's figures are the **confound**: what the same predicates and the
# same materialised column cost on each file, measured in one session.
ROW_GROUP_PROBE_CHECKS = [
    ("PERF.md", "the predicates alone, 6 groups", "A", "query.predicates_only", 2),
    ("PERF.md", "the predicates alone, 12 groups", "B", "query.predicates_only", 2),
    ("PERF.md", "the predicates alone, 24 groups", "C", "query.predicates_only", 2),
    ("PERF.md", "the whole query, 6 groups", "A", "query.q6_revenue", 2),
    ("PERF.md", "the whole query, 12 groups", "B", "query.q6_revenue", 2),
    ("PERF.md", "the whole query, 24 groups", "C", "query.q6_revenue", 2),
    ("PERF.md", "the materialised column's decode, 6 groups", "A", "l_extendedprice.arrow_decode", 2),
    ("PERF.md", "the materialised column's decode, 24 groups", "C", "l_extendedprice.arrow_decode", 2),
]


def load_reports(pattern: str) -> list[dict]:
    """The committed reports matching a glob, loaded."""
    return [json.loads(path.read_text()) for path in sorted((ROOT / "bench-results").glob(pattern))]



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

    if CURVE_REPORTS:
        curve_runs = [json.loads(path.read_text()) for path in CURVE_REPORTS]
        for doc, label, system, rows, places in CURVE_CHECKS:
            figure = curve_figure(curve_runs, system, rows)
            if figure is None:
                failures.append(f"{doc}: no figure for {label} in the curve reports")
                continue
            rendered = f"{figure:.{places}f}"
            if rendered not in texts[doc]:
                failures.append(
                    f"{doc}: {label} is {rendered} in the reports but that figure "
                    f"does not appear in the document"
                )
            else:
                print(f"ok  {doc:<10} {label:<28} {rendered}")

    if MATERIALISED_REPORTS:
        materialised_runs = [json.loads(path.read_text()) for path in MATERIALISED_REPORTS]
        for doc, label, key, places in MATERIALISED_CHECKS:
            figure = materialised_figure(materialised_runs, key)
            if figure is None:
                failures.append(f"{doc}: no figure for {label} in the materialised reports")
                continue
            rendered = f"{figure:.{places}f}"
            if rendered not in texts[doc]:
                failures.append(
                    f"{doc}: {label} is {rendered} in the reports but that figure "
                    f"does not appear in the document"
                )
            else:
                print(f"ok  {doc:<10} {label:<28} {rendered}")

        # Every campaign must agree on the rows that survive, and on the
        # answer: the claim is about one query, and the query must be the same
        # query every time.
        survivors = {run.get("query", {}).get("survivors") for run in materialised_runs}
        if len(survivors) != 1:
            failures.append(
                "the materialised campaigns disagree on the surviving rows: "
                f"{sorted(survivors)}"
            )
        else:
            rows = survivors.pop()
            if str(rows) not in texts["PERF.md"]:
                failures.append(
                    f"PERF.md: the surviving rows are {rows} in the reports but "
                    f"that figure does not appear in the document"
                )
            else:
                print(f"ok  {'PERF.md':<10} {'the surviving rows':<28} {rows}")

        answers = {run.get("query", {}).get("value") for run in materialised_runs}
        if len(answers) != 1:
            failures.append(f"the materialised campaigns disagree on the answer: {answers}")

    # Spec 015: the row-group cap, the sweep on each file, and the probe that
    # measures the confound.
    sweeps = {
        name: collect_scaling(load_reports(pattern)) for name, pattern in ROW_GROUP_SWEEPS
    }
    sweeps = {name: sweep for name, sweep in sweeps.items() if sweep}

    for doc, label, name, threads, system, places in ROW_GROUP_SCALING_CHECKS:
        sweep = sweeps.get(name)
        if not sweep:
            # No reports, no figures to check: the same rule the other
            # sections follow, so a report set that is not committed yet does
            # not fail the gate.
            continue
        values = sweep.get(threads, {}).get(system)
        if not values:
            failures.append(f"{doc}: no figure for {label} in the row-group sweeps")
            continue
        figure = statistics.median(values)
        rendered = f"{figure:.{places}f}"
        if rendered not in texts[doc]:
            failures.append(
                f"{doc}: {label} is {rendered} in the reports but that figure "
                f"does not appear in the document"
            )
        else:
            print(f"ok  {doc:<10} {label:<40} {rendered}")

    for doc, label, name, threads, places in ROW_GROUP_CAP_CHECKS:
        sweep = sweeps.get(name)
        if not sweep:
            # No reports, no figures to check: the same rule the other
            # sections follow, so a report set that is not committed yet does
            # not fail the gate.
            continue
        values = sweep.get(threads, {}).get("workers")
        if not values:
            failures.append(f"{doc}: no worker count for {label}")
            continue
        figure = statistics.median(values)
        rendered = f"{figure:.{places}f}"
        if rendered not in texts[doc]:
            failures.append(
                f"{doc}: {label} is {rendered} in the reports but that figure "
                f"does not appear in the document"
            )
        else:
            print(f"ok  {doc:<10} {label:<40} {rendered}")

    for doc, label, name, threads, places in ROW_GROUP_SPEEDUP_CHECKS:
        sweep = sweeps.get(name)
        if not sweep:
            # No reports, no figures to check: the same rule the other
            # sections follow, so a report set that is not committed yet does
            # not fail the gate.
            continue
        base = sweep.get(1, {}).get("engine")
        now = sweep.get(threads, {}).get("engine")
        if not base or not now:
            failures.append(f"{doc}: no speedup for {label}")
            continue
        figure = statistics.median(base) / statistics.median(now)
        # The documents write the multiplier sign; the plain one is accepted
        # too, because it is the same figure and a stranger may type either.
        rendered = f"{figure:.{places}f}"
        if not any(f"{rendered}{sign}" in texts[doc] for sign in ("x", "×")):
            failures.append(
                f"{doc}: {label} is {rendered}x in the reports but that figure "
                f"does not appear in the document"
            )
        else:
            print(f"ok  {doc:<10} {label:<40} {rendered}x")

    for doc, label, name, key, places in ROW_GROUP_PROBE_CHECKS:
        pattern = dict(ROW_GROUP_PROBES)[name]
        probe = load_reports(pattern)
        if not probe:
            # No reports, no figures to check: the same rule the other
            # sections follow, so a report set that is not committed yet does
            # not fail the gate.
            continue
        figure = materialised_figure(probe, key)
        if figure is None:
            failures.append(f"{doc}: no figure for {label} in the row-group probes")
            continue
        rendered = f"{figure:.{places}f}"
        if rendered not in texts[doc]:
            failures.append(
                f"{doc}: {label} is {rendered} in the reports but that figure "
                f"does not appear in the document"
            )
        else:
            print(f"ok  {doc:<10} {label:<40} {rendered}")

    # The three files must agree on the answer: they are the same rows in a
    # different arrangement, and a different answer would mean the row-group
    # size changed the query rather than the schedule.
    answers = set()
    survivors = set()
    for name, pattern in ROW_GROUP_PROBES:
        for run in load_reports(pattern):
            answers.add(run.get("query", {}).get("value"))
            survivors.add(run.get("query", {}).get("survivors"))
    if len(answers) != 1:
        failures.append(f"the row-group campaigns disagree on the answer: {answers}")
    if len(survivors) != 1:
        failures.append(
            f"the row-group campaigns disagree on the surviving rows: {sorted(survivors)}"
        )

    if RLE_EXPANSION_REPORTS:
        expansion_runs = [json.loads(path.read_text()) for path in RLE_EXPANSION_REPORTS]
        for doc, label, key, places in RLE_EXPANSION_CHECKS:
            values = [
                run["measurements_ms"][key]
                for run in expansion_runs
                if key in run.get("measurements_ms", {})
            ]
            if not values:
                failures.append(f"{doc}: no figure for {label} in the expansion reports")
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

        # Every campaign must agree on the rows that survive: the expansion
        # is a claim about a column, and the column's answer is the same
        # answer the engine gives.
        survivors = {run.get("survivors") for run in expansion_runs}
        if len(survivors) != 1:
            failures.append(
                f"the expansion campaigns disagree on the surviving rows: {sorted(survivors)}"
            )
        else:
            count = survivors.pop()
            print(f"ok  {'PERF.md':<10} {'the surviving rows':<28} {count:,}")

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
