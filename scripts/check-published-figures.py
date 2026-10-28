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
import re
import statistics
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REPORTS = sorted((ROOT / "bench-results").glob("compare-run*.json"))
BEFORE_REPORTS = sorted((ROOT / "bench-results").glob("compare-before-run*.json"))
SPEC021_REPORTS = sorted((ROOT / "bench-results").glob("compare-spec021-run*.json"))
SPEC023_REPORTS = sorted((ROOT / "bench-results").glob("compare-spec023-run*.json"))
QUERY_COST_REPORTS = sorted((ROOT / "bench-results").glob("query-cost-run*.json"))
QUERY_COST_BEFORE_REPORTS = sorted((ROOT / "bench-results").glob("query-cost-before-run*.json"))
PROBE_REPORTS = sorted((ROOT / "bench-results").glob("operator-probe-*.json"))
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

# The number of campaigns a machine has to run before its median is published.
# This is the repository's rule: a single run is an estimate of a number that
# moves, so a median needs at least three.
MIN_CAMPAIGNS = 3

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
    # The two queries spec 020 added. The join's **resident** figure is
    # deliberately absent: the engine's resident path reads the build side
    # inside the clock, and the incumbents load it before the clock starts, so
    # the two numbers would not be the same measurement. It is named in the
    # prose instead of published next to a number measured differently.
    ("PERF.md", "lazy join-orders cold", "engine", "lazy", "cold", "join-orders", 1),
    ("PERF.md", "duckdb join-orders cold", "duckdb", "-", "cold", "join-orders", 1),
    ("PERF.md", "polars join-orders cold", "polars", "-", "cold", "join-orders", 1),
    ("PERF.md", "datafusion join-orders cold", "datafusion-peer", "-", "cold", "join-orders", 1),
    ("PERF.md", "lazy sort-topk cold", "engine", "lazy", "cold", "sort-topk", 1),
    ("PERF.md", "duckdb sort-topk cold", "duckdb", "-", "cold", "sort-topk", 1),
    ("PERF.md", "polars sort-topk cold", "polars", "-", "cold", "sort-topk", 1),
    ("PERF.md", "datafusion sort-topk cold", "datafusion-peer", "-", "cold", "sort-topk", 1),
    ("PERF.md", "sort-topk resident", "engine", "eager", "resident", "sort-topk", 1),
    ("PERF.md", "duckdb sort-topk resident", "duckdb", "-", "resident", "sort-topk", 1),
    ("PERF.md", "polars sort-topk resident", "polars", "-", "resident", "sort-topk", 1),
]

# (document, label, system, mode, metric, query, places)
#
# Spec 021 publishes the **old code's** figures beside the new ones, so a
# reader can see what the change did rather than what the machine did. Those
# old figures are measured on the **same host**, back to back, in their own
# campaigns -- so they are derived from committed reports here too, and not
# typed into the prose.
BEFORE_CHECKS = [
    ("PERF.md", "the old code's lazy join-orders cold", "engine", "lazy", "cold", "join-orders", 1),
    ("PERF.md", "the old code's lazy sort-topk cold", "engine", "lazy", "cold", "sort-topk", 1),
    ("PERF.md", "the old code's sort-topk resident", "engine", "eager", "resident", "sort-topk", 1),
    ("PERF.md", "the old code's lazy q6 cold", "engine", "lazy", "cold", "q6", 1),
    ("PERF.md", "the old code's duckdb q6 cold", "duckdb", "-", "cold", "q6", 1),
]

# (document, label, system, mode, metric, query, places)
#
# The set **before** the one this file publishes now -- spec 021's set. Its
# figures are published beside the new ones, so they are derived from the
# reports they came from rather than typed into the prose.
SPEC021_CHECKS = [
    ("PERF.md", "the last set's lazy join-orders cold", "engine", "lazy", "cold", "join-orders", 1),
    ("PERF.md", "the last set's lazy sort-topk cold", "engine", "lazy", "cold", "sort-topk", 1),
    ("PERF.md", "the last set's sort-topk resident", "engine", "eager", "resident", "sort-topk", 1),
    ("PERF.md", "the last set's join-orders resident", "engine", "eager", "resident", "join-orders", 1),
    ("PERF.md", "the last set's lazy q6 cold", "engine", "lazy", "cold", "q6", 1),
    ("PERF.md", "the last set's eager q6 cold", "engine", "eager", "cold", "q6", 1),
    ("PERF.md", "the last set's duckdb q6 cold", "duckdb", "-", "cold", "q6", 1),
    ("PERF.md", "the last set's polars q6 cold", "polars", "-", "cold", "q6", 1),
    ("PERF.md", "the last set's duckdb join-orders cold", "duckdb", "-", "cold", "join-orders", 1),
    ("PERF.md", "the last set's duckdb sort-topk cold", "duckdb", "-", "cold", "sort-topk", 1),
    ("PERF.md", "the last set's duckdb sort-topk resident", "duckdb", "-", "resident", "sort-topk", 1),
]

# (document, label, system, mode, metric, query, places)
#
# The set **this one replaces** -- spec 023's set, the one that was published
# before it. Its figures are published beside the new ones, old against new,
# so they are derived from the reports they came from rather than typed into
# the prose. The reports are kept under their own name so that every figure
# this file quotes stays derivable from a committed report.
SPEC023_CHECKS = [
    ("PERF.md", "the replaced set's lazy join-orders cold", "engine", "lazy", "cold", "join-orders", 1),
    ("PERF.md", "the replaced set's lazy sort-topk cold", "engine", "lazy", "cold", "sort-topk", 1),
    ("PERF.md", "the replaced set's lazy q6-revenue cold", "engine", "lazy", "cold", "q6-revenue", 1),
    ("PERF.md", "the replaced set's join-orders resident", "engine", "eager", "resident", "join-orders", 1),
    ("PERF.md", "the replaced set's sort-topk resident", "engine", "eager", "resident", "sort-topk", 1),
    ("PERF.md", "the replaced set's eager q6 cold", "engine", "eager", "cold", "q6", 1),
    ("PERF.md", "the replaced set's duckdb q6 cold", "duckdb", "-", "cold", "q6", 1),
    ("PERF.md", "the replaced set's duckdb join-orders cold", "duckdb", "-", "cold", "join-orders", 1),
    ("PERF.md", "the replaced set's duckdb sort-topk cold", "duckdb", "-", "cold", "sort-topk", 1),
    ("PERF.md", "the replaced set's duckdb sort-topk resident", "duckdb", "-", "resident", "sort-topk", 1),
    ("PERF.md", "the replaced set's polars q6 cold", "polars", "-", "cold", "q6", 1),
    ("PERF.md", "the replaced set's polars join-orders cold", "polars", "-", "cold", "join-orders", 1),
    ("PERF.md", "the replaced set's datafusion join-orders cold", "datafusion-peer", "-", "cold", "join-orders", 1),
    ("PERF.md", "the replaced set's datafusion sort-topk cold", "datafusion-peer", "-", "cold", "sort-topk", 1),
]

# (document, label, which, shape, places)
#
# Spec 024's instrument, first part: the shapes of the dataset's "every row"
# selection -- the one the old code expanded into row numbers, and the bitmap
# that is the control.
QUERY_COST_SHAPE_CHECKS = [
    ("PERF.md", "the old code's selection expanded", "before", "selection_rows", 2),
    ("PERF.md", "the new code's selection expanded", "after", "selection_rows", 2),
    ("PERF.md", "the old code's rows read as they are needed", "before", "selection_walk", 2),
    ("PERF.md", "the new code's rows read as they are needed", "after", "selection_walk", 2),
    ("PERF.md", "the old code's expansion, with the selection a bitmap", "before", "bitmap_rows", 2),
    ("PERF.md", "the new code's expansion, with the selection a bitmap", "after", "bitmap_rows", 2),
    ("PERF.md", "the old code's walk, with the selection a bitmap", "before", "bitmap_walk", 2),
    ("PERF.md", "the new code's walk, with the selection a bitmap", "after", "bitmap_walk", 2),
]

# (document, label, which, path, query, key, places)
#
# Spec 024's instrument, second part: the whole query, the plan output's own
# stage table, and the time between them -- the part of the query the plan
# output does not name -- for **every** published query, on **both** paths.
QUERY_COST_CHECKS = [
    ("PERF.md", "the old code's whole query, the join", "before", "resident", "join-orders", "query_ms", 2),
    ("PERF.md", "the new code's whole query, the join", "after", "resident", "join-orders", "query_ms", 2),
    ("PERF.md", "the old code's stage table, the join", "before", "resident", "join-orders", "stages_ms", 2),
    ("PERF.md", "the new code's stage table, the join", "after", "resident", "join-orders", "stages_ms", 2),
    ("PERF.md", "the old code's unnamed time, the join", "before", "resident", "join-orders", "unnamed_ms", 3),
    ("PERF.md", "the new code's unnamed time, the join", "after", "resident", "join-orders", "unnamed_ms", 3),
    ("PERF.md", "the old code's whole query, the sort", "before", "resident", "sort-topk", "query_ms", 2),
    ("PERF.md", "the new code's whole query, the sort", "after", "resident", "sort-topk", "query_ms", 2),
    ("PERF.md", "the old code's stage table, the sort", "before", "resident", "sort-topk", "stages_ms", 2),
    ("PERF.md", "the new code's stage table, the sort", "after", "resident", "sort-topk", "stages_ms", 2),
    ("PERF.md", "the old code's unnamed time, the sort", "before", "resident", "sort-topk", "unnamed_ms", 3),
    ("PERF.md", "the new code's unnamed time, the sort", "after", "resident", "sort-topk", "unnamed_ms", 3),
    ("PERF.md", "the old code's unnamed time, the join, cold", "before", "cold", "join-orders", "unnamed_ms", 3),
    ("PERF.md", "the new code's unnamed time, the join, cold", "after", "cold", "join-orders", "unnamed_ms", 3),
    ("PERF.md", "the old code's unnamed time, the sort, cold", "before", "cold", "sort-topk", "unnamed_ms", 3),
    ("PERF.md", "the new code's unnamed time, the sort, cold", "after", "cold", "sort-topk", "unnamed_ms", 3),
    ("PERF.md", "the old code's unnamed time, the revenue query, cold", "before", "cold", "q6-revenue", "unnamed_ms", 3),
    ("PERF.md", "the new code's unnamed time, the revenue query, cold", "after", "cold", "q6-revenue", "unnamed_ms", 3),
]

# (document, label, report, measurement, places)
#
# Spec 021's instrument is the **operator probe**, and its figures are the
# claim the increment rests on: the join's probe and the sort, taken apart.
# The probe writes one report per code version -- `operator-probe-before`
# against the code the increment replaced, `operator-probe-after` against the
# code it wrote -- so both halves of every published number are derived.
PROBE_CHECKS = [
    ("PERF.md", "the probe's engine, before", "before", "engine", 1),
    ("PERF.md", "the probe's engine, after", "after", "engine", 1),
    ("PERF.md", "the probe's floor, before", "before", "read", 1),
    ("PERF.md", "the probe's plain floor, after", "after", "read_plain", 1),
    ("PERF.md", "the probe's direct table, before", "before", "direct_bytes", 1),
    ("PERF.md", "the probe's bitmap, before", "before", "prefilter", 1),
    ("PERF.md", "the probe's bitmap, after", "after", "prefilter", 1),
    ("PERF.md", "the probe's table, after", "after", "int_table", 1),
    ("PERF.md", "the probe's sort, before", "before", "bounded", 1),
    ("PERF.md", "the probe's sort, after", "after", "bounded", 1),
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


# A line of the plan output: the stage's name, the rows it was handed, the rows
# it produced, and how long it took.
STAGE_LINE = re.compile(r"^([a-z]+)\s+(\d+)\s+(\d+)\s+([0-9.]+) ms$")


def collect_stages(runs: list[dict]) -> dict[tuple[str, str, str], list[float]]:
    """The plan output's stage timings, taken from the reports themselves.

    Every campaign records the plan output of every query it ran, so the
    engine's own account of the work is in the committed reports and not only
    in the prose. The key is `(query, mode, stage)`.
    """
    out: dict[tuple[str, str, str], list[float]] = {}
    for run in runs:
        for entry in run.get("engine_queries", []):
            for line in (entry.get("plan_output") or "").splitlines():
                match = STAGE_LINE.match(line.strip())
                if match is None:
                    continue
                key = (entry["query"], entry["mode"], match.group(1))
                out.setdefault(key, []).append(float(match.group(4)))
    return out


def _query_cost_shapes(paths: list[Path]) -> dict[str, list[float]]:
    """The query probe's shapes, from the committed reports.

    The shapes are the dataset's "every row" selection, measured once: they
    are a property of the dataset and not of a query.
    """
    out: dict[str, list[float]] = {}
    for path in paths:
        run = json.loads(path.read_text())
        for key, value in run.get("shapes_ms", {}).items():
            out.setdefault(key, []).append(value)
    return out


def _query_cost_queries(paths: list[Path]) -> dict[str, dict[str, dict[str, list[float]]]]:
    """The query probe's per-query figures, from the committed reports.

    The key is `(path, query, key)`: the path is the resident one or the
    cold one, and each query keeps its own whole-query time, its own stage
    table and its own time outside the stages.
    """
    out: dict[str, dict[str, dict[str, list[float]]]] = {}
    for path in paths:
        run = json.loads(path.read_text())
        for which, entries in run.get("queries", {}).items():
            for query, values in entries.items():
                for key, value in values.items():
                    if key.endswith("_spread_ms"):
                        continue
                    out.setdefault(which, {}).setdefault(query, {}).setdefault(key, []).append(value)
    return out


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


# (document, label, set, query, stage, places)
#
# The **plan output** is the engine's own account of the work, and it is in the
# committed reports: every campaign records it. Spec 021 publishes the stage
# breakdown beside the query figures, for the three sets, so that the code's
# share can be read a stage at a time rather than from a clock around the whole
# query. These are the figures that table rests on.
STAGE_CHECKS = [
    ("PERF.md", "the new set's join stage", "new", "join-orders", "join", 1),
    ("PERF.md", "the new set's sort stage", "new", "sort-topk", "sort", 1),
    ("PERF.md", "the new set's join scan", "new", "join-orders", "scan", 1),
    ("PERF.md", "the new set's sort scan", "new", "sort-topk", "scan", 1),
    ("PERF.md", "the old set's join stage", "before", "join-orders", "join", 1),
    ("PERF.md", "the old set's sort stage", "before", "sort-topk", "sort", 1),
    ("PERF.md", "the old set's join scan", "before", "join-orders", "scan", 1),
    ("PERF.md", "the old set's sort scan", "before", "sort-topk", "scan", 1),
    ("PERF.md", "the last set's join stage", "last", "join-orders", "join", 1),
    ("PERF.md", "the last set's sort stage", "last", "sort-topk", "sort", 1),
    ("PERF.md", "the last set's join scan", "last", "join-orders", "scan", 1),
    ("PERF.md", "the replaced set's join stage", "replaced", "join-orders", "join", 1),
    ("PERF.md", "the replaced set's sort stage", "replaced", "sort-topk", "sort", 1),
    ("PERF.md", "the replaced set's join scan", "replaced", "join-orders", "scan", 1),
    ("PERF.md", "the replaced set's sort scan", "replaced", "sort-topk", "scan", 1),
]


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


# Spec 016: the reader's batch limit. The published table is the limit sweep
# over the two shapes the lazy path serves, and the A/B of the fixed batch size
# the tree used before against the batch it derives now.
#
# The keys are `shape.limit` for a row of the sweep and `shape.candidate` for a
# row of the A/B, and the shape names are the probe's own.
BATCH_SIZE_CHECKS = [
    ("PERF.md", "the limit's scan, one column", "one column, one predicate", "limit", 131072, 2),
    (
        "PERF.md",
        "the limit's scan, the real Q6",
        "the real Q6's predicate set",
        "limit",
        131072,
        2,
    ),
    (
        "PERF.md",
        "the old batch, the real Q6",
        "the real Q6's predicate set",
        "candidate",
        65536,
        2,
    ),
    (
        "PERF.md",
        "the derived batch, the real Q6",
        "the real Q6's predicate set",
        "candidate",
        None,
        2,
    ),
]


def batch_size_figure(
    runs: list[dict], shape: str, kind: str, key: int | None
) -> float | None:
    """The median of one figure in the batch-size reports.

    `kind` is `limit` for a row of the sweep -- keyed by the limit -- and
    `candidate` for a row of the A/B -- keyed by the batch, or by the batch the
    tree derives now when the key is `None`.
    """
    values: list[float] = []
    for run in runs:
        for entry in run.get("shapes", []):
            if entry.get("shape") != shape:
                continue
            if kind == "limit":
                for limit in entry.get("limits", []):
                    if limit.get("limit") == key:
                        values.append(limit["scan_ms"])
            else:
                for candidate in entry.get("candidates", []):
                    batch = candidate.get("batch")
                    if key is None:
                        if batch == run.get("batch_in_the_tree"):
                            values.append(candidate["scan_ms"])
                    elif batch == key:
                        values.append(candidate["scan_ms"])
    return statistics.median(values) if values else None


# Spec 016's cost probe: the same instrument on the three files, in one
# session. The published figures are the ladder's two rows -- the fixed batch
# size the tree used and the batch it derives now -- and the attribution.
#
# The keys are `ladder.<batch>` for a row of the ladder and
# `measurements.<key>` for a row of the attribution.
ROW_GROUP_COST_CHECKS = [
    ("PERF.md", "the fixed batch, 6 groups", "ladder", 65536, "A", 2),
    ("PERF.md", "the fixed batch, 12 groups", "ladder", 65536, "B", 2),
    ("PERF.md", "the fixed batch, 24 groups", "ladder", 65536, "C", 2),
    ("PERF.md", "the derived batch, 6 groups", "ladder", 125000, "A", 2),
    ("PERF.md", "the derived batch, 12 groups", "ladder", 125000, "B", 2),
    ("PERF.md", "the derived batch, 24 groups", "ladder", 125000, "C", 2),
    ("PERF.md", "the crate's read of l_shipdate, 6 groups", "measurements",
     "crate_shipdate", "A", 2),
    ("PERF.md", "the crate's read of l_shipdate, 24 groups", "measurements",
     "crate_shipdate", "C", 2),
    ("PERF.md", "the engine's own scan, 6 groups", "measurements",
     "engine_predicates", "A", 2),
    ("PERF.md", "the engine's own scan, 24 groups", "measurements",
     "engine_predicates", "C", 2),
]

# The cost of one more row group, in milliseconds: the 24-group figure less the
# 6-group one, over the eighteen row groups between them.
ROW_GROUP_COST_SLOPE_CHECKS = [
    ("PERF.md", "the fixed batch's cost a row group", "ladder", 65536, 2),
    ("PERF.md", "the derived batch's cost a row group", "ladder", 125000, 2),
    ("PERF.md", "the crate's read of l_shipdate's cost a row group",
     "measurements", "crate_shipdate", 3),
]


def row_group_cost_figure(
    files: dict[str, list[dict]], kind: str, key: str | int, which: str
) -> float | None:
    """The median of one figure in the row-group cost reports."""
    values: list[float] = []
    for run in files.get(which, []):
        if kind == "ladder":
            for entry in run.get("ladder", []):
                if int(entry.get("batch", -1)) == int(key):
                    values.append(float(entry["scan_ms"]))
        else:
            measurements = run.get("measurements_ms", {})
            if key in measurements:
                values.append(float(measurements[key]))
    return statistics.median(values) if values else None


def row_group_cost_slope(
    files: dict[str, list[dict]], kind: str, key: str | int
) -> float | None:
    """The cost of one more row group, in milliseconds."""
    first = row_group_cost_figure(files, kind, key, "A")
    last = row_group_cost_figure(files, kind, key, "C")
    if first is None or last is None:
        return None
    return (last - first) / 18.0



# The x86-64 kernel A/B. Only the **ratios** are published: the absolute
# figures describe a shared runner and the repository does not publish a
# number it cannot reproduce.
#
# The ratios are published **per processor**, and checked against the line
# that names that processor: GitHub's hosted runners are a fleet, and the
# ratio is a property of the CPU as much as of the kernel, so a pooled
# median across six different machines would be a number about no machine.
AVX2_CHECKS = [
    ("cpp-simd", "ratio_vs_scalar"),
    ("cpp-simd", "ratio_vs_auto"),
    ("rust-simd", "ratio_vs_scalar"),
    ("rust-simd", "ratio_vs_auto"),
]

# §1's table: the ARM64 kernel A/B, as the median of its campaigns. The
# divisor takes rows/sec down to the G the document publishes.
KERNEL_CHECKS = [
    ("scalar", "rows_per_sec", 2, 1e9),
    ("cpp-auto", "rows_per_sec", 2, 1e9),
    ("cpp-simd", "rows_per_sec", 2, 1e9),
    ("rust-simd", "rows_per_sec", 2, 1e9),
    ("scalar", "ns_per_row", 3, 1.0),
    ("cpp-auto", "ns_per_row", 3, 1.0),
    ("cpp-simd", "ns_per_row", 3, 1.0),
    ("rust-simd", "ns_per_row", 3, 1.0),
    ("cpp-auto", "ratio_vs_scalar", 2, 1.0),
    ("cpp-simd", "ratio_vs_scalar", 2, 1.0),
    ("rust-simd", "ratio_vs_scalar", 2, 1.0),
    ("cpp-simd", "ratio_vs_auto", 2, 1.0),
    ("rust-simd", "ratio_vs_auto", 2, 1.0),
]


def kernel_figure(runs: list[dict], flavour: str, key: str) -> float | None:
    """The median of one figure in the ARM64 kernel reports."""
    values: list[float] = []
    for run in runs:
        for entry in run.get("kernel_ab", {}).get("flavors", []):
            if entry.get("flavor") == flavour and key in entry:
                values.append(float(entry[key]))
    return statistics.median(values) if values else None


def avx2_row_mentions(text: str, cpu: str, rendered: str) -> bool:
    """Whether the row that names a processor also carries a figure.

    The x86-64 table has one row per processor, and the ratio is a property
    of the processor. A figure that appears *somewhere* in the document is
    not the same claim as that figure appearing on the processor's own row,
    so the check is made against the row and not against the whole text.
    """
    for line in text.splitlines():
        if cpu not in line:
            continue
        for sign in ("x", "×"):
            if f"{rendered}{sign}" in line:
                return True
            if f"{rendered} G" in line or f"{rendered} ms" in line:
                return True
    return False


def avx2_groups(runs: list[dict]) -> dict[str, list[dict]]:
    """Group the x86-64 reports by the processor that produced them."""
    groups: dict[str, list[dict]] = {}
    for run in runs:
        cpu = run.get("machine", {}).get("cpu", "?")
        groups.setdefault(cpu, []).append(run)
    return groups


def avx2_figure(runs: list[dict], flavour: str, key: str) -> float | None:
    """The median of one ratio in the x86-64 reports."""
    values: list[float] = []
    for run in runs:
        for entry in run.get("kernel_ab", {}).get("flavors", []):
            if entry.get("flavor") == flavour and key in entry:
                values.append(float(entry[key]))
    return statistics.median(values) if values else None


# Spec 022's dictionary probe: the same read in two shapes -- the reader
# expanding the dictionary-encoded columns, and the reader keeping them --
# with the engine's own comparison measured on top of each.
#
# The keys are `measurements.<key>` for a row of the shapes, `columns.<key>`
# for a row of the per-column table, and `dictionary.<key>` for the
# alternative's own shape. The `before` set is the same probe against the code
# the increment replaced, measured back to back on the same host.
DICTIONARY_COST_CHECKS = [
    ("PERF.md", "the crate's read, expanded, 6 groups", "measurements",
     "crate_expanded", "A", 2),
    ("PERF.md", "the crate's read, expanded, 12 groups", "measurements",
     "crate_expanded", "B", 2),
    ("PERF.md", "the crate's read, expanded, 24 groups", "measurements",
     "crate_expanded", "C", 2),
    ("PERF.md", "the crate's read, kept, 6 groups", "measurements",
     "crate_kept", "A", 2),
    ("PERF.md", "the crate's read, kept, 24 groups", "measurements",
     "crate_kept", "C", 2),
    ("PERF.md", "the engine's read, over the values, 6 groups", "measurements",
     "engine_expanded", "A", 2),
    ("PERF.md", "the engine's read, over the values, 24 groups", "measurements",
     "engine_expanded", "C", 2),
    ("PERF.md", "the engine's read, over the distinct, 6 groups", "measurements",
     "engine_kept", "A", 2),
    ("PERF.md", "the engine's own read, 6 groups", "measurements",
     "engine_read", "A", 2),
    ("PERF.md", "l_quantity, expanded", "columns", "crate_expanded_quantity", "A", 2),
    ("PERF.md", "l_quantity, kept", "columns", "crate_kept_quantity", "A", 2),
    ("PERF.md", "l_discount, expanded", "columns", "crate_expanded_discount", "A", 2),
    ("PERF.md", "l_discount, kept", "columns", "crate_kept_discount", "A", 2),
    ("PERF.md", "l_shipdate, expanded", "columns", "crate_expanded_shipdate", "A", 2),
    ("PERF.md", "l_shipdate, kept", "columns", "crate_kept_shipdate", "A", 2),
    ("PERF.md", "the distinct values, compared, 6 groups", "dictionary",
     "distinct", "A", 2),
    ("PERF.md", "the map, 6 groups", "dictionary", "map", "A", 2),
]

DICTIONARY_COST_BEFORE_CHECKS = [
    ("PERF.md", "the old crate's read, expanded, 6 groups", "measurements",
     "crate_expanded", "A", 2),
    ("PERF.md", "the old crate's read, expanded, 24 groups", "measurements",
     "crate_expanded", "C", 2),
    ("PERF.md", "the old crate's read, kept, 6 groups", "measurements",
     "crate_kept", "A", 2),
    ("PERF.md", "the old engine's read, over the values, 6 groups", "measurements",
     "engine_expanded", "A", 2),
    ("PERF.md", "the old engine's read, over the values, 24 groups", "measurements",
     "engine_expanded", "C", 2),
    ("PERF.md", "the old engine's read, over the distinct, 6 groups", "measurements",
     "engine_kept", "A", 2),
    ("PERF.md", "the old engine's own read, 6 groups", "measurements",
     "engine_read", "A", 2),
]

# The engine's own cost over the crate's read: the difference between the two
# shapes, and the rate of each over the eighteen row groups between the
# six-row-group file and the twenty-four-row-group one.
DICTIONARY_COST_DERIVED_CHECKS = [
    ("PERF.md", "the engine's comparison, over the values", "comparison", "A", 2),
    ("PERF.md", "the old engine's comparison, over the values", "comparison", "A", 2),
]

DICTIONARY_COST_SLOPE_CHECKS = [
    ("PERF.md", "the crate's read's cost a row group", "crate_expanded", 2),
    ("PERF.md", "the engine's comparison's cost a row group", "comparison", 2),
]

# Spec 023's scan probe: the join's scan, taken apart. The engine's copy is
# the difference between the engine's shape and the crate's shape over the
# same two columns.
SCAN_COST_CHECKS = [
    ("PERF.md", "the crate's read of l_orderkey", "crate_orderkey", 2),
    ("PERF.md", "the crate's read of l_extendedprice", "crate_price", 2),
    ("PERF.md", "the crate's read of both", "crate_both", 2),
    ("PERF.md", "the engine's own scan of l_orderkey", "engine_orderkey", 2),
    ("PERF.md", "the engine's own scan of l_extendedprice", "engine_price", 2),
    ("PERF.md", "the engine's own scan of both", "engine_both", 2),
    ("PERF.md", "the crate's read of the build side", "crate_orders_both", 2),
]

SCAN_COST_DERIVED_CHECKS = [
    ("PERF.md", "the engine's copy", 2),
]


def dictionary_cost_figure(
    files: dict[str, list[dict]], kind: str, key: str, which: str
) -> float | None:
    """The median of one figure in the dictionary cost reports."""
    values: list[float] = []
    for run in files.get(which, []):
        section = run.get(f"{kind}_ms", {})
        if key in section:
            values.append(float(section[key]))
    return statistics.median(values) if values else None


def dictionary_cost_comparison(files: dict[str, list[dict]], which: str) -> float | None:
    """The engine's own cost over the crate's read."""
    expanded = dictionary_cost_figure(files, "measurements", "engine_expanded", which)
    crate = dictionary_cost_figure(files, "measurements", "crate_expanded", which)
    if expanded is None or crate is None:
        return None
    return expanded - crate


def dictionary_cost_slope(
    files: dict[str, list[dict]], kind: str, first: str, last: str
) -> float | None:
    """The cost of one more row group, in milliseconds."""
    span = 18
    if kind == "comparison":
        low = dictionary_cost_comparison(files, first)
        high = dictionary_cost_comparison(files, last)
    else:
        low = dictionary_cost_figure(files, "measurements", kind, first)
        high = dictionary_cost_figure(files, "measurements", kind, last)
    if low is None or high is None:
        return None
    return (high - low) / span


def scan_cost_figure(runs: list[dict], key: str) -> float | None:
    """The median of one figure in the scan cost reports."""
    values = [
        float(run["measurements_ms"][key])
        for run in runs
        if key in run.get("measurements_ms", {})
    ]
    return statistics.median(values) if values else None


def scan_cost_copy(runs: list[dict]) -> float | None:
    """The engine's copy: its own scan over the crate's read."""
    engine = scan_cost_figure(runs, "engine_both")
    crate = scan_cost_figure(runs, "crate_both")
    if engine is None or crate is None:
        return None
    return engine - crate


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
    # The typographic minus sign is normalised to the ASCII one, so that a
    # negative figure can be checked the way every other figure is.
    texts = {doc.name: doc.read_text().replace("\u2212", "-") for doc in DOCS}

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

    if BEFORE_REPORTS:
        before_runs = [json.loads(path.read_text()) for path in BEFORE_REPORTS]
        before = collect(before_runs)
        for doc, label, system, mode, metric, query, places in BEFORE_CHECKS:
            values = before.get((system, mode, metric, query))
            if not values:
                failures.append(f"{doc}: no figure for {label} in the before reports")
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

    if SPEC021_REPORTS:
        spec021_runs = [json.loads(path.read_text()) for path in SPEC021_REPORTS]
        last = collect(spec021_runs)
        for doc, label, system, mode, metric, query, places in SPEC021_CHECKS:
            values = last.get((system, mode, metric, query))
            if not values:
                failures.append(f"{doc}: no figure for {label} in the last set's reports")
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

    if SPEC023_REPORTS:
        spec023_runs = [json.loads(path.read_text()) for path in SPEC023_REPORTS]
        replaced = collect(spec023_runs)
        for doc, label, system, mode, metric, query, places in SPEC023_CHECKS:
            values = replaced.get((system, mode, metric, query))
            if not values:
                failures.append(f"{doc}: no figure for {label} in the replaced set's reports")
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

    if QUERY_COST_REPORTS or QUERY_COST_BEFORE_REPORTS:
        query_cost_shapes = {
            "after": _query_cost_shapes(QUERY_COST_REPORTS),
            "before": _query_cost_shapes(QUERY_COST_BEFORE_REPORTS),
        }
        query_cost_queries = {
            "after": _query_cost_queries(QUERY_COST_REPORTS),
            "before": _query_cost_queries(QUERY_COST_BEFORE_REPORTS),
        }
        for doc, label, which, shape, places in QUERY_COST_SHAPE_CHECKS:
            values = query_cost_shapes[which].get(shape, [])
            if not values:
                failures.append(f"{doc}: no figure for {label} in the query probe's reports")
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
        for doc, label, which, path, query, key, places in QUERY_COST_CHECKS:
            values = query_cost_queries[which].get(path, {}).get(query, {}).get(key, [])
            if not values:
                failures.append(f"{doc}: no figure for {label} in the query probe's reports")
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

    stage_sets: dict[str, dict[tuple[str, str, str], list[float]]] = {}
    if REPORTS:
        stage_sets["new"] = collect_stages(runs)
    if BEFORE_REPORTS:
        stage_sets["before"] = collect_stages(before_runs)
    if SPEC021_REPORTS:
        stage_sets["last"] = collect_stages(spec021_runs)
    if SPEC023_REPORTS:
        stage_sets["replaced"] = collect_stages(spec023_runs)

    for doc, label, which, query, stage, places in STAGE_CHECKS:
        values = stage_sets.get(which, {}).get((query, "lazy", stage))
        if not values:
            failures.append(f"{doc}: no figure for {label} in the {which} reports")
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

    if PROBE_REPORTS:
        probes = {
            path.stem.removeprefix("operator-probe-"): json.loads(path.read_text())
            for path in PROBE_REPORTS
        }
        for doc, label, report, measurement, places in PROBE_CHECKS:
            run = probes.get(report)
            values = []
            if run is not None:
                section = run.get("join_probe_ms", {})
                if measurement in section:
                    values.append(section[measurement])
                section = run.get("sort_ms", {})
                if measurement in section:
                    values.append(section[measurement])
            if not values:
                failures.append(f"{doc}: no figure for {label} in the probe reports")
                continue
            figure = values[0]
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

    # Spec 016: the reader's batch limit, swept over the two shapes the lazy
    # path serves.
    batch_runs = load_reports("batch-size-run*.json")
    if batch_runs:
        for doc, label, shape, kind, key, places in BATCH_SIZE_CHECKS:
            figure = batch_size_figure(batch_runs, shape, kind, key)
            if figure is None:
                failures.append(f"{doc}: no figure for {label} in the batch-size reports")
                continue
            rendered = f"{figure:.{places}f}"
            if rendered not in texts[doc]:
                failures.append(
                    f"{doc}: {label} is {rendered} in the reports but that figure "
                    f"does not appear in the document"
                )
            else:
                print(f"ok  {doc:<10} {label:<40} {rendered}")

    # Spec 016's cost probe: the ladder, and the attribution, on the three
    # files.
    cost_files = {
        "A": load_reports("row-group-cost-6-run*.json"),
        "B": load_reports("row-group-cost-12-run*.json"),
        "C": load_reports("row-group-cost-24-run*.json"),
    }
    avx2_runs = load_reports("avx2-run*.json")
    if avx2_runs:
        for cpu, group in sorted(avx2_groups(avx2_runs).items()):
            campaigns = len(group)
            if campaigns < MIN_CAMPAIGNS:
                print(f"skip {cpu:<44} {campaigns} campaign(s): fewer than "
                      f"{MIN_CAMPAIGNS}, so no median is published")
                continue
            for flavour, key in AVX2_CHECKS:
                figure = avx2_figure(group, flavour, key)
                if figure is None:
                    failures.append(
                        f"PERF.md: no figure for {flavour} {key} in the "
                        f"x86-64 reports from {cpu}")
                    continue
                rendered = f"{figure:.2f}"
                label = f"{flavour} {key} on {cpu}"
                if not avx2_row_mentions(texts["PERF.md"], cpu, rendered):
                    failures.append(
                        f"PERF.md: {label} is {rendered} in the reports but that "
                        f"figure does not appear on the row that names {cpu}"
                    )
                else:
                    print(f"ok  PERF.md   {label:<56} {rendered}")

    kernel_runs = load_reports("kernel-run*.json")
    if kernel_runs:
        for flavour, key, places, divisor in KERNEL_CHECKS:
            figure = kernel_figure(kernel_runs, flavour, key)
            if figure is None:
                failures.append(
                    f"PERF.md: no figure for {flavour} {key} in the kernel reports")
                continue
            rendered = f"{figure / divisor:.{places}f}"
            if rendered not in texts["PERF.md"]:
                failures.append(
                    f"PERF.md: {flavour} {key} is {rendered} in the reports but "
                    f"that figure does not appear in the document"
                )
            else:
                print(f"ok  PERF.md   {flavour + ' ' + key:<56} {rendered}")

    if any(cost_files.values()):
        for doc, label, kind, key, which, places in ROW_GROUP_COST_CHECKS:
            figure = row_group_cost_figure(cost_files, kind, key, which)
            if figure is None:
                failures.append(f"{doc}: no figure for {label} in the cost reports")
                continue
            rendered = f"{figure:.{places}f}"
            if rendered not in texts[doc]:
                failures.append(
                    f"{doc}: {label} is {rendered} in the reports but that figure "
                    f"does not appear in the document"
                )
            else:
                print(f"ok  {doc:<10} {label:<46} {rendered}")

        for doc, label, kind, key, places in ROW_GROUP_COST_SLOPE_CHECKS:
            figure = row_group_cost_slope(cost_files, kind, key)
            if figure is None:
                failures.append(f"{doc}: no figure for {label} in the cost reports")
                continue
            rendered = f"{figure:.{places}f}"
            if rendered not in texts[doc]:
                failures.append(
                    f"{doc}: {label} is {rendered} in the reports but that figure "
                    f"does not appear in the document"
                )
            else:
                print(f"ok  {doc:<10} {label:<46} {rendered}")

    # Spec 022's dictionary probe: the two shapes of the read, the new code
    # and the old one.
    dictionary_files = {
        "A": load_reports("dictionary-cost-6-run*.json"),
        "B": load_reports("dictionary-cost-12-run*.json"),
        "C": load_reports("dictionary-cost-24-run*.json"),
    }
    dictionary_before = {
        "A": load_reports("dictionary-cost-before-6-run*.json"),
        "B": load_reports("dictionary-cost-before-12-run*.json"),
        "C": load_reports("dictionary-cost-before-24-run*.json"),
    }
    if any(dictionary_files.values()):
        for doc, label, kind, key, which, places in DICTIONARY_COST_CHECKS:
            figure = dictionary_cost_figure(dictionary_files, kind, key, which)
            if figure is None:
                failures.append(f"{doc}: no figure for {label} in the dictionary reports")
                continue
            rendered = f"{figure:.{places}f}"
            if rendered not in texts[doc]:
                failures.append(
                    f"{doc}: {label} is {rendered} in the reports but that figure "
                    f"does not appear in the document"
                )
            else:
                print(f"ok  {doc:<10} {label:<46} {rendered}")

        for doc, label, kind, key, which, places in DICTIONARY_COST_BEFORE_CHECKS:
            figure = dictionary_cost_figure(dictionary_before, kind, key, which)
            if figure is None:
                failures.append(f"{doc}: no figure for {label} in the before reports")
                continue
            rendered = f"{figure:.{places}f}"
            if rendered not in texts[doc]:
                failures.append(
                    f"{doc}: {label} is {rendered} in the reports but that figure "
                    f"does not appear in the document"
                )
            else:
                print(f"ok  {doc:<10} {label:<46} {rendered}")

        for doc, label, kind, which, places in DICTIONARY_COST_DERIVED_CHECKS:
            files = dictionary_before if "old" in label else dictionary_files
            figure = dictionary_cost_comparison(files, which)
            if figure is None:
                failures.append(f"{doc}: no figure for {label} in the dictionary reports")
                continue
            rendered = f"{figure:.{places}f}"
            if rendered not in texts[doc]:
                failures.append(
                    f"{doc}: {label} is {rendered} in the reports but that figure "
                    f"does not appear in the document"
                )
            else:
                print(f"ok  {doc:<10} {label:<46} {rendered}")

        for doc, label, kind, places in DICTIONARY_COST_SLOPE_CHECKS:
            figure = dictionary_cost_slope(dictionary_files, kind, "A", "C")
            if figure is None:
                failures.append(f"{doc}: no figure for {label} in the dictionary reports")
                continue
            rendered = f"{figure:.{places}f}"
            if rendered not in texts[doc]:
                failures.append(
                    f"{doc}: {label} is {rendered} in the reports but that figure "
                    f"does not appear in the document"
                )
            else:
                print(f"ok  {doc:<10} {label:<46} {rendered}")

    # Spec 023's scan probe: the join's scan, taken apart.
    scan_runs = load_reports("scan-cost-run*.json")
    if scan_runs:
        for doc, label, key, places in SCAN_COST_CHECKS:
            figure = scan_cost_figure(scan_runs, key)
            if figure is None:
                failures.append(f"{doc}: no figure for {label} in the scan reports")
                continue
            rendered = f"{figure:.{places}f}"
            if rendered not in texts[doc]:
                failures.append(
                    f"{doc}: {label} is {rendered} in the reports but that figure "
                    f"does not appear in the document"
                )
            else:
                print(f"ok  {doc:<10} {label:<46} {rendered}")

        for doc, label, places in SCAN_COST_DERIVED_CHECKS:
            figure = scan_cost_copy(scan_runs)
            if figure is None:
                failures.append(f"{doc}: no figure for {label} in the scan reports")
                continue
            rendered = f"{figure:.{places}f}"
            if rendered not in texts[doc]:
                failures.append(
                    f"{doc}: {label} is {rendered} in the reports but that figure "
                    f"does not appear in the document"
                )
            else:
                print(f"ok  {doc:<10} {label:<46} {rendered}")

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

    # The README's **Next** row is prose: a hand-written summary of the next
    # question, and the one place where a figure goes stale without a table
    # noticing it. The figures it quotes are checked *inside the row*, and they
    # are built from the reports rather than typed here.
    next_row = next(
        (line for line in texts["README.md"].splitlines() if line.startswith("| Next |")),
        "",
    )
    if next_row and REPORTS:
        figures = collect(runs)
        engine_fr = figures.get(("engine", "lazy", "cold", "filter-range"), [])
        polars_fr = figures.get(("polars", "-", "cold", "filter-range"), [])
        wanted: list[tuple[str, str]] = []
        if engine_fr and polars_fr:
            wanted.append(
                (
                    "the Next row's filter-range pair",
                    f"{statistics.median(engine_fr):.1f} ms against Polars' "
                    f"{statistics.median(polars_fr):.1f} ms",
                )
            )
            wanted.append(
                (
                    "the Next row's filter-range spreads",
                    f"({min(engine_fr):.1f}–{max(engine_fr):.1f} ms against "
                    f"{min(polars_fr):.1f}–{max(polars_fr):.1f} ms)",
                )
            )
        probe_first = load_reports(dict(ROW_GROUP_PROBES)["A"])
        probe_last = load_reports(dict(ROW_GROUP_PROBES)["C"])
        if probe_first and probe_last:
            first = materialised_figure(probe_first, "query.predicates_only")
            last = materialised_figure(probe_last, "query.predicates_only")
            if first is not None and last is not None:
                wanted.append(
                    (
                        "the Next row's three-predicate residual",
                        f"{(last - first) / 18.0:.2f} ms a row group",
                    )
                )
        for label, phrase in wanted:
            if phrase not in next_row:
                failures.append(
                    f"README.md: the Next row does not carry {label}: the reports say {phrase}"
                )
            else:
                print(f"ok  README.md  {label:<40} {phrase}")

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

    print(
        f"\nall published figures agree with the committed reports: "
        f"{len(REPORTS)} campaigns, {len(BEFORE_REPORTS)} before, "
        f"{len(SPEC023_REPORTS)} of the replaced set, "
        f"{len(SPEC021_REPORTS)} of the last set, {len(PROBE_REPORTS)} probe, "
        f"{len(STAGE_CHECKS)} of the plan output, "
        f"{len(QUERY_COST_CHECKS) + len(QUERY_COST_SHAPE_CHECKS)} of the query probe"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
