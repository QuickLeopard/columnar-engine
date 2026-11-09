#!/usr/bin/env python3
"""Reduce one or more `compare` reports to the published medians.

The engine's published comparison table is **the median of several full
`just compare` runs**, because this machine is shared and a single run is not
a reliable estimate of anything. This script is the derivation: it takes the
raw reports in `bench-results/` and prints the table that `PERF.md` publishes,
together with the spread of each figure, so the prose and the evidence cannot
drift apart.

Usage:

    python3 scripts/medians.py bench-results/compare-run*.json

Each input is one `columnar-cli compare --out <file>` report. The script
reads nothing but the reports; it measures nothing itself.

The **thread-count sweep** is derived the same way, from the
`scaling-run*.json` reports, which are picked up automatically when they are
in `bench-results/`.
"""

from __future__ import annotations

import json
import statistics
import sys
from pathlib import Path

QUERIES = [
    "scan",
    "filter-numeric",
    "filter-string",
    "filter-range",
    "q6",
    "q6-revenue",
    "join-orders",
    "sort-topk",
]
COMPARATORS = ["duckdb", "polars", "datafusion-peer"]
DISPLAY = {"duckdb": "duckdb", "polars": "polars", "datafusion-peer": "datafusion"}
ENGINE_MODES = ["eager", "lazy"]

# The number of campaigns a machine has to run before its median is published.
# This is the repository's rule, not a script's: a single run is an estimate
# of a number that moves, so a median needs at least three.
MIN_CAMPAIGNS = 3


def load(path: Path) -> dict:
    with path.open() as handle:
        return json.load(handle)


def median(values: list[float]) -> float:
    return statistics.median(values)


def collect(runs: list[dict]) -> dict[tuple[str, str, str, str], list[float]]:
    """Gather every figure from every run, keyed by (system, mode, metric, query).

    `metric` is one of `cold`, `resident` or `load`. The engine's `eager` mode
    has all three: its resident time is the query against the already
    materialised dataset, its cold time is the read plus the query, and its
    load is the decode itself. The `lazy` path has no resident form -- it
    reads the file during the query -- so its cold and resident figures are
    the same number, by construction.
    """
    out: dict[tuple[str, str, str, str], list[float]] = {}

    def push(key: tuple[str, str, str, str], value: float | None) -> None:
        if value is not None:
            out.setdefault(key, []).append(value)

    for run in runs:
        for entry in run.get("engine_queries", []):
            query, mode = entry["query"], entry["mode"]
            push(("engine", mode, "cold", query), entry.get("cold_ms"))
            push(("engine", mode, "resident", query), entry.get("elapsed_ms"))
            push(("engine", mode, "load", query), entry.get("load_ms"))

        for entry in run.get("external", []):
            query, comparator, mode = (
                entry["query"],
                entry["comparator"],
                entry["mode"],
            )
            push((comparator, "-", mode, query), entry.get("query_ms"))

    # The incumbents' load is reported as the WORST load across the queries,
    # which is what the comparison report prints, so the derived table and the
    # report cannot disagree about it.
    for run in runs:
        for comparator in COMPARATORS:
            loads = [
                entry["load_ms"]
                for entry in run.get("external", [])
                if entry["comparator"] == comparator and entry["mode"] == "resident"
            ]
            if loads:
                push((comparator, "-", "load", "worst"), max(loads))

    return out


def cell(values: list[float] | None) -> str:
    """`median (min-max)`, right-aligned, or a dash when there is no figure."""
    if not values:
        return f"{'-':>26}"
    return f"{median(values):>16.1f} ({min(values):.1f}-{max(values):.1f})".rjust(26)


def collect_scaling(runs: list[dict]) -> dict[int, dict[str, list[float]]]:
    """Gather the sweep's figures, keyed by thread count.

    Each run contributes one measurement per thread count per system, so the
    median of the runs is the sweep's published figure -- the same rule the
    headline table follows, for the same reason: this machine is shared.
    """
    out: dict[int, dict[str, list[float]]] = {}

    def push(threads: int, key: str, value: float | None) -> None:
        if value is not None:
            out.setdefault(threads, {}).setdefault(key, []).append(value)

    for run in runs:
        for point in run.get("points", []):
            threads = point["threads"]
            push(threads, "workers", point.get("workers"))
            push(threads, "filter_workers", point.get("filter_workers"))
            push(threads, "projection_workers", point.get("projection_workers"))
            push(threads, "engine", point.get("engine_ms"))
            push(threads, "engine_scan", point.get("engine_scan_ms"))
            push(threads, "engine_operators", point.get("engine_operators_ms"))
            push(threads, "duckdb", point.get("duckdb_ms"))
            push(threads, "polars", point.get("polars_ms"))
            push(threads, "datafusion", point.get("datafusion_ms"))

    return out


def worker_cell(values: list[float] | None) -> str:
    """A worker count, right-aligned, or a dash when there is none.

    A missing count is a dash and never a zero: a mode that ran no query did
    not do the work with zero workers.
    """
    if not values:
        return f"{'-':>8}"
    return f"{median(values):>8.0f}"


def print_scaling(runs: list[dict], label: str = "q6", mode: str = "cold") -> None:
    """Print the thread-count sweep's medians."""
    if not runs:
        return
    sweep = collect_scaling(runs)
    if not sweep:
        return

    # The file and its row-group count are printed with the table, because
    # there is more than one sweep now and the row-group count is the cap on
    # the engine's parallelism -- a reader who does not see it cannot read the
    # curve.
    dataset = runs[0].get("dataset", {})
    row_groups = runs[0].get("row_groups")

    print(f"\n=== THREAD-COUNT SWEEP, {label} ({mode}, {len(runs)} runs) ===")
    print(
        f"file     {dataset.get('path', '?')} · {dataset.get('rows', '?')} rows · "
        f"{row_groups if row_groups is not None else '?'} row groups"
    )
    print(
        f"{'threads':>8}{'read':>8}{'filter':>8}{'proj':>8}{'engine':>26}{'engine scan':>26}"
        f"{'engine ops':>26}{'duckdb':>26}{'polars':>26}{'datafusion':>26}"
    )
    for threads in sorted(sweep):
        figures = sweep[threads]
        workers = figures.get("workers", [0])
        row = (
            f"{threads:>8}"
            f"{median(workers):>8.0f}"
            f"{worker_cell(figures.get('filter_workers'))}"
            f"{worker_cell(figures.get('projection_workers'))}"
            f"{cell(figures.get('engine'))}"
            f"{cell(figures.get('engine_scan'))}"
            f"{cell(figures.get('engine_operators'))}"
            f"{cell(figures.get('duckdb'))}"
            f"{cell(figures.get('polars'))}"
            f"{cell(figures.get('datafusion'))}"
        )
        print(row)

    base = sweep.get(1, {})
    if base.get("engine") and base.get("duckdb"):
        print("\n-- from the medians --")
        base_engine = median(base["engine"])
        base_duckdb = median(base["duckdb"])
        for threads in sorted(sweep):
            figures = sweep[threads]
            if not figures.get("engine"):
                continue
            engine = median(figures["engine"])
            duckdb = median(figures["duckdb"]) if figures.get("duckdb") else float("nan")
            print(
                f"{threads:>8} threads  engine {base_engine / engine:5.2f}x  "
                f"duckdb {base_duckdb / duckdb:5.2f}x  "
                f"engine / duckdb {engine / duckdb:5.2f}x"
            )


def print_row_groups(
    files: list[tuple[str, list[dict]]], label: str = "q6", mode: str = "cold"
) -> None:
    """Print the sweeps of every file, side by side.

    The claim under test is that the engine's parallelism is capped by the
    **file's** row-group count rather than by the machine, so the table that
    settles it is the one that puts the files' curves beside each other: the
    worker count asked for against the worker count that ran, and the speedup
    each file's curve achieved.

    The incumbents are printed too, and they are the control: they are **not**
    capped by the row-group count, so their numbers are the same on every
    file up to the machine's noise. A reader who sees the engine's curve move
    with the row-group count and the incumbents' not has seen the effect
    separated from the noise.
    """
    sweeps = [(name, collect_scaling(runs)) for name, runs in files]
    sweeps = [(name, sweep) for name, sweep in sweeps if sweep]
    if not sweeps:
        return

    print(f"\n=== THE ROW-GROUP CAP, {label} ({mode}, {len(files[0][1])} runs each) ===")
    for name, runs in files:
        print(
            f"file {name}  {runs[0].get('dataset', {}).get('path', '?')} "
            f"· {runs[0].get('row_groups')} row groups"
        )

    def figure(sweep: dict, threads: int, key: str) -> float | None:
        values = sweep.get(threads, {}).get(key)
        return median(values) if values else None

    def speedup(sweep: dict, threads: int, key: str) -> float:
        base = figure(sweep, 1, key)
        now = figure(sweep, threads, key)
        if not base or not now or now <= 0:
            return float("nan")
        return base / now

    def shown(value: float | None, width: int = 8) -> str:
        if value is None or value != value:  # noqa: PLR0124 -- NaN is missing
            return format("-", f">{width}")
        return f"{value:.1f}".rjust(width)

    threads_list = sorted({threads for _, sweep in sweeps for threads in sweep})

    print("\n-- the engine, and the cap --")
    header = f"{'threads':>8}"
    for name, _ in sweeps:
        header += f"{name + ' workers':>16}{name + ' engine':>14}{name + ' x':>10}"
    print(header)
    for threads in threads_list:
        row = f"{threads:>8}"
        for _, sweep in sweeps:
            workers = figure(sweep, threads, "workers")
            engine = figure(sweep, threads, "engine")
            row += (
                f"{(f'{workers:.0f}' if workers is not None else '-'):>16}"
                f"{shown(engine, 14):>14}"
                f"{speedup(sweep, threads, 'engine'):>9.2f}x"
            )
        print(row)

    print("\n-- the incumbents, the control --")
    header = f"{'threads':>8}"
    for name, _ in sweeps:
        header += f"{name + ' duckdb':>15}{name + ' polars':>14}{name + ' datafusion':>17}"
    print(header)
    for threads in threads_list:
        row = f"{threads:>8}"
        for _, sweep in sweeps:
            row += (
                f"{shown(figure(sweep, threads, 'duckdb'), 15):>15}"
                f"{shown(figure(sweep, threads, 'polars'), 14):>14}"
                f"{shown(figure(sweep, threads, 'datafusion'), 17):>17}"
            )
        print(row)


def print_browser(runs: list[dict]) -> None:
    """Print the browser's medians.

    The browser's report has a different shape from the engine's: it is one
    **campaign** of several rounds, each round a median over the same
    column. Every round of every campaign is a sample, so the published
    figure is the median over all of them, with the spread.

    The browser's number is not comparable with the native one, and this
    section never prints it next to the native table. What it does print is
    the one comparison the browser can make: the `simd128` path against the
    oracle, over the same data, in the same process.
    """
    if not runs:
        return

    oracle: list[float] = []
    simd: list[float] = []
    ratios: list[float] = []
    matches: set[int] = set()
    agree: set[bool] = set()
    browsers: set[str] = set()

    for run in runs:
        oracle.extend(run.get("oracle_ns_per_row", []))
        simd.extend(run.get("simd_ns_per_row", []))
        if run.get("ratio_median") is not None:
            ratios.append(run["ratio_median"])
        if run.get("matches") is not None:
            matches.add(run["matches"])
        if run.get("masks_agree") is not None:
            agree.add(run["masks_agree"])
        if run.get("browser"):
            browsers.add(run["browser"])

    print(f"\n=== BROWSER, {len(runs)} campaigns ===")
    print(f"{'flavor':<12}{'ns/row (median)':>28}{'rows/sec':>12}{'matches':>14}")
    for name, values in (("oracle", oracle), ("simd128", simd)):
        if not values:
            continue
        rate = 1e9 / median(values) / 1e9
        print(
            f"{name:<12}{f'{median(values):.3f} ({min(values):.3f}-{max(values):.3f})':>28}"
            f"{rate:>11.3f}G{str(sorted(matches)[0] if matches else '-'):>14}"
        )
    if ratios:
        print(
            f"\nsimd128 / oracle: {median(ratios):.2f}x "
            f"({min(ratios):.2f}-{max(ratios):.2f}) over {len(ratios)} campaigns"
        )
    print(f"masks identical in every campaign: {agree == {True}}")
    print(f"match count in every campaign:     {sorted(matches)}")
    for browser in sorted(browsers):
        print(f"browser: {browser}")


def print_rle_expansion(runs: list[dict]) -> None:
    """Print the RLE expansion's medians.

    This is spec 012's T3: what the expansion of six million RLE dictionary
    indices costs, against what it would cost not to expand them.

    Every figure is the median over the campaigns, with the spread. The
    derived figures are **derived from the medians**, never typed: the share
    of the decode that is the expansion, the share of the query it is, and
    the net of the increment the hypothesis proposes.
    """
    if not runs:
        return

    def figure(key: str) -> tuple[float, float, float]:
        values = [
            run["measurements_ms"][key]
            for run in runs
            if key in run.get("measurements_ms", {})
        ]
        if not values:
            return (0.0, 0.0, 0.0)
        return (statistics.median(values), min(values), max(values))

    rows = [
        ("P", "the parquet crate's decode", "parquet_decode"),
        ("A", "Arrow's decode", "arrow_decode"),
        ("PLAIN", "the same column, no dictionary", "plain_arrow_decode"),
        ("V", "the probe's decode", "probe_decode"),
        ("I", "the probe's indices, no gather", "probe_indices"),
        ("G", "the expansion", "expansion"),
        ("G1", "  the reader's fresh buffers", "expansion_buffers"),
        ("G2", "  the gather alone", "expansion_gather"),
        ("M", "the mask from the indices", "mask_from_indices"),
        ("C", "the hypothesis, end to end", "prototype"),
        ("S1", "the engine's scan of the column", "engine_scan"),
        ("S2", "the whole query", "engine_query"),
    ]

    print(f"\n=== THE RLE EXPANSION, {len(runs)} campaigns ===")
    print(f"{'':6}{'figure':<34}{'median (min-max)':>26}")
    for name, label, key in rows:
        median, low, high = figure(key)
        print(f"{name:<6}{label:<34}{f'{median:.2f} ({low:.2f}-{high:.2f})':>26}")

    predicate, _, _ = figure("engine_predicate")
    expansion, _, _ = figure("expansion")
    mask, _, _ = figure("mask_from_indices")
    scan, _, _ = figure("engine_scan")
    query, _, _ = figure("engine_query")
    parquet, _, _ = figure("parquet_decode")
    probe, _, _ = figure("probe_decode")
    decode = scan - predicate
    net_crate = expansion - (mask - predicate)
    net_probe = net_crate - (parquet - probe)

    print()
    print(f"{'the expansion share of that decode':<44}{expansion / decode * 100:.0f}%")
    print(f"{'the expansion share of the query':<44}{expansion / query * 100:.1f}%")
    print(f"{'the mask from the indices, less the predicate':<44}{mask - predicate:.2f} ms")
    print(f"{'the net, with the crate own RLE decoder':<44}{net_crate:.2f} ms")
    print(f"{'the net, with the probe RLE decoder':<44}{net_probe:.2f} ms")
    print(f"{'the host load average':<44}"
          f"{statistics.median([run.get('host_load_average', 0.0) for run in runs]):.1f}")


def print_materialised(runs: list[dict]) -> None:
    """Print the materialised column's medians.

    This is spec 014: the decode of the column the query **materialises**,
    against the decode of the column it only filters on, and against the
    counterfactual -- the same column, written the other way.

    Every figure is the median over the campaigns, with the spread. The
    derived figures are **derived from the medians**, never typed.
    """
    if not runs:
        return

    columns: dict[str, dict[str, list[float]]] = {}
    shapes: dict[str, dict] = {}
    for run in runs:
        for entry in run.get("columns", []):
            name = entry["name"]
            shapes.setdefault(name, entry)
            bucket = columns.setdefault(name, {})
            for key, value in entry.get("measurements_ms", {}).items():
                bucket.setdefault(key, []).append(value)

    def figure(name: str, key: str) -> tuple[float, float, float]:
        values = columns.get(name, {}).get(key, [])
        if not values:
            return (0.0, 0.0, 0.0)
        return (statistics.median(values), min(values), max(values))

    rows = [
        ("P", "the parquet crate's decode", "parquet_decode"),
        ("A", "Arrow's decode", "arrow_decode"),
        ("V", "the probe's decode", "probe_decode"),
        ("I", "the probe's indices, no gather", "probe_indices"),
        ("G", "the expansion", "expansion"),
        ("G1", "  the reader's fresh buffers", "expansion_buffers"),
        ("G2", "  the gather, or the copy", "expansion_gather"),
        ("X", "the counterfactual, parquet crate", "counterfactual_parquet_decode"),
        ("X", "the counterfactual, Arrow", "counterfactual_arrow_decode"),
    ]

    print(f"\n=== THE MATERIALISED COLUMN, {len(runs)} campaigns ===")
    for name, shape in shapes.items():
        print(f"\n-- {name} ({shape['role']}) --")
        print(
            f"   written {shape['encoding']}, "
            f"{shape['dictionary_pages']} dictionary pages and "
            f"{shape['plain_pages']} plain ones, "
            f"{shape['dictionary_values']} dictionary values, "
            f"{shape['bit_width']}-bit indices"
        )
        print(
            f"   {shape['compressed_bytes']:,} bytes on disk "
            f"(the counterfactual: {shape['counterfactual_style']}, "
            f"{shape['counterfactual_bytes']:,} bytes)"
        )
        print(f"{'':4}{'figure':<36}{'median (min-max)':>26}")
        for key, label, measurement in rows:
            if measurement not in columns.get(name, {}):
                continue
            median, low, high = figure(name, measurement)
            print(f"{key:<4}{label:<36}{f'{median:.2f} ({low:.2f}-{high:.2f})':>26}")

    query: dict[str, list[float]] = {}
    for run in runs:
        for key, value in run.get("query", {}).get("measurements_ms", {}).items():
            query.setdefault(key, []).append(value)

    if not query:
        return

    def q(key: str) -> float:
        return statistics.median(query.get(key, [0.0]))

    print("\n-- the real Q6 --")
    ladder = [
        ("Q0", "the predicates alone", "predicates_only"),
        ("Q1", "the predicates, and the price", "predicates_and_price"),
        ("Q2", "the predicates, and the discount", "predicates_and_discount"),
        ("Q3", "the predicates, and both", "predicates_and_both"),
        ("Q4", "the whole query", "q6_revenue"),
    ]
    for key, label, measurement in ladder:
        values = query.get(measurement, [])
        if not values:
            continue
        print(
            f"{key:<4}{label:<36}"
            f"{f'{statistics.median(values):.2f} ({min(values):.2f}-{max(values):.2f})':>26}"
        )
    # The shares are derived from the **campaigns**, not from the medians of
    # the medians: each campaign's share is measured, and the median of those
    # is the figure. The guard in `check-published-figures.py` derives them
    # the same way, and the two must agree.
    shares = [
        run["query"]["measurements_ms"]["predicates_and_price"]
        - run["query"]["measurements_ms"]["predicates_only"]
        for run in runs
        if "query" in run
    ]
    aggregates = [
        run["query"]["measurements_ms"]["q6_revenue"]
        - run["query"]["measurements_ms"]["predicates_and_both"]
        for run in runs
        if "query" in run
    ]
    print()
    print(f"{'the materialised column share of the query':<40}{statistics.median(shares):.2f} ms")
    print(f"{'the aggregate own share of the query':<40}{statistics.median(aggregates):.2f} ms")
    print(f"{'the scan stage':<40}{q('q6_revenue_scan'):.2f} ms")
    print(f"{'the operator stages':<40}{q('q6_revenue_operators'):.2f} ms")
    print(
        f"{'the host load average':<40}"
        f"{statistics.median([run.get('host_load_average', 0.0) for run in runs]):.1f}"
    )


def print_row_group_probe(files: list[tuple[str, list[dict]]]) -> None:
    """Print the files' shapes and their decodes, side by side.

    This is spec 015's confound: the row-group size is not only a parallelism
    knob, it changes the **file**. The writer's dictionary page limit is per
    row group, so a smaller row group carries a smaller dictionary and a
    different page mix, and the files are not the same shape.

    The files are therefore printed **together**, from one session's campaigns,
    so that a difference between them is a difference between the files and
    not a difference between two times on a shared machine.
    """
    if not files:
        return

    def shapes(runs: list[dict]) -> dict[str, dict]:
        out: dict[str, dict] = {}
        for run in runs:
            for entry in run.get("columns", []):
                out.setdefault(entry["name"], entry)
        return out

    def timings(runs: list[dict]) -> dict[str, dict[str, list[float]]]:
        out: dict[str, dict[str, list[float]]] = {}
        for run in runs:
            for entry in run.get("columns", []):
                bucket = out.setdefault(entry["name"], {})
                for key, value in entry.get("measurements_ms", {}).items():
                    bucket.setdefault(key, []).append(value)
        return out

    all_shapes = [shapes(runs) for _, runs in files]
    all_times = [timings(runs) for _, runs in files]

    def figure(times: dict, name: str, key: str, width: int = 24) -> str:
        values = times.get(name, {}).get(key, [])
        if not values:
            return format("-", f">{width}")
        return f"{statistics.median(values):.2f} ({min(values):.2f}-{max(values):.2f})".rjust(width)

    def shape_line(entry: dict) -> str:
        return (
            f"{entry['encoding']}, "
            f"{entry['dictionary_pages']} dictionary pages and "
            f"{entry['plain_pages']} plain ones, "
            f"{entry['dictionary_values']} dictionary values, "
            f"{entry['bit_width']}-bit indices"
        )

    groups = [runs[0].get("row_groups") for _, runs in files]
    print(f"\n=== THE ROW-GROUP FILE, {len(files[0][1])} campaigns each ===")
    for (name, runs), group in zip(files, groups):
        print(f"file {name}  {runs[0].get('file', '?')} \u00b7 {group} row groups")

    names = list(all_shapes[0].keys())
    print("\n-- the shape --")
    for column in names:
        first = all_shapes[0].get(column)
        if first is None:
            continue
        print(f"\n{column} ({first['role']})")
        for (name, _), file_shapes in zip(files, all_shapes):
            entry = file_shapes.get(column)
            if entry is None:
                continue
            print(f"   {name}  {shape_line(entry)}")
        for (name, _), file_shapes in zip(files, all_shapes):
            entry = file_shapes.get(column)
            if entry is None:
                continue
            print(
                f"   {name}  {entry['compressed_bytes']:,} bytes on disk "
                f"({entry['dictionary_page_bytes']:,} dictionary-page, "
                f"{entry['plain_data_bytes']:,} plain-data)"
            )

    print("\n-- the decode --")
    header = f"{'':4}{'figure':<34}"
    for name, group in zip((name for name, _ in files), groups):
        header += f"{name + ', ' + str(group) + ' groups':>24}"
    print(header)
    rows = [
        ("P", "the parquet crate's decode", "parquet_decode"),
        ("A", "Arrow's decode", "arrow_decode"),
        ("V", "the probe's decode", "probe_decode"),
    ]
    for column in names:
        print(f"\n{column}")
        for key, label, measurement in rows:
            if not any(measurement in times.get(column, {}) for times in all_times):
                continue
            row = f"{key:<4}{label:<34}"
            for times in all_times:
                row += figure(times, column, measurement)
            print(row)

    def ladder(runs: list[dict]) -> dict[str, list[float]]:
        out: dict[str, list[float]] = {}
        for run in runs:
            for key, value in run.get("query", {}).get("measurements_ms", {}).items():
                out.setdefault(key, []).append(value)
        return out

    all_ladders = [ladder(runs) for _, runs in files]

    print("\n-- the real Q6, as a ladder --")
    header = f"{'':4}{'rung':<34}"
    for name, group in zip((name for name, _ in files), groups):
        header += f"{name + ', ' + str(group) + ' groups':>24}"
    print(header)
    rungs = [
        ("Q0", "the predicates alone", "predicates_only"),
        ("Q1", "the predicates, and the price", "predicates_and_price"),
        ("Q2", "the predicates, and the discount", "predicates_and_discount"),
        ("Q3", "the predicates, and both", "predicates_and_both"),
        ("Q4", "the whole query", "q6_revenue"),
    ]
    for key, label, measurement in rungs:
        row = f"{key:<4}{label:<34}"
        for rung_ladder in all_ladders:
            values = rung_ladder.get(measurement, [])
            text = f"{statistics.median(values):.2f} ({min(values):.2f}-{max(values):.2f})" if values else "-"
            row += f"{text:>24}"
        print(row)

    print()
    for (name, _), rung_ladder in zip(files, all_ladders):
        print(f"{'the scan stage, ' + name:<40}{statistics.median(rung_ladder.get('q6_revenue_scan', [0.0])):.2f} ms")
    for name, runs in files:
        print(
            f"{'the host load average, ' + name:<40}"
            f"{statistics.median([run.get('host_load_average', 0.0) for run in runs]):.1f}"
        )

    # The materialised column's share is derived from the **campaigns**, not
    # from the medians of the medians: each campaign's share is measured, and
    # the median of those is the figure.
    print()
    for name, runs in files:
        shares = [
            run["query"]["measurements_ms"]["predicates_and_price"]
            - run["query"]["measurements_ms"]["predicates_only"]
            for run in runs
            if "query" in run
        ]
        if shares:
            print(f"{'the materialised column share, ' + name:<40}{statistics.median(shares):.2f} ms")


def print_curve(runs: list[dict]) -> None:
    """Print the size curve's medians.

    This is spec 014's third axis: the query and the thread count are held
    fixed and the **size of the data** moves. The figure it exists for is the
    cost per million rows, because that is what a reader asking about the
    size axis wants, and a table of whole-file times makes them divide it
    out by hand.

    Every system is measured at every size, in the same harness, on the same
    file, with the thread count pinned -- a curve with only this engine's
    line on it is a curve with nothing to be compared against.
    """
    if not runs:
        return

    def points(key: str) -> dict[int, list[float]]:
        out: dict[int, list[float]] = {}
        for run in runs:
            for point in run.get("points", []):
                value = point.get(key)
                if value is not None:
                    out.setdefault(point["rows"], []).append(value)
        return out

    engine = points("engine_ms")
    duckdb = points("duckdb_ms")
    polars = points("polars_ms")
    datafusion = points("datafusion_ms")
    scan = points("engine_scan_ms")
    operators = points("engine_operators_ms")
    estimate = points("engine_estimate_bytes")
    rows_of: dict[int, int] = {}
    for run in runs:
        for point in run.get("points", []):
            rows_of[point["rows"]] = point.get("scale_factor") or 0

    budget = runs[0].get("memory_budget_bytes")
    print(f"\n=== THE SIZE CURVE, {len(runs)} campaigns ===")
    if budget:
        print(f"the engine's memory budget: {budget // (1024 * 1024)} MiB")
    header = f"{'rows':>10}{'sf':>4}{'est MiB':>10}{'engine':>12}{'scan':>10}{'ops':>8}"
    for name in ("duckdb", "polars", "datafusion"):
        header += f"{name:>12}"
    header += f"{'ms/Mrow':>10}"
    print(header)
    print("-" * len(header))
    for rows in sorted(engine):
        line = f"{rows:>10}{rows_of.get(rows, 0):>4}"
        estimates = estimate.get(rows)
        line += f"{statistics.median(estimates) / (1024 * 1024):>10.1f}" if estimates else f"{'-':>10}"
        line += f"{statistics.median(engine[rows]):>12.1f}"
        line += f"{statistics.median(scan.get(rows, [0.0])):>10.1f}"
        line += f"{statistics.median(operators.get(rows, [0.0])):>8.1f}"
        for figures in (duckdb, polars, datafusion):
            values = figures.get(rows)
            line += f"{statistics.median(values):>12.1f}" if values else f"{'-':>12}"
        line += f"{statistics.median(engine[rows]) / (rows / 1e6):>10.1f}"
        print(line)


def print_batch_size(runs: list[dict]) -> None:
    """Print the reader batch limit's medians.

    This is spec 016: the limit on the reader's batch size, swept over the
    two shapes the lazy path serves, and the A/B of the fixed batch size the
    tree used before against the batch it derives now.

    Every figure is the median over the campaigns, with the spread.
    """
    if not runs:
        return

    shapes: dict[str, dict[str, dict[int, list[float]]]] = {}
    candidates: dict[str, dict[int, list[float]]] = {}
    order: list[str] = []
    for run in runs:
        for shape in run.get("shapes", []):
            name = shape["shape"]
            if name not in order:
                order.append(name)
            limits = shapes.setdefault(name, {})
            for entry in shape.get("limits", []):
                bucket = limits.setdefault(str(entry["limit"]), {})
                bucket.setdefault("scan_ms", []).append(entry["scan_ms"])
                bucket.setdefault("batch", []).append(entry["batch"])
            pair = candidates.setdefault(name, {})
            for entry in shape.get("candidates", []):
                pair.setdefault(str(entry["batch"]), []).append(entry["scan_ms"])

    print(f"\n=== THE READER'S BATCH LIMIT, {len(runs)} campaigns ===")
    for name in order:
        limits = shapes.get(name, {})
        print(f"\n-- {name} --")
        print(f"{'the limit':>12}{'the batch':>12}{'the scan':>22}")
        for key in sorted(limits, key=lambda value: int(value)):
            bucket = limits.get(key, {})
            scans = bucket.get("scan_ms", [])
            batches = bucket.get("batch", [])
            if not scans:
                continue
            print(
                f"{int(key):>12}{int(statistics.median(batches)):>12}"
                + cell((statistics.median(scans), min(scans), max(scans)))
            )
        pair = candidates.get(name, {})
        print("  the two candidates, interleaved:")
        for key in sorted(pair, key=lambda value: int(value)):
            scans = pair.get(key, [])
            if scans:
                print(f"{int(key):>12}{'':>12}" + cell(
                    (statistics.median(scans), min(scans), max(scans))))
        if len(pair) == 2:
            keys = sorted(pair, key=lambda value: int(value))
            old = statistics.median(pair.get(keys[0], [0.0]))
            new = statistics.median(pair.get(keys[1], [0.0]))
            print(f"  the difference: {new - old:8.2f} ms  ({(new - old) / old * 100.0:+.1f}%)")

    print(f"\n{'the host load average':<44}"
          f"{statistics.median([run.get('host_load_average', 0.0) for run in runs]):.1f}")


def print_row_group_cost(files: list[tuple[str, list[dict]]]) -> None:
    """Print the per-row-group cost's medians.

    This is spec 016: the same probe on the six-row-group file, the
    twelve-row-group one and the twenty-four-row-group one, measured in one
    session. The probe reports the crate's read at the **fixed** batch size the
    tree used, the engine's own scan at the **derived** batch size, and a
    ladder of reader batch sizes.

    The slope is the cost of one more row group: the 24-group figure minus
    the 6-group figure, over the eighteen row groups between them.
    """
    if not files:
        return

    measurements: dict[str, dict[str, dict[str, list[float]]]] = {}
    ladders: dict[str, dict[int, dict[str, list[float]]]] = {}
    groups: dict[str, int] = {}
    order: list[str] = []
    for name, runs in files:
        order.append(name)
        for run in runs:
            groups[name] = int(run.get("row_groups", 0))
            for key, value in run.get("measurements_ms", {}).items():
                measurements.setdefault(name, {}).setdefault(key, {}).setdefault(
                    "value", []).append(value)
            for entry in run.get("ladder", []):
                ladders.setdefault(name, {}).setdefault(int(entry["batch"]), {}).setdefault(
                    "scan_ms", []).append(entry["scan_ms"])

    def slope(name: str, key: str) -> float:
        """The cost of one more row group, in milliseconds."""
        if len(files) < 2:
            return float("nan")
        first = statistics.median(
            measurements.get(files[0][0], {}).get(key, {}).get("value", [float("nan")]))
        last = statistics.median(
            measurements.get(files[-1][0], {}).get(key, {}).get("value", [float("nan")]))
        span = groups[files[-1][0]] - groups[files[0][0]]
        if not span:
            return float("nan")
        return (last - first) / span

    campaigns = min(len(runs) for _, runs in files)
    print(f"\n=== THE PER-ROW-GROUP COST, {len(files)} files, {campaigns} campaigns each ===")
    print(f"{'the figure':<52}" + "".join(f"{name + ', ' + str(groups[name]) + ' groups':>20}"
                                          for name, _ in files)
          + f"{'ms/row group':>16}")

    rows = [
        ("crate_shipdate", "the crate's read of l_shipdate, fixed batch"),
        ("crate_discount", "the crate's read of l_discount, fixed batch"),
        ("crate_price", "the crate's read of l_extendedprice, fixed batch"),
        ("crate_both", "the crate's read of both predicate columns"),
        ("crate_with_pushdown", "the crate's read, with the pushdown on top"),
        ("engine_predicates", "the engine's own scan, the derived batch"),
        ("engine_full", "the engine's own scan, and the materialised column"),
    ]
    for key, label in rows:
        line = f"{label:<52}"
        for name, _ in files:
            values = measurements.get(name, {}).get(key, {}).get("value", [])
            line += f"{statistics.median(values):>20.2f}" if values else f"{'-':>20}"
        line += f"{slope(name, key):>16.3f}"
        print(line)

    print("\n-- the ladder: the engine's own scan, by reader batch size --")
    header = f"{'the batch':>12}"
    for name, _ in files:
        header += f"{name + ', ' + str(groups[name]) + ' groups':>26}"
    print(header)
    batches = sorted({batch for buckets in ladders.values() for batch in buckets})
    for batch in batches:
        line = f"{batch:>12}"
        for name, _ in files:
            values = ladders.get(name, {}).get(batch, {}).get("scan_ms", [])
            line += f"{statistics.median(values):>26.2f}" if values else f"{'-':>26}"
        print(line)

    print("\n-- the same ladder, as the cost of one more row group --")
    print(f"{'the batch':>12}{'ms/row group':>16}"
          f"   (the 24-group figure less the 6-group one, over 18 row groups)")
    for batch in batches:
        first = statistics.median(
            ladders.get(files[0][0], {}).get(batch, {}).get("scan_ms", [float("nan")]))
        last = statistics.median(
            ladders.get(files[-1][0], {}).get(batch, {}).get("scan_ms", [float("nan")]))
        span = groups[files[-1][0]] - groups[files[0][0]]
        line = f"{batch:>12}"
        line += f"{(last - first) / span:>16.3f}" if span else f"{'-':>16}"
        print(line)


def print_kernel(runs: list[dict]) -> None:
    """Print the ARM64 kernel A/B: the medians, and the spread across campaigns.

    This is §1's table. Each campaign is the median of eight iterations of
    the four flavors, and the published figure is the median of the
    campaigns -- the same rule the rest of the file follows.
    """
    if not runs:
        return

    flavours: dict[str, dict[str, list[float]]] = {}
    matches: set[int] = set()
    for run in runs:
        for flavour in run.get("kernel_ab", {}).get("flavors", []):
            name = flavour["flavor"]
            bucket = flavours.setdefault(
                name,
                {"rows_per_sec": [], "ns_per_row": [],
                 "ratio_vs_scalar": [], "ratio_vs_auto": []},
            )
            bucket["rows_per_sec"].append(float(flavour["rows_per_sec"]))
            bucket["ns_per_row"].append(float(flavour["ns_per_row"]))
            bucket["ratio_vs_scalar"].append(float(flavour["ratio_vs_scalar"]))
            bucket["ratio_vs_auto"].append(float(flavour["ratio_vs_auto"]))
            matches.add(int(flavour["matches"]))

    machine = runs[0].get("machine", {})
    rows = runs[0].get("kernel_ab", {})
    print(f"\n=== THE KERNEL A/B, {len(runs)} campaigns ===")
    print(f"machine: {machine.get('cpu', '?')}, {machine.get('arch', '?')}, "
          f"{machine.get('cores', '?')} core(s), {machine.get('os', '?')}, "
          f"{machine.get('rustc', '?')}, simd {machine.get('simd', '?')}")
    print(f"rows: {rows.get('rows', '?')}, {rows.get('iterations', '?')} iterations "
          f"a campaign, column {rows.get('column', '?')} {rows.get('operator', '?')} "
          f"{rows.get('threshold', '?')}")
    print(f"{'flavor':<12}{'rows/sec':>26}{'ns/row':>26}"
          f"{'vs scalar':>26}{'vs cpp-auto':>26}")
    for name in ("scalar", "cpp-auto", "cpp-simd", "rust-simd"):
        bucket = flavours.get(name)
        if not bucket:
            continue
        speed = bucket["rows_per_sec"]
        ns = bucket["ns_per_row"]
        vs = bucket["ratio_vs_scalar"]
        va = bucket["ratio_vs_auto"]
        print(f"{name:<12}"
              f"{statistics.median(speed) / 1e9:>16.3f}G"
              f" ({min(speed) / 1e9:.2f}-{max(speed) / 1e9:.2f})"
              f"{statistics.median(ns):>16.3f}"
              f" ({min(ns):.3f}-{max(ns):.3f})"
              f"{statistics.median(vs):>16.2f}x"
              f" ({min(vs):.2f}-{max(vs):.2f})"
              f"{statistics.median(va):>16.2f}x"
              f" ({min(va):.2f}-{max(va):.2f})")
    if len(matches) == 1:
        print(f"every flavour agrees on the match count: {next(iter(matches)):,}")
    else:
        print(f"the flavours DISAGREE on the match count: {sorted(matches)}")


def avx2_machine(run: dict) -> str:
    """The machine a report was measured on, as the runner named it.

    GitHub's hosted runners are a **fleet**: two runs of the same workflow can
    land on two different CPU generations, and the kernel's ratio is a
    property of the CPU as much as of the kernel. So the reports are grouped
    by machine and the median is taken *within* a machine, never across one.
    """
    machine = run.get("machine", {})
    return f"{machine.get('cpu', '?')} · {machine.get('cores', '?')} cores"


def avx2_groups(runs: list[dict]) -> dict[str, list[dict]]:
    """Group the x86-64 reports by the machine that produced them."""
    groups: dict[str, list[dict]] = {}
    for run in runs:
        groups.setdefault(avx2_machine(run), []).append(run)
    return groups


def print_avx2(runs: list[dict]) -> None:
    """Print the x86-64 kernel A/B: the ratios, and nothing else.

    These figures come from a GitHub-hosted x86-64 runner. The **absolute**
    rows/sec and ns/row describe a shared machine, so they are not published;
    the ratios are taken from the same process, on the same machine, in the
    same run, so they are.

    The ratios are published **per machine**, and only for a machine that ran
    the campaign at least `MIN_CAMPAIGNS` times. A pooled median across
    different CPUs would be a number about no machine at all.
    """
    if not runs:
        return

    groups = avx2_groups(runs)
    print(f"\n=== THE KERNEL A/B ON x86-64, {len(runs)} campaigns "
          f"on {len(groups)} machine(s) ===")

    for machine, group in sorted(groups.items(), key=lambda kv: (-len(kv[1]), kv[0])):
        spec = group[0].get("machine", {})
        campaigns = len(group)
        flavours: dict[str, dict[str, list[float]]] = {}
        matches: set[int] = set()
        for run in group:
            for flavour in run.get("kernel_ab", {}).get("flavors", []):
                name = flavour["flavor"]
                bucket = flavours.setdefault(
                    name, {"ratio_vs_scalar": [], "ratio_vs_auto": [], "matches": []})
                bucket["ratio_vs_scalar"].append(float(flavour["ratio_vs_scalar"]))
                bucket["ratio_vs_auto"].append(float(flavour["ratio_vs_auto"]))
                bucket["matches"].append(float(flavour["matches"]))
                matches.add(int(flavour["matches"]))

        print(f"\n-- {machine} -- {campaigns} campaign(s)")
        print(f"   {spec.get('arch', '?')}, {spec.get('os', '?')}, "
              f"{spec.get('rustc', '?')}, simd {spec.get('simd', '?')}, "
              f"{spec.get('threads', '?')} thread(s)")
        rows = group[0].get("kernel_ab", {})
        print(f"   rows: {rows.get('rows', '?')}, {rows.get('iterations', '?')} "
              f"iterations, operator {rows.get('operator', '?')}")

        if campaigns < MIN_CAMPAIGNS:
            print(f"   NOT PUBLISHED: {campaigns} campaign(s), fewer than the "
                  f"{MIN_CAMPAIGNS} this repository requires for a median.")
            continue

        print(f"   {'flavor':<12}{'vs scalar':>22}{'vs cpp-auto':>22}{'matches':>14}")
        for name in ("scalar", "cpp-auto", "cpp-simd", "rust-simd"):
            bucket = flavours.get(name)
            if not bucket:
                continue
            vs = bucket["ratio_vs_scalar"]
            va = bucket["ratio_vs_auto"]
            print(f"   {name:<12}"
                  f"{statistics.median(vs):>7.2f}x ({min(vs):.2f}-{max(vs):.2f})".rjust(22)
                  + f"{statistics.median(va):>7.2f}x ({min(va):.2f}-{max(va):.2f})".rjust(22)
                  + f"{int(statistics.median(bucket['matches'])):>14,}")
        if len(matches) == 1:
            print(f"   every flavour agrees on the match count: {next(iter(matches)):,}")
        else:
            print(f"   the flavours DISAGREE on the match count: {sorted(matches)}")


def _probe_medians(files: list[tuple[str, list[dict]]], section: str) -> tuple[
    dict[str, dict[str, list[float]]], dict[str, int]
]:
    """The medians of a probe's measurements, keyed by file and by shape."""
    measurements: dict[str, dict[str, list[float]]] = {}
    groups: dict[str, int] = {}
    for name, runs in files:
        for run in runs:
            groups[name] = int(run.get("row_groups", 0))
            for key, value in run.get(section, {}).items():
                measurements.setdefault(name, {}).setdefault(key, []).append(value)
    return measurements, groups


def print_dictionary_cost(
    after_files: list[tuple[str, list[dict]]],
    before_files: list[tuple[str, list[dict]]],
) -> None:
    """Print the dictionary probe's medians.

    This is spec 022: the same read in two shapes -- the reader expanding the
    dictionary-encoded columns, and the reader keeping them -- with the
    engine's own comparison measured on top of each. The "before" set is the
    same probe against the code the increment replaced, measured back to back
    on the same host.
    """
    for label, files in (("the new code", after_files), ("the old code", before_files)):
        if not files:
            continue
        measurements, groups = _probe_medians(files, "measurements_ms")
        columns, _ = _probe_medians(files, "columns_ms")
        dictionary, _ = _probe_medians(files, "dictionary_ms")
        campaigns = min(len(runs) for _, runs in files)
        names = [name for name, _ in files]
        print(f"\n=== THE DICTIONARY, COUNTED -- {label} ({campaigns} campaigns each) ===")
        header = f"{'the shape':<52}" + "".join(
            f"{names[index] + ', ' + str(groups[names[index]]) + ' groups':>20}"
            for index in range(len(names))
        )
        print(header)
        for key, shape in (
            ("crate_expanded", "the crate's read, the dictionary expanded"),
            ("crate_kept", "the crate's read, the dictionary kept"),
            ("engine_expanded", "the engine's read, over the values"),
            ("engine_kept", "the engine's read, over the distinct"),
            ("engine_read", "the engine's own read"),
        ):
            line = f"{shape:<52}"
            for name in names:
                values = measurements.get(name, {}).get(key, [])
                line += f"{statistics.median(values):>20.2f}" if values else f"{'-':>20}"
            print(line)
        line = f"{'the engine\'s comparison, over the values':<52}"
        for name in names:
            expanded = measurements.get(name, {}).get("engine_expanded", [])
            crate = measurements.get(name, {}).get("crate_expanded", [])
            if expanded and crate:
                line += f"{statistics.median(expanded) - statistics.median(crate):>20.2f}"
            else:
                line += f"{'-':>20}"
        print(line)

        print("\n-- one column at a time --")
        for key, column in (
            ("quantity", "l_quantity"),
            ("discount", "l_discount"),
            ("shipdate", "l_shipdate"),
        ):
            line = f"{column:<52}"
            for name in names:
                expanded = columns.get(name, {}).get(f"crate_expanded_{key}", [])
                kept = columns.get(name, {}).get(f"crate_kept_{key}", [])
                if expanded and kept:
                    line += (
                        f"{statistics.median(expanded):>9.2f} -> {statistics.median(kept):>9.2f}"
                    )
                else:
                    line += f"{'-':>20}"
            print(line)

        print("\n-- the alternative's own shape --")
        for key, shape in (
            ("distinct", "the distinct values, compared per batch"),
            ("map", "the map, from the answer to the rows"),
        ):
            line = f"{shape:<52}"
            for name in names:
                values = dictionary.get(name, {}).get(key, [])
                line += f"{statistics.median(values):>20.2f}" if values else f"{'-':>20}"
            print(line)

        if len(names) >= 2:
            first, last = names[0], names[-1]
            span = groups[last] - groups[first]

            def comparison(name: str) -> float:
                """The engine's own cost over the crate's read."""
                expanded = measurements.get(name, {}).get("engine_expanded", [])
                crate = measurements.get(name, {}).get("crate_expanded", [])
                if not expanded or not crate:
                    return float("nan")
                return statistics.median(expanded) - statistics.median(crate)

            print(f"\n-- the rate, over {span} row groups --")
            for shape, rate in (
                ("the crate's read", (
                    statistics.median(measurements[last]["crate_expanded"])
                    - statistics.median(measurements[first]["crate_expanded"])
                ) / span),
                ("the engine's comparison, over the values",
                 (comparison(last) - comparison(first)) / span),
            ):
                print(f"{shape:<52}{rate:>20.3f}")


def print_scan_cost(files: list[tuple[str, list[dict]]]) -> None:
    """Print the scan probe's medians.

    This is spec 023: the join's scan, taken apart -- the crate's read of the
    two columns the join reads, the engine's own scan of the same two, and
    the difference between them, which is the engine's copy.
    """
    if not files:
        return
    measurements, groups = _probe_medians(files, "measurements_ms")
    campaigns = min(len(runs) for _, runs in files)
    names = [name for name, _ in files]
    print(f"\n=== THE SCAN, TAKEN APART ({campaigns} campaigns each) ===")
    print(f"{'the shape':<52}" + "".join(
        f"{names[index] + ', ' + str(groups[names[index]]) + ' groups':>20}"
        for index in range(len(names))
    ))
    for key, shape in (
        ("crate_orderkey", "the crate's read of l_orderkey"),
        ("crate_price", "the crate's read of l_extendedprice"),
        ("crate_both", "the crate's read of both"),
        ("engine_orderkey", "the engine's own scan of l_orderkey"),
        ("engine_price", "the engine's own scan of l_extendedprice"),
        ("engine_both", "the engine's own scan of both"),
        ("crate_orders_both", "the crate's read of the build side"),
    ):
        line = f"{shape:<52}"
        for name in names:
            values = measurements.get(name, {}).get(key, [])
            line += f"{statistics.median(values):>20.2f}" if values else f"{'-':>20}"
        print(line)
    line = f"{'the engine\'s copy':<52}"
    for name in names:
        both = measurements.get(name, {}).get("engine_both", [])
        crate = measurements.get(name, {}).get("crate_both", [])
        if both and crate:
            line += f"{statistics.median(both) - statistics.median(crate):>20.2f}"
        else:
            line += f"{'-':>20}"
    print(line)


def print_own_decode(files: list[tuple[str, list[dict]]]) -> None:
    """Print the reader that owns the decode.

    This is spec 052: ADR-0018's shape, measured again on the code that is
    here now, and the shape it did not measure -- **the engine's own reader**,
    with the page walk, the dictionary and the indices, and no expansion at
    all.
    """
    if not files:
        return
    campaigns = min(len(runs) for _, runs in files)
    print(f"\n=== THE READER THAT OWNS THE DECODE ({campaigns} campaigns each) ===")
    for name, runs in files:
        batches = [entry["batch_rows"] for entry in runs[0]["the_measurements"]]
        print(f"\n-- {name}: the shapes at each batch size --")
        print(f"{'the batch':>10} {'A the crate':>16} {'B Arrow':>16} {'C the old shape':>18} {'D the engine':>18}")
        for index, batch in enumerate(batches):
            def median(key: str) -> float:
                return statistics.median(
                    [run["the_measurements"][index][key] for run in runs]
                )

            print(
                f"{batch:>10} {median('a'):>16.3f} {median('b'):>16.3f} "
                f"{median('c'):>18.3f} {median('d'):>18.3f}"
            )

        def reader_median(key: str) -> float:
            return statistics.median(
                [run["the_engine_s_own_reader"][key] for run in runs]
            )

        engine = reader_median("the_engine_s_own_default")
        whole = reader_median("e")
        whole_expanded = reader_median("e2")
        by_group = reader_median("e3")
        by_group_expanded = reader_median("e4")
        print(f"\n-- {name}: the reader's own shapes --")
        print(f"  {'the engine\'s own scan':<44}{engine:>10.3f} ms")
        print(f"  {'the reader, by row group':<44}{by_group:>10.3f} ms   {by_group - engine:+.3f} ms")
        print(f"  {'the same, with the expansion':<44}{by_group_expanded:>10.3f} ms   {by_group_expanded - by_group:+.3f} ms")
        print(f"  {'the reader, the whole file\'s':<44}{whole:>10.3f} ms   {whole - by_group:+.3f} ms")
        print(f"  {'the same, with the expansion':<44}{whole_expanded:>10.3f} ms   {whole_expanded - whole:+.3f} ms")


def _query_cost_figures(
    files: list[tuple[str, list[dict]]],
) -> tuple[dict[str, dict[str, list[float]]], dict[str, dict[str, list[list[float]]]]]:
    """The query probe's figures.

    The probe's report has two parts: the **shapes** -- the dataset's "every
    row" selection, measured once, because they are a property of the dataset
    and not of a query -- and the **queries**, per query and per path (the
    resident one and the cold one).

    This collects them across the campaigns, so a median can be taken over the
    campaigns rather than over the iterations inside one of them.
    """
    shapes: dict[str, list[float]] = {}
    shape_spreads: dict[str, list[list[float]]] = {}
    queries: dict[str, dict[str, dict[str, list[float]]]] = {}
    for _, runs in files:
        for run in runs:
            for key, value in run.get("shapes_ms", {}).items():
                shapes.setdefault(key, []).append(value)
            for key, value in run.get("spreads_ms", {}).items():
                shape_spreads.setdefault(key, []).append(value)
            for path, entries in run.get("queries", {}).items():
                for query, values in entries.items():
                    for key, value in values.items():
                        if key.endswith("_spread_ms"):
                            continue
                        queries.setdefault(path, {}).setdefault(query, {}).setdefault(key, []).append(value)
    return {"shapes": shapes, "queries": queries}, {"shapes": shape_spreads}


# The probe's shapes, and what they are called in the table.
QUERY_COST_SHAPES = (
    ("selection_rows", "the selection expanded into row numbers"),
    ("selection_walk", "the same rows, read as they are needed"),
    ("bitmap_rows", "the same expansion, with the selection a bitmap"),
    ("bitmap_walk", "the same walk, with the selection a bitmap"),
    ("plan_resolve", "the plan's resolution"),
    ("memory_estimate", "the memory estimate"),
    ("build_open", "the join's build side opened"),
)

# The queries, in the order the probe measures them.
QUERY_COST_QUERIES = (
    "scan",
    "filter-numeric",
    "filter-string",
    "filter-range",
    "q6",
    "q6-revenue",
    "join-orders",
    "sort-topk",
)


def print_query_cost(
    after_files: list[tuple[str, list[dict]]],
    before_files: list[tuple[str, list[dict]]],
) -> None:
    """Print the query probe's medians.

    This is spec 024: the whole query, the plan output's own stage table,
    and the time between them -- the part of the query the plan output does
    not name. The "before" set is the same probe against the code the
    increment replaced, measured back to back on the same host.
    """
    for label, files in (("the new code", after_files), ("the old code", before_files)):
        if not files:
            continue
        figures, spreads = _query_cost_figures(files)
        campaigns = min(len(runs) for _, runs in files)
        print(f"\n=== THE QUERY, AND THE PART OF IT THE PLAN OUTPUT DOES NOT NAME -- {label} "
              f"({campaigns} campaigns each) ===")

        print("\n-- the shapes, once: the dataset's \"every row\" selection --")
        print(f"{'the shape':<52}{'median':>12}{'the spread':>20}")
        for key, shape in QUERY_COST_SHAPES:
            values = figures["shapes"].get(key, [])
            if not values:
                continue
            line = f"{shape:<52}{statistics.median(values):>12.3f}"
            bounds = spreads["shapes"].get(key, [])
            if bounds:
                low = statistics.median([bound[0] for bound in bounds])
                high = statistics.median([bound[1] for bound in bounds])
                line += f"{f'{low:.2f}-{high:.2f}':>20}"
            else:
                line += f"{'-':>20}"
            print(line)

        for path in ("resident", "cold"):
            entries = figures["queries"].get(path, {})
            if not entries:
                continue
            print(f"\n-- {path} --")
            print(f"{'the query':<20}{'the query':>12}{'the stages':>12}{'unnamed':>12}")
            for query in QUERY_COST_QUERIES:
                values = entries.get(query, {})
                if not values:
                    continue
                line = f"{query:<20}"
                for key in ("query_ms", "stages_ms", "unnamed_ms"):
                    figures_for_key = values.get(key, [])
                    if figures_for_key:
                        line += f"{statistics.median(figures_for_key):>12.3f}"
                    else:
                        line += f"{'-':>12}"
                print(line)


# The probe's parts, and what they are called in the table.
#
# The names are the probe's own and the labels are the ones the published
# table uses, so the derivation and the document cannot drift apart over a
# name.
SORT_COST_PARTS = (
    ("stage_cold", "the stage, cold"),
    ("stage_resident", "the stage, resident"),
    ("operator_cold", "the operator, cold"),
    ("operator_warm", "the operator, warm"),
    ("read_shape", "the read, the engine's shape"),
    ("read_plain", "the read, plain"),
    ("read_small", "the read, the small control"),
    ("floor_block", "the comparison, in blocks"),
    ("candidate_block", "the candidate, in blocks"),
    ("rows_range", "the rows, walked"),
    ("key_column", "the key column's construction"),
    ("fused_decode", "the decode, on its own"),
    ("fused_topk", "the fused top-k"),
)

# The fallbacks: the two paths the published query does **not** take. They are
# named so that a reader can see what the bounded sort over a numeric column
# is worth against the sort that has to sort every row, and against the sort
# that has to read a key as an enum.
SORT_COST_FALLBACKS = (
    ("operator_all", "the sort with no limit"),
    ("operator_text", "the sort over strings"),
)


def _sort_cost_figures(
    files: list[tuple[str, list[dict]]],
) -> tuple[dict[str, list[float]], dict[str, list[list[float]]], dict[str, list[float]]]:
    """The sort probe's figures, across the campaigns.

    The report has three parts: the **parts** -- the stage, the operator, the
    reads and the candidates, each one the median of its own iterations -- the
    **spread** beside every one of them, and the **fallbacks**, the two paths
    the published query does not take.

    This collects them across the campaigns, so the median is taken over the
    campaigns rather than over the iterations inside one of them.
    """
    parts: dict[str, list[float]] = {}
    spreads: dict[str, list[list[float]]] = {}
    fallbacks: dict[str, list[float]] = {}
    for _, runs in files:
        for run in runs:
            for key, value in run.get("parts_ms", {}).items():
                parts.setdefault(key, []).append(value)
            for key, value in run.get("spreads_ms", {}).items():
                spreads.setdefault(key, []).append(value)
            for key, value in run.get("fallbacks_ms", {}).items():
                fallbacks.setdefault(key, []).append(value)
    return parts, spreads, fallbacks


def print_sort_cost(
    after_files: list[tuple[str, list[dict]]],
    before_files: list[tuple[str, list[dict]]],
) -> None:
    """Print the sort probe's medians.

    This is spec 025: the sort's stage, taken apart -- the engine's own stage
    on both paths, the sort's own call over the same rows and the same key
    column, the reads that are the floor, and the candidate that is the
    cross-check. The "before" set is the same probe against the code the
    increment replaced, measured back to back on the same host.
    """
    for label, files in (("the new code", after_files), ("the old code", before_files)):
        if not files:
            continue
        parts, spreads, fallbacks = _sort_cost_figures(files)
        campaigns = min(len(runs) for _, runs in files)
        print(f"\n=== THE SORT, TAKEN APART -- {label} ({campaigns} campaigns each) ===")

        print("\n-- the parts --")
        print(f"{'the part':<52}{'median':>12}{'the spread':>20}")
        for key, part in SORT_COST_PARTS:
            values = parts.get(key, [])
            if not values:
                continue
            line = f"{part:<52}{statistics.median(values):>12.2f}"
            bounds = spreads.get(key, [])
            if bounds:
                low = statistics.median([bound[0] for bound in bounds])
                high = statistics.median([bound[1] for bound in bounds])
                line += f"{f'{low:.2f}-{high:.2f}':>20}"
            else:
                line += f"{'-':>20}"
            print(line)

        # The fallbacks are measured fewer times and the report carries no
        # spread for them, so the table is the median alone.
        print("\n-- the fallbacks: the paths the published query does not take --")
        print(f"{'the fallback':<52}{'median':>12}")
        for key, fallback in SORT_COST_FALLBACKS:
            values = fallbacks.get(key, [])
            if not values:
                continue
            print(f"{fallback:<52}{statistics.median(values):>12.2f}")


# The names are the probe's own and the labels are the ones the published
# table uses, so the derivation and the document cannot drift apart over a
# name.
JOIN_COST_PARTS = (
    ("stage_cold", "the stage, cold"),
    ("stage_resident", "the stage, resident"),
    ("build_open", "the build side's open"),
    ("build_read", "the build side's read"),
    ("build_keys", "the build side's keys"),
    ("build_whole", "the build side, whole"),
    ("table_own", "the table, on its own"),
    ("probe_column", "the probe, over the column"),
    ("probe_batches", "the probe, in the batches"),
    ("floor_probe", "the floor the probe cannot go under"),
    ("candidate_probe", "the candidate, written out"),
    ("candidate_with_map", "the candidate with the overflow consult"),
    ("cold_candidate", "the cold candidate"),
    ("candidate_chain", "the candidate, the chain"),
    ("candidate_width", "the candidate, the width"),
    ("candidate_state", "the candidate, the state through `self`"),
    ("candidate_map_option", "the candidate, the map's option"),
    ("engine_lookup_plain", "the engine's lookup, over a plain slice"),
    ("read_plain", "the read, plain"),
    ("read_small", "the read, the small control"),
)

# The differences: the figures the instrument computes from its own parts, so
# that a difference is derived rather than typed. The table is the difference
# between the whole and the read and the keys; the output rows are the
# difference between the candidate and the floor; the probe's remainder is the
# difference between the engine's probe and the candidate's; the two paths and
# the two probes are the differences between the two stages and between the two
# probes.
JOIN_COST_DERIVED = (
    ("build_table", "the table, as the difference"),
    ("output_rows", "the output rows"),
    ("the_probe_s_remainder", "the probe's remainder"),
    ("the_map_s_share", "the map's share"),
    ("the_cold_remainder", "the cold remainder"),
    ("the_two_paths", "the two paths"),
    ("the_two_probes", "the two probes"),
    ("the_chain_s_cost", "the chain's cost"),
    ("the_width_s_cost", "the width's cost"),
    ("the_state_s_cost", "the state's cost"),
    ("the_map_option_s_cost", "the map's option's cost"),
    ("the_loop_s_cost", "the engine's loop's cost"),
    ("the_key_source_s_cost", "the key source's cost"),
    ("the_ladder_s_sum", "the ladder's sum"),
    ("the_unexplained", "what is left unexplained"),
    ("the_residual_cold", "the stage's residual, cold"),
    ("the_residual_resident", "the stage's residual, resident"),
    ("the_context_cold", "the context, cold"),
    ("the_context_resident", "the context, resident"),
)


# The floor: the machine's own measure of a memory-bound loop. Its drift is the
# yardstick's floor.
JOIN_COST_FLOOR = "floor_probe"

# The file-state floor: the file's own read, in the iteration. It is the build
# side's read through the engine's own reader, with the engine's own
# predicates, and it is measured in the iteration -- so the file's state and
# the machine's own movement are in the same iteration as the parts.
JOIN_COST_FILE_FLOOR = "build_read"

# The ladder's leaves, per path: the figure, the label the section publishes it
# under, the decimal places, the word it carries, and **the sides the leaf
# stands on** -- the measurements whose drifts the yardstick takes.
#
# The parts are the **leaves**: the build side's gap is the build side's own --
# both sides read the build side's file warm, so the file's state does not enter
# it -- and the probe's gap is its file's state, its row-number offset and what
# is left of it.
JOIN_COST_LADDER = {
    "cold": (
        (
            "the_build_gap_cold",
            "the build side's gap, cold",
            3,
            "the build side's",
            ("engine_build_cold", "build_whole"),
        ),
        (
            "the_open_gap_cold",
            "the open's gap, cold",
            3,
            "the clock's",
            ("engine_open_cold", "build_open"),
        ),
        (
            "the_probe_row_offset",
            "the probe's row-number offset, cold",
            3,
            "the clock's",
            ("probe_batches", "probe_batches_no_offset"),
        ),
        (
            "the_probe_residual_cold",
            "the probe's own remainder, cold",
            3,
            "the probe's",
            ("engine_probe_cold", "probe_batches_no_offset"),
        ),
        (
            "the_host_remainder_cold",
            "the part in no gap, cold",
            3,
            "the host's",
            ("stage_cold",),
        ),
    ),
    "resident": (
        (
            "the_build_gap_resident",
            "the build side's gap, resident",
            3,
            "the build side's",
            ("engine_build_resident", "build_whole"),
        ),
        (
            "the_open_gap_resident",
            "the open's gap, resident",
            3,
            "the clock's",
            ("engine_open_resident", "build_open"),
        ),
        (
            "the_probe_file_state",
            "the probe's file state, resident",
            3,
            "the memory's",
            ("probe_held", "probe_column"),
        ),
        (
            "the_probe_residual_resident",
            "the probe's own remainder, resident",
            3,
            "the probe's",
            ("engine_probe_resident", "probe_held"),
        ),
        (
            "the_host_remainder_resident",
            "the part in no gap, resident",
            3,
            "the host's",
            ("stage_resident",),
        ),
    ),
}

# The ladder's own figures: the containers, the checks, and the controls. The
# controls are reported beside every set and are never summed.
JOIN_COST_LADDER_FIGURES = (
    ("the_probe_gap_cold", "the probe's gap, cold", 3),
    ("the_gaps_sum_cold", "the three gaps, cold", 3),
    ("the_reconciliation_cold", "the reconciliation, cold", 3),
    ("the_reduction_s_noise_cold", "the reduction's own noise, cold", 3),
    ("the_probe_gap_resident", "the probe's gap, resident", 3),
    ("the_gaps_sum_resident", "the three gaps, resident", 3),
    ("the_reconciliation_resident", "the reconciliation, resident", 3),
    ("the_reduction_s_noise_resident", "the reduction's own noise, resident", 3),
    ("the_build_file_state", "the build side's file state", 3),
    ("the_added_clock_cost", "the added clock's cost", 6),
    ("the_order_s_cost_cold", "the order's own cost, cold", 3),
    ("the_order_s_cost_resident", "the order's own cost, resident", 3),
    ("the_separation_cold", "the separation, cold", 3),
    ("the_separation_resident", "the separation, resident", 3),
    ("the_added_work_cost", "the interleaving's added work", 6),
)


def join_cost_drift(parts: dict[str, list[float]]) -> dict[str, float]:
    """Every measurement's drift: the range across a set's campaigns.

    The drift is the machine's own run-to-run movement over the set's three
    campaigns, and it is the yardstick's only input.
    """
    return {key: max(values) - min(values) for key, values in parts.items() if values}


def join_cost_yardstick(parts: dict[str, list[float]], path: str) -> dict[str, float]:
    """Each leaf's yardstick, by the rule.

    The yardstick for a leaf is the **sum** of the drifts of the measurements
    that leaf stands on, and the **floor's** drift, and the **larger** of the
    two. The sum is the conservative choice, because the yardstick judges a
    **difference**: the difference of two drifting series moves by up to the sum
    of the two drifts, so the larger of them was never an upper bound on what
    the machine could have produced on its own. The floor's drift is the second
    floor, the machine's own measure of a memory-bound loop.
    """
    drifts = join_cost_drift(parts)
    floor = drifts.get(JOIN_COST_FLOOR, 0.0)
    yardsticks: dict[str, float] = {}
    for key, _, _, _, sides in JOIN_COST_LADDER[path]:
        sides_drifts = [drifts[side] for side in sides if side in drifts]
        yardsticks[key] = max([sum(sides_drifts), floor])
    return yardsticks


def join_cost_yardstick_file_aware(parts: dict[str, list[float]], path: str) -> dict[str, float]:
    """Each leaf's yardstick, with the file's state in it.

    The yardstick for a leaf is the **sum** of the drifts of the measurements
    that leaf stands on, the **floor's** drift, and the **file-state floor's**
    drift -- the **larger** of them. So the yardstick covers what the machine
    **and the file** could have produced on their own.
    """
    drifts = join_cost_drift(parts)
    floor = drifts.get(JOIN_COST_FLOOR, 0.0)
    file_state = drifts.get(JOIN_COST_FILE_FLOOR, 0.0)
    yardsticks: dict[str, float] = {}
    for key, _label, _places, _word, sides in JOIN_COST_LADDER[path]:
        sides_drifts = [drifts[side] for side in sides if side in drifts]
        yardsticks[key] = max([sum(sides_drifts), floor, file_state])
    return yardsticks


def join_cost_verdict(
    parts: dict[str, list[float]],
    derived: dict[str, list[float]],
    path: str,
    yardsticks: dict[str, float] | None = None,
) -> dict[str, object]:
    """The context's verdict, in one of the six words, the yardstick it was judged
    against, and the state of that test.

    The **yardstick** is the machine's own drift, by the rule: the larger of the
    drifts of the measurements the leaf stands on and of the floor's drift. A
    leaf whose own spread is wider than the yardstick is **inconclusive** and is
    not counted; a leaf whose value is outside the yardstick is a part of the
    context; anything else is at the noise. The verdict is the word of the
    largest leaf that counts, and **unattributed** when no leaf does.

    The verdict's own **state** says whether the test could tell: **sound** when
    the yardstick could measure what it judged and at least one leaf could be
    judged, **not sound** when the yardstick was narrower than the machine's own
    movement, and **inconclusive** when no leaf could be judged at all.

    A yardstick that sees the file's state as well is passed in, and the same
    rules are applied to it.
    """
    context = derived.get(f"the_context_{path}", [])
    if not context:
        return {}
    if yardsticks is None:
        yardsticks = join_cost_yardstick(parts, path)
    leaves: list[tuple[str, str, float, float, float, str]] = []
    for key, label, _, word, _sides in JOIN_COST_LADDER[path]:
        values = [value for value in derived.get(key, []) if value is not None]
        if not values:
            continue
        value = statistics.median(values)
        spread = max(values) - min(values)
        yardstick = yardsticks[key]
        if spread > yardstick:
            state = "inconclusive"
        elif abs(value) > yardstick:
            state = "a part of the context"
        else:
            state = "at the noise"
        leaves.append((label, word, value, spread, yardstick, state))
    if not leaves:
        # No leaves: the code does not report the join stage's parts, so there
        # is no ladder to judge and no verdict to publish.
        return {}
    counted = [abs(value) for _, _, value, _, _, state in leaves if state == "a part of the context"]
    verdict = "unattributed"
    if counted:
        largest = max(counted)
        verdict = next(
            word for _, word, value, _, _, state in leaves
            if state == "a part of the context" and abs(value) == largest
        )
    floor = join_cost_drift(parts).get(JOIN_COST_FLOOR, 0.0)
    if all(state == "inconclusive" for _, _, _, _, _, state in leaves):
        verdict_state = "inconclusive"
    elif yardsticks and min(yardsticks.values()) < floor:
        # The rule takes the floor's drift as its floor, so a yardstick narrower
        # than it means a later change made the rule narrower.
        verdict_state = "not sound"
    else:
        verdict_state = "sound"
    return {
        "yardsticks": yardsticks,
        "parts": leaves,
        "verdict": verdict,
        "state": verdict_state,
    }


def join_cost_query_cost(runs: list[dict]) -> dict[str, float]:
    """The query's cost under the two orders, and the difference between them.

    The query's own cost -- the join stage, as the query pays it -- must not
    depend on the order the instrument put the query in. The even iterations
    take the query first and the odd ones take it last, so the difference
    between the two orders' medians is what the order could have moved the
    published figure by.

    It is the **median across the campaigns** of the per-campaign difference,
    the same reduction every other figure here uses, and the threshold it is
    measured against is **the floor's own spread** -- the machine's own measure
    of what a change of this kind costs.
    """
    differences: dict[str, list[float]] = {}
    for run in runs:
        series = run.get("series_ms", {})
        for path, key in (("cold", "stage_cold"), ("resident", "stage_resident")):
            values = series.get(key)
            if not values or len(values) < 2:
                continue
            first = statistics.median(values[0::2])
            last = statistics.median(values[1::2])
            differences.setdefault(path, []).append(abs(first - last))
    return {path: statistics.median(values) for path, values in differences.items()}


def _rendered(figure: float, places: int) -> str:
    """A figure as the document writes it.

    A negative zero is a zero: the difference of two equal numbers is a
    rounding error of the last bit, and the document writes a zero.
    """
    rounded = round(figure, places)
    if rounded == 0:
        rounded = 0.0
    return f"{rounded:.{places}f}"


def _join_cost_figures(
    files: list[tuple[str, list[dict]]],
) -> tuple[dict[str, list[float]], dict[str, list[float]]]:
    """The join probe's figures, across the campaigns.

    The report has two parts: the **parts** -- the stage on both paths, the
    build side's open, read, keys and table, the probe measured both ways, the
    floor and the candidate -- and the **differences**, which the probe
    computes from its own parts.

    This collects them across the campaigns, so the median is taken over the
    campaigns rather than over the iterations inside one of them.
    """
    parts: dict[str, list[float]] = {}
    derived: dict[str, list[float]] = {}
    for _, runs in files:
        for run in runs:
            for key, value in run.get("parts_ms", {}).items():
                parts.setdefault(key, []).append(value)
            for key, value in run.get("derived_ms", {}).items():
                derived.setdefault(key, []).append(value)
    return parts, derived


def print_join_cost(sets: list[tuple[str, list[tuple[str, list[dict]]]]]) -> None:
    """Print the join probe's medians.

    This is spec 026: the join's stage, taken apart -- the engine's own stage
    on both paths, the build side's open, read, keys and table, the probe
    measured both ways, the floor the probe cannot go under, and the candidate
    that is the cross-check. The "before" set is the same probe against the
    code the increment replaced, measured back to back on the same host.

    Spec 028 measures the probe's remainder, so the instrument gained two
    parts -- the candidate with the overflow map's consult in it, and the
    candidate inside the reader's own batch loop.

    Spec 029 measures the remainder's **parts**, so the instrument gained five
    more -- the chain, the width, the state through `self`, the map's option,
    and the engine's own lookup over a plain slice -- and the section publishes
    the two sets this increment measured, the two that spec 028 measured, and
    the two that spec 026 measured. Every set is preserved under its own name.
    """
    for label, files in sets:
        if not files:
            continue
        parts, derived = _join_cost_figures(files)
        campaigns = min(len(runs) for _, runs in files)
        print(f"\n=== THE JOIN'S STAGE, TAKEN APART -- {label} ({campaigns} campaigns each) ===")

        print("\n-- the parts --")
        print(f"{'the part':<52}{'median':>12}")
        for key, part in JOIN_COST_PARTS:
            values = parts.get(key, [])
            if not values:
                continue
            print(f"{part:<52}{statistics.median(values):>12.3f}")

        print("\n-- the differences --")
        print(f"{'the difference':<52}{'median':>12}")
        for key, difference in JOIN_COST_DERIVED:
            values = derived.get(key, [])
            if not values:
                continue
            print(f"{difference:<52}{statistics.median(values):>12.3f}")

        # The ladder: the context's parts, each with its own spread, and the
        # verdict they give. The spread is the **range** of the per-campaign
        # values, and the yardstick is the machine's own drift.
        if any(key in derived for key, _, _ in JOIN_COST_LADDER_FIGURES):
            print("\n-- the context's ladder --")
            print(f"{'the part':<44}{'median':>12}{'spread':>12}")
            for key, label, places in JOIN_COST_LADDER_FIGURES:
                values = [value for value in derived.get(key, []) if value is not None]
                if not values:
                    continue
                spread = max(values) - min(values)
                print(f"{label:<44}{_rendered(statistics.median(values), places):>12}{spread:>12.3f}")

            # The drifts: the machine's own movement over the set's campaigns.
            # They are the yardstick's only input.
            print("\n-- the drifts --")
            print(f"{'the measurement':<44}{'drift':>12}")
            for key, drift in sorted(join_cost_drift(parts).items()):
                print(f"{key:<44}{drift:>12.3f}")

            for path in ("cold", "resident"):
                verdict = join_cost_verdict(parts, derived, path)
                if not verdict:
                    continue
                print(f"\n-- the {path} path: the yardstick is the machine's own drift --")
                print(f"{'the part':<44}{'value':>12}{'spread':>12}{'yardstick':>12}  the state")
                for label, word, value, spread, yardstick, state in verdict["parts"]:
                    print(f"{label:<44}{value:>12.3f}{spread:>12.3f}{yardstick:>12.3f}  {state} ({word})")
                print(f"the verdict: {verdict['verdict']} ({verdict['state']})")

            # The query's cost under the two orders: the interleaving's own
            # effect on the figure that is published, measured against the
            # floor's own spread.
            query_cost = join_cost_query_cost([run for _, runs in files for run in runs])
            if query_cost:
                floor = join_cost_drift(parts).get(JOIN_COST_FLOOR, 0.0)
                print("\n-- the query's cost, under the two orders --")
                print(f"{'the path':<44}{'the difference':>12}{'the floor':>12}")
                for path, difference in query_cost.items():
                    print(f"{path:<44}{difference:>12.3f}{floor:>12.3f}")


def print_join_parts(runs: list[dict]) -> None:
    """Print the join-orders query, on both sides, taken apart.

    This is spec 046: the engine's cold `join-orders` query is behind
    `DuckDB`'s, and every part of the engine's own analysis so far was set
    against `DuckDB`'s **whole** -- and a part cannot be compared with a whole.

    So both sides are taken apart, in one session, and the parts are named the
    same way on both sides. The third arm -- `DuckDB` with its join filter
    pushdown **disabled** -- changes exactly one thing and therefore names the
    mechanism instead of assuming it.

    The **paired** difference is the figure: both sides are measured back to
    back in the same iteration and the order is alternated, so the machine's
    drift cannot land on one side. A difference of two medians would let the
    drift land on both.
    """
    iterations = _join_parts_iterations(runs)
    campaigns = len(runs)
    print(
        f"\n=== THE JOIN-ORDERS QUERY, ON BOTH SIDES, TAKEN APART "
        f"({campaigns} campaigns, {len(iterations)} iterations) ==="
    )

    print("\n-- the parts --")
    header = f"{'the part':<26}" + "".join(
        f"{JOIN_PARTS_SIDE_LABELS[side]:>26}" for side in JOIN_PARTS_SIDES
    )
    print(header)
    for key, label in JOIN_PARTS_PARTS:
        row = f"{label:<26}"
        for side in JOIN_PARTS_SIDES:
            values = _join_parts_side_values(iterations, side, key)
            if not values:
                row += f"{'-':>26}"
                continue
            row += (
                f"{statistics.median(values):>15.3f}"
                f" [{min(values):>6.2f},{max(values):>6.2f}]"
            )
        print(row)

    # The unprofiled arm has no parts: it is the whole, the way the comparison
    # harness measures it, and it is what the profiler's price is read against.
    unprofiled = _join_parts_side_values(iterations, "duckdb-unprofiled", "total")
    if unprofiled:
        print(
            f"{'the whole, unprofiled':<26}"
            f"{statistics.median(unprofiled):>15.3f}"
            f" [{min(unprofiled):>6.2f},{max(unprofiled):>6.2f}]"
        )

    print("\n-- the paired differences, same iteration, back to back --")
    print(f"{'the difference':<46}{'median':>11}{'min':>11}{'max':>11}")
    for label, left, right, key in JOIN_PARTS_DIFFERENCES:
        values = _join_parts_paired(iterations, left, right, key)
        if not values:
            continue
        print(
            f"{label:<46}{statistics.median(values):>11.3f}"
            f"{min(values):>11.3f}{max(values):>11.3f}"
        )

    print("\n-- the rows each side's join saw --")
    for side in JOIN_PARTS_SIDES:
        values = _join_parts_rows(iterations, side)
        if values:
            print(f"{JOIN_PARTS_SIDE_LABELS[side]:<26}{min(values):>12} - {max(values):>12}")


# The sides, and the label each is published under.
JOIN_PARTS_SIDES = ("engine", "duckdb", "duckdb-filter-off")
JOIN_PARTS_SIDE_LABELS = {
    "engine": "the engine",
    "duckdb": "duckdb, as it runs",
    "duckdb-unprofiled": "duckdb, unprofiled",
    "duckdb-filter-off": "duckdb, the filter off",
}

# The parts, and the label each is published under.
JOIN_PARTS_PARTS = (
    ("total", "the whole"),
    ("probe_side_read_ms", "the probe side's read"),
    ("build_side_read_ms", "the build side's read"),
    ("build_side_open_ms", "the build side's open"),
    ("join_ms", "the join"),
    ("projection_ms", "the projection"),
    ("aggregate_ms", "the aggregate"),
    ("outside_parts_ms", "outside the parts"),
)

# The paired differences: the left side's figure less the right side's, taken
# in the same iteration.
JOIN_PARTS_DIFFERENCES = (
    ("the whole: the engine less duckdb", "engine", "duckdb", "total"),
    ("the probe side's read: the engine less duckdb", "engine", "duckdb", "probe_side_read_ms"),
    ("the build side's read: the engine less duckdb", "engine", "duckdb", "build_side_read_ms"),
    ("the join: the engine less duckdb", "engine", "duckdb", "join_ms"),
    ("the projection: the engine less duckdb", "engine", "duckdb", "projection_ms"),
    ("the aggregate: the engine less duckdb", "engine", "duckdb", "aggregate_ms"),
    ("outside the parts: the engine less duckdb", "engine", "duckdb", "outside_parts_ms"),
    ("the join: duckdb with the filter off less duckdb", "duckdb-filter-off", "duckdb", "join_ms"),
    ("the join: duckdb with the filter off less the engine", "duckdb-filter-off", "engine", "join_ms"),
    (
        "the probe side's read: duckdb with the filter off less duckdb",
        "duckdb-filter-off",
        "duckdb",
        "probe_side_read_ms",
    ),
)


def _join_parts_iterations(runs: list[dict]) -> list[dict]:
    """Every iteration of every campaign, with its arms by side."""
    iterations = []
    for run in runs:
        for iteration in run.get("iterations", []):
            arms = {arm["side"]: arm for arm in iteration.get("arms", [])}
            iterations.append(
                {
                    "index": iteration.get("index"),
                    "order": iteration.get("order"),
                    "load_average": iteration.get("load_average"),
                    "arms": arms,
                }
            )
    return iterations


def _join_parts_side_values(iterations: list[dict], side: str, key: str) -> list[float]:
    """One part's values, across every iteration, for one side."""
    values = []
    for iteration in iterations:
        arm = iteration["arms"].get(side)
        if not arm:
            continue
        if key == "total":
            values.append(arm["total_ms"])
            continue
        parts = arm.get("parts") or {}
        if key in parts:
            values.append(parts[key])
    return values


def _join_parts_paired(
    iterations: list[dict], left: str, right: str, key: str
) -> list[float]:
    """The paired difference of one part, in the same iteration."""
    values = []
    for iteration in iterations:
        arms = iteration["arms"]
        if left not in arms or right not in arms:
            continue
        left_value = _join_parts_one(arms[left], key)
        right_value = _join_parts_one(arms[right], key)
        if left_value is None or right_value is None:
            continue
        values.append(left_value - right_value)
    return values


def _join_parts_one(arm: dict, key: str) -> float | None:
    """One figure of one arm."""
    if key == "total":
        return arm.get("total_ms")
    parts = arm.get("parts") or {}
    return parts.get(key)


def _join_parts_rows(iterations: list[dict], side: str) -> list[int]:
    """How many rows reached each side's join."""
    values = []
    for iteration in iterations:
        arm = iteration["arms"].get(side)
        if not arm:
            continue
        parts = arm.get("parts") or {}
        if "rows_to_join" in parts:
            values.append(parts["rows_to_join"])
    return values


# The key-filter probe's ladder: the rungs, and the label each is published
# under.
KEY_FILTER_RUNGS = (
    ("table_build_ms", "the table's own construction"),
    ("filter_build_ms", "the filter's own construction"),
    ("floor_table_ms", "the table's own load"),
    ("floor_filter_ms", "the filter's own load"),
    ("candidate_table_ms", "the probe as it is"),
    ("candidate_filter_ms", "the filter in front of the table"),
    ("candidate_compacted_ms", "the filter compacting the rows"),
    ("candidate_survivors_ms", "the probe over the survivors alone"),
    ("engine_probe_ms", "the engine's own probe (the control)"),
)

# The paired verdicts: the two sides of each are measured back to back in the
# same iteration, with the order alternated, and the report carries the
# difference already computed.
KEY_FILTER_VERDICTS = (
    (
        "the_filter_s_verdict_ms",
        "the filter in front of the table, less the probe as it is",
    ),
    (
        "the_compaction_s_verdict_ms",
        "the filter compacting the rows, less the probe as it is",
    ),
    (
        "the_prize_ms",
        "the prize: the probe as it is, less the probe over the survivors",
    ),
)


def _key_filter_samples(runs: list[dict], key: str) -> list[float]:
    """One family's samples, pooled over every campaign.

    The pooling is the house's rule: every iteration of every campaign is a
    sample, and the published figure is the median of the pooled samples --
    the same rule the join-parts probe follows.
    """
    samples: list[float] = []
    for run in runs:
        series = run.get(key) or {}
        samples.extend(float(value) for value in series.get("samples", []))
    return samples


def print_key_filter(runs: list[dict]) -> None:
    """Print the key-filter probe's ladder, and its paired verdicts.

    This is spec 047: what a filter over the build side's keys costs, and
    what it saves. The ladder is the house's -- the two constructions, the two
    floors, the probe as it is, the two filter shapes and the control -- and
    the differences are the **paired** ones: both sides are measured back to
    back in the same iteration and the order is alternated, so the machine's
    drift cannot land on one side.

    The rows the filter lets through are printed with the figures, because a
    filter that changed the answer would not be a filter.
    """
    if not runs:
        return

    iterations = [
        iteration
        for run in runs
        for iteration in run.get("the_iterations", [])
    ]
    campaigns = len(runs)
    probe = runs[0].get("probe", "key-filter")

    print(
        f"\n=== THE KEY FILTER ({probe} probe, "
        f"{campaigns} campaigns, {len(iterations)} iterations) ==="
    )
    print(
        f"the probe's {runs[0].get('probe_rows', '?')} rows over the build side's "
        f"{runs[0].get('build_keys', '?')} keys"
    )

    print("\n-- the ladder --")
    print(f"{'the rung':<46}{'median':>11}{'min':>11}{'max':>11}")
    for key, label in KEY_FILTER_RUNGS:
        values = _key_filter_samples(runs, key)
        if not values:
            continue
        print(
            f"{label:<46}{statistics.median(values):>11.3f}"
            f"{min(values):>11.3f}{max(values):>11.3f}"
        )

    print("\n-- the paired differences, same iteration, back to back --")
    print(f"{'the difference':<64}{'median':>11}{'min':>11}{'max':>11}{'samples':>10}")
    for key, label in KEY_FILTER_VERDICTS:
        values = _key_filter_samples(runs, key)
        if not values:
            continue
        print(
            f"{label:<64}{statistics.median(values):>11.3f}"
            f"{min(values):>11.3f}{max(values):>11.3f}{len(values):>10}"
        )

    print("\n-- the correctness --")
    rows_agree = {bool(run.get("the_rows_agree")) for run in runs}
    engine_agrees = {bool(run.get("the_engine_agrees")) for run in runs}
    print(f"the rows agree in every campaign: {rows_agree == {True}}")
    print(f"the engine agrees in every campaign: {engine_agrees == {True}}")
    through = [value for run in runs for value in run.get("rows_through", [])]
    if through:
        print(f"the rows the filter lets through: {min(through)} - {max(through)}")

    loads = [
        iteration["load_average"]
        for iteration in iterations
        if iteration.get("load_average") is not None
    ]
    if loads:
        print(f"{'the host load average':<46}{min(loads):>12.2f} - {max(loads):>12.2f}")


# Spec 048's probe-paths: the engine's own probe, in the two paths it runs in.
#
# The rungs are the **same probe**, measured three times and once inside the
# reader: cold, resident, after the reader's work, and inside the reader.
PROBE_PATHS_RUNGS = (
    ("ladder_path_ms", "the ladder's path, the table cold"),
    ("ladder_warm_ms", "the ladder's path again, the table resident"),
    ("ladder_last_ms", "the ladder's path once more, after the reader's work"),
    ("query_path_ms", "the query's path, inside the reader"),
)

# The paired verdicts: both sides are measured back to back in the same
# iteration, so the machine's drift cannot land on one side.
PROBE_PATHS_VERDICTS = (
    ("the_gap_ms", "the query's path less the ladder's path"),
    ("the_residency_ms", "the table's residency: cold less resident"),
    ("the_readers_cost_ms", "what the reader's work costs the table"),
)


def _probe_paths_samples(runs: list[dict], key: str) -> list[float]:
    """One family's samples, pooled over every campaign.

    The pooling is the house's rule: every iteration of every campaign is a
    sample, and the published figure is the median of the pooled samples.
    """
    samples: list[float] = []
    for run in runs:
        series = run.get(key) or {}
        samples.extend(float(value) for value in series.get("samples", []))
    return samples


def _probe_paths_own_point(runs: list[dict]) -> tuple[int, int] | None:
    """The sweep's point that is the engine's own, and its batch size.

    **Read from the report, not assumed**: the report says which batch size
    the reader arrived in on the engine's own path, and the sweep's points are
    named in rows.
    """
    for run in runs:
        own = run.get("the_engine_s_own_batch_rows") or []
        points = run.get("sweep_batch_rows") or []
        if not own or not points:
            continue
        for index, rows in enumerate(points):
            if int(rows) == int(own[0]):
                return index, int(rows)
    return None


def _probe_paths_sweep_values(runs: list[dict], point: int) -> list[float]:
    """One sweep point's own times, across every iteration."""
    values: list[float] = []
    for run in runs:
        for iteration in run.get("the_iterations", []):
            entry = next(
                (e for e in (iteration.get("sweep") or []) if e.get("point") == point), None
            )
            if entry is not None:
                values.append(float(entry["ms"]))
    return values


def _probe_paths_sweep(runs: list[dict], point: int, own: int) -> list[float]:
    """One sweep point's paired differences against the engine's own.

    **Both sides are measured in the same iteration** -- the sweep's rotation
    puts every point in every position -- so the difference is the paired one,
    not the difference of two medians.
    """
    values: list[float] = []
    for run in runs:
        for iteration in run.get("the_iterations", []):
            sweep = iteration.get("sweep") or []
            mine = next((entry for entry in sweep if entry.get("point") == point), None)
            theirs = next((entry for entry in sweep if entry.get("point") == own), None)
            if mine is None or theirs is None:
                continue
            values.append(float(mine["ms"]) - float(theirs["ms"]))
    return values


def print_probe_paths(runs: list[dict], label: str = "the query's own shape") -> None:
    """Print the engine's own probe, in the two paths it runs in.

    This is spec 048: the engine's probe is measured in the **ladder's path**
    -- over a materialised column, outside the reader -- and in the **query's
    path** -- inside the reader, between the reader's own work -- in the same
    iteration, back to back, with the order alternated.

    The same probe is measured three times in the ladder's path: **cold**,
    right after the materialisation; **resident**, at once, with nothing in
    between; and once more **after the reader's work**. The difference
    between the first two is the build table's residency, and the difference
    between the last two is what the reader's work costs the table.
    """
    if not runs:
        return

    iterations = [iteration for run in runs for iteration in run.get("the_iterations", [])]
    campaigns = len(runs)
    probe = runs[0].get("probe", "probe-paths")

    print(
        f"\n=== THE PROBE, TWICE ({label}, {probe} probe, "
        f"{campaigns} campaigns, {len(iterations)} iterations) ==="
    )
    order = runs[0].get("the_probe_side_s_key_order") or {}
    if order:
        print(
            f"the probe side's keys fall {order.get('descents', '?')} times in "
            f"{order.get('rows', '?')} rows"
        )

    print("\n-- the paths --")
    print(f"{'the path':<46}{'median':>11}{'min':>11}{'max':>11}")
    for key, rung in PROBE_PATHS_RUNGS:
        values = _probe_paths_samples(runs, key)
        if not values:
            continue
        print(
            f"{rung:<46}{statistics.median(values):>11.3f}"
            f"{min(values):>11.3f}{max(values):>11.3f}"
        )

    print("\n-- the paired differences, same iteration, back to back --")
    print(f"{'the difference':<64}{'median':>11}{'min':>11}{'max':>11}{'samples':>10}")
    for key, rung in PROBE_PATHS_VERDICTS:
        values = _probe_paths_samples(runs, key)
        if not values:
            continue
        print(
            f"{rung:<64}{statistics.median(values):>11.3f}"
            f"{min(values):>11.3f}{max(values):>11.3f}{len(values):>10}"
        )

    own = _probe_paths_own_point(runs)
    if own is not None:
        point, rows = own
        print("\n-- the sweep: the reader's path at each batch size --")
        print(f"{'the batch size':<24}{'median':>11}{'less the engine\'s own':>24}{'faster':>12}")
        for index, size in enumerate(runs[0].get("sweep_batch_rows", [])):
            own_values = _probe_paths_sweep_values(runs, index)
            if not own_values:
                continue
            median = statistics.median(own_values)
            line = f"{int(size):>23,}{median:>11.3f}"
            if index == point:
                line += f"{'-':>24}{'-':>12}"
            else:
                values = _probe_paths_sweep(runs, index, point)
                faster = sum(1 for value in values if value < 0)
                line += (
                    f"{statistics.median(values):>+24.3f}"
                    f"{faster:>7}/{len(values):<4}"
                )
            print(line)
        print(f"the engine's own batch size: {rows:,} rows")

    print("\n-- the correctness --")
    rows_agree = {bool(run.get("the_rows_agree")) for run in runs}
    print(f"the rows agree in every campaign: {rows_agree == {True}}")
    rows = [value for run in runs for value in run.get("the_rows", [])]
    if rows:
        print(f"the rows the probe keeps: {min(rows)} - {max(rows)}")

    loads = [
        iteration["load_average"]
        for iteration in iterations
        if iteration.get("load_average") is not None
    ]
    if loads:
        print(f"{'the host load average':<46}{min(loads):>12.2f} - {max(loads):>12.2f}")


def print_join_parts_batch_sweep(sets: list[tuple[str, list[dict]]]) -> None:
    """Print the query's own parts, at each of the reader's batch sizes.

    This is spec 048's other half: the engine's probe is only worth a
    section if the **query** pays it, so the join-orders query is taken apart
    at every batch size, the engine's own included.

    The **paired** difference against `DuckDB` is the figure: both sides are
    measured back to back in the same iteration and the order is alternated.
    """
    if not sets:
        return
    print("\n=== THE QUERY'S OWN PARTS, AT EACH BATCH SIZE ===")
    print(f"{'the batch size':<30}{'the engine':>13}{'duckdb':>11}{'paired':>11}{'the scan':>12}")
    for label, runs in sets:
        iterations = _join_parts_iterations(runs)
        engine = _join_parts_side_values(iterations, "engine", "join_ms")
        duckdb = _join_parts_side_values(iterations, "duckdb", "join_ms")
        paired = _join_parts_paired(iterations, "engine", "duckdb", "join_ms")
        scan = _join_parts_side_values(iterations, "engine", "probe_side_read_ms")
        if not engine:
            continue
        print(
            f"{label:<30}{statistics.median(engine):>13.3f}"
            f"{statistics.median(duckdb):>11.3f}{statistics.median(paired):>+11.3f}"
            f"{statistics.median(scan):>12.3f}"
        )
    for label, runs in sets:
        iterations = _join_parts_iterations(runs)
        paired = _join_parts_paired(iterations, "engine", "duckdb", "join_ms")
        if paired:
            print(
                f"{label:<30}the engine is slower in "
                f"{sum(1 for value in paired if value > 0)} of {len(paired)} iterations"
            )


# The ladder's leaves, per path: the figure, the leaf's name, and **the sides
# the leaf stands on** -- the measurements whose drifts the yardstick takes.
#
# The sides are the same ones the guard uses, derived here again and not
# imported: two derivations that agree are a check, one that imports the other
# is not.
CONTEXT_LEAVES = {
    "cold": (
        ("the build side's gap", "the_build_gap_cold", ("engine_build_cold", "build_whole")),
        ("the open's gap", "the_open_gap_cold", ("engine_open_cold", "build_open")),
        (
            "the probe's row-number offset",
            "the_probe_row_offset",
            ("probe_batches", "probe_batches_no_offset"),
        ),
        (
            "the probe's own remainder",
            "the_probe_residual_cold",
            ("engine_probe_cold", "probe_batches_no_offset"),
        ),
        ("the part in no gap", "the_host_remainder_cold", ("stage_cold",)),
    ),
    "resident": (
        ("the build side's gap", "the_build_gap_resident", ("engine_build_resident", "build_whole")),
        ("the open's gap", "the_open_gap_resident", ("engine_open_resident", "build_open")),
        ("the probe's file state", "the_probe_file_state", ("probe_held", "probe_column")),
        ("the probe's own remainder", "the_probe_residual_resident", ("engine_probe_resident", "probe_held")),
        ("the part in no gap", "the_host_remainder_resident", ("stage_resident",)),
    ),
}

# The two floors: the machine's own measure and the file's own read.
CONTEXT_MACHINE_FLOOR = "floor_probe"
CONTEXT_FILE_FLOOR = "build_read"


def context_drift(runs: list[dict]) -> dict[str, float]:
    """Each part's drift: the range of its medians over the campaigns."""
    medians: dict[str, list[float]] = {}
    for run in runs:
        for name, value in run["parts_ms"].items():
            medians.setdefault(name, []).append(value)
    return {name: max(values) - min(values) for name, values in medians.items()}


def context_yardsticks(runs: list[dict], path: str, file_aware: bool = True) -> dict[str, float]:
    """Each leaf's yardstick, with the file's state in it.

    The yardstick is the sum of the drifts of the measurements the leaf stands
    on, the machine's own drift, and the **file-state floor's** drift -- the
    larger of them.
    """
    drifts = context_drift(runs)
    machine = drifts.get(CONTEXT_MACHINE_FLOOR, 0.0)
    file_state = drifts.get(CONTEXT_FILE_FLOOR, 0.0)
    yardsticks: dict[str, float] = {}
    for _name, key, sides in CONTEXT_LEAVES[path]:
        sides_drifts = [drifts[side] for side in sides if side in drifts]
        floors = [sum(sides_drifts), machine]
        if file_aware:
            floors.append(file_state)
        yardsticks[key] = max(floors)
    return yardsticks


def context_states(runs: list[dict], path: str) -> dict[str, str]:
    """Each leaf's state: at the noise, inconclusive, or a part of the context."""
    yardsticks = context_yardsticks(runs, path)
    states: dict[str, str] = {}
    for _name, key, _sides in CONTEXT_LEAVES[path]:
        values = [run["derived_ms"][key] for run in runs]
        value = sorted(values)[len(values) // 2]
        own = max(values) - min(values)
        if own > yardsticks[key]:
            states[key] = "inconclusive"
        elif abs(value) > yardsticks[key]:
            states[key] = "a part of the context"
        else:
            states[key] = "at the noise"
    return states


def context_verdict(runs: list[dict], path: str) -> str:
    """The path's verdict: the word of the largest leaf that counts."""
    states = context_states(runs, path)
    yardsticks = context_yardsticks(runs, path)
    words = {
        "the_build_gap_cold": "the build side's",
        "the_build_gap_resident": "the build side's",
        "the_open_gap_cold": "the clock's",
        "the_open_gap_resident": "the clock's",
        "the_probe_row_offset": "the clock's",
        "the_probe_file_state": "the memory's",
        "the_probe_residual_cold": "the probe's",
        "the_probe_residual_resident": "the probe's",
        "the_host_remainder_cold": "the host's",
        "the_host_remainder_resident": "the host's",
    }
    counted = [
        (abs(sorted([run["derived_ms"][key] for run in runs])[len(runs) // 2]), key)
        for key, state in states.items()
        if state == "a part of the context"
    ]
    if not counted:
        return "unattributed"
    _size, key = max(counted)
    return words[key]


def print_context_accounts(sets: list[tuple[str, list[dict]]]) -> None:
    """Print the context's two sides, in one account: the ladder and the verdict.

    This is spec 053: the context is a difference between the query's account of
    the join stage and the standalone's account of the same work, and the two
    were taken hundreds of milliseconds apart.  The iteration is now two path
    accounts, each holding the path's query and the path's standalone work back
    to back, and the whole iteration is reversed in the odd iterations.

    So the ladder is printed per path -- the leaves, their spreads, the two
    yardsticks and their states -- and the verdict with it, and the separation
    is printed in the two geometries it now has.
    """
    leaves = {
        "cold": (
            ("the build side's gap", "the_build_gap_cold"),
            ("the open's gap", "the_open_gap_cold"),
            ("the probe's row-number offset", "the_probe_row_offset"),
            ("the probe's own remainder", "the_probe_residual_cold"),
            ("the part in no gap", "the_host_remainder_cold"),
        ),
        "resident": (
            ("the build side's gap", "the_build_gap_resident"),
            ("the open's gap", "the_open_gap_resident"),
            ("the probe's file state", "the_probe_file_state"),
            ("the probe's own remainder", "the_probe_residual_resident"),
            ("the part in no gap", "the_host_remainder_resident"),
        ),
    }
    for label, runs in sets:
        if not runs:
            continue
        print(f"\n=== THE CONTEXT'S TWO SIDES -- {label} ({len(runs)} campaigns) ===")
        for path in ("cold", "resident"):
            def median(values: list[float]) -> float:
                ordered = sorted(values)
                return ordered[len(ordered) // 2]

            def spread(values: list[float]) -> float:
                return max(values) - min(values)

            context = [run["derived_ms"][f"the_context_{path}"] for run in runs]
            print(f"\n-- the {path} path: the context {median(context):.3f} ms,"
                  f" its spread {spread(context):.3f} ms")
            yardsticks = context_yardsticks(runs, path)
            machine_yardsticks = context_yardsticks(runs, path, file_aware=False)
            states = context_states(runs, path)
            for name, key in leaves[path]:
                values = [run["derived_ms"][key] for run in runs]
                value = median(values)
                own = spread(values)
                print(f"   {name:<48} {value:8.3f} ms   spread {own:7.3f} ms"
                      f"   machine {machine_yardsticks[key]:7.3f} ms"
                      f"   file-aware {yardsticks[key]:7.3f} ms"
                      f"   {states[key]}")
            print(f"   the campaigns' contexts: {[round(v, 3) for v in context]}")
            print(f"   the verdict: {context_verdict(runs, path)}")
        for name in ("the_separation_cold", "the_separation_resident",
                     "the_order_s_cost_cold", "the_order_s_cost_resident",
                     "the_floors_separation", "the_build_file_state",
                     "the_added_work_cost"):
            values = [run["derived_ms"][name] for run in runs]
            print(f"   {name:<48} {[round(v, 3) for v in values]}   median {median(values):.3f}")
        if "separation_series_ms" in runs[0]:
            for path in ("cold", "resident"):
                for leaf in ("the build side's gap", "the open's gap"):
                    evens = [median(run["separation_series_ms"][path][leaf][0::2]) for run in runs]
                    odds = [median(run["separation_series_ms"][path][leaf][1::2]) for run in runs]
                    print(f"   the {path} path's {leaf}: the query first {median(evens):.3f} ms,"
                          f" the standalone first {median(odds):.3f} ms")


def print_drift_ladder(sets: list[tuple[str, list[dict]]]) -> None:
    """Print the drift's own parts: the ladder, and the movement that makes it.

    This is spec 035: the drift is a **range**, and a range is not additive, so
    the decomposition is made of **movements** -- the difference between two
    campaigns -- and the movement is taken on the **means**, because the mean is
    the statistic the parts tile. The drift itself stays the range of the
    medians, and it is printed beside the movement.

    The ladder is taken over the pair of campaigns that make the **context's**
    own drift, so the movement is the one the yardstick is about, and the
    reconciliation says whether the ladder closes: it does, exactly.
    """
    leaves = {
        "cold": (
            ("the build side's gap", "the_build_gap_cold", "the build side's gap"),
            ("the open's gap", "the_open_gap_cold", "the open's gap"),
            ("the probe's file state, less its row-number offset", "the_probe_row_offset", None),
            ("the probe's own remainder", "the_probe_residual_cold", "the probe's own remainder"),
            ("the part in no gap", "the_host_remainder_cold", "the part in no gap"),
        ),
        "resident": (
            ("the build side's gap", "the_build_gap_resident", "the build side's gap"),
            ("the open's gap", "the_open_gap_resident", "the open's gap"),
            ("the probe's file state, less its row-number offset", "the_probe_file_state", None),
            ("the probe's own remainder", "the_probe_residual_resident", "the probe's own remainder"),
            ("the part in no gap", "the_host_remainder_resident", "the part in no gap"),
        ),
    }
    for label, runs in sets:
        if not runs:
            continue
        print(f"\n=== THE DRIFT'S OWN PARTS -- {label} ({len(runs)} campaigns) ===")
        for path in ("cold", "resident"):
            def series_of(leaf: str) -> list[list[float]]:
                return [run["ladder_series_ms"][path][leaf] for run in runs]

            def medians(samples: list[list[float]]) -> list[float]:
                return [sorted(s)[len(s) // 2] for s in samples]

            def means(samples: list[list[float]]) -> list[float]:
                return [sum(s) / len(s) for s in samples]

            context = series_of("the context")
            context_medians = medians(context)
            low = context_medians.index(min(context_medians))
            high = context_medians.index(max(context_medians))
            movement = means(context)[high] - means(context)[low]
            print(f"\n-- the {path} path: the context's drift {max(context_medians) - min(context_medians):.3f} ms"
                  f", the extremes are campaigns {low + 1} and {high + 1} --")
            total = 0.0
            floor_series = [run["series_ms"]["floor_probe"] for run in runs]
            for name, key, series_name in leaves[path]:
                published = [run["derived_ms"][key] for run in runs]
                if series_name is None:
                    fs = series_of("the probe's file state")
                    offset = series_of("the probe's row-number offset")
                    series = [[f - o for f, o in zip(a, b)] for a, b in zip(fs, offset)]
                else:
                    series = series_of(series_name)
                leaf_movement = means(series)[high] - means(series)[low]
                paired = [[a - b for a, b in zip(one, other)] for one, other in zip(series, floor_series)]
                leaf_paired = means(paired)[high] - means(paired)[low]
                total += leaf_movement
                print(f"   {name:<48} drift {max(published) - min(published):8.3f} ms"
                      f"   movement {leaf_movement:+8.3f} ms   paired {leaf_paired:+8.3f} ms")
            floor = [run["parts_ms"]["floor_probe"] for run in runs]
            floor_series = [run["series_ms"]["floor_probe"] for run in runs]
            floor_movement = means(floor_series)[high] - means(floor_series)[low]
            print(f"   {'the floor -- the machine\'s own movement':<48} drift {max(floor) - min(floor):8.3f} ms"
                  f"   movement {floor_movement:+8.3f} ms")
            print(f"   the movement {movement:+.3f} ms, the leaves' sum {total:+.3f} ms,"
                  f" the reconciliation {abs(movement - total):.6f} ms")
            cost = [run["parts_ms"][f"stage_{path}"] for run in runs]
            cost_drift = max(cost) - min(cost)
            print(f"   the cost: {cost[0]:.3f} ms, its drift {cost_drift:.3f} ms,"
                  f" {100.0 * cost_drift / (sum(cost) / len(cost)):.2f}% of it")


def print_file_state_floor(sets: list[tuple[str, list[dict]]]) -> None:
    """Print the file-state floor: the two floors, and the pairing between them.

    This is spec 036: the machine's own measure and the file's own read are
    both measured **in the iteration**, back to back, and the file's own read
    is the build side's read through the engine's own reader, with the engine's
    own predicates -- the file's state, as the engine pays it.

    The **paired** series is the file's own read less the machine's own
    movement, iteration by iteration: the file's share, with the machine's
    common movement taken out. And the **yardstick** that sees the file's state
    as well as the machine's is the larger of the machine's own drift and the
    file-state floor's drift.
    """
    def medians(samples: list[list[float]]) -> list[float]:
        return [sorted(one)[len(one) // 2] for one in samples]

    for label, runs in sets:
        if not runs:
            continue
        print(f"\n=== THE FILE-STATE FLOOR -- {label} ({len(runs)} campaigns) ===")
        machine = [run["series_ms"][JOIN_COST_FLOOR] for run in runs]
        file_state = [run["series_ms"][JOIN_COST_FILE_FLOOR] for run in runs]
        paired = [
            [file - floor for file, floor in zip(file_series, machine_series)]
            for file_series, machine_series in zip(file_state, machine)
        ]
        for name, series in (
            ("the machine's own measure", machine),
            ("the file's own read", file_state),
            ("the file's read less the machine's", paired),
        ):
            medians_of = medians(series)
            print(
                f"   {name:<44} drift {max(medians_of) - min(medians_of):8.3f} ms"
                f"   medians {', '.join(f'{value:.3f}' for value in medians_of)}"
            )
        separations = [
            run["derived_ms"].get("the_floors_separation") for run in runs
        ]
        separations = [value for value in separations if value is not None]
        if separations:
            print(
                "   the two floors' separation: "
                f"{', '.join(f'{value:.3f}' for value in separations)} ms"
            )

        parts: dict[str, list[float]] = {}
        derived: dict[str, list[float]] = {}
        for run in runs:
            for key, value in run.get("parts_ms", {}).items():
                parts.setdefault(key, []).append(value)
            for key, value in run.get("derived_ms", {}).items():
                derived.setdefault(key, []).append(value)
        for path in ("cold", "resident"):
            machine_yardsticks = join_cost_yardstick(parts, path)
            file_yardsticks = join_cost_yardstick_file_aware(parts, path)
            print(f"\n-- the {path} path: the yardsticks, the machine's and the file-aware --")
            for key, leaf, _places, _word, _sides in JOIN_COST_LADDER[path]:
                print(
                    f"   {leaf:<44} the machine's {machine_yardsticks[key]:8.3f} ms"
                    f"   the file-aware {file_yardsticks[key]:8.3f} ms"
                )
            for name, yardsticks in (
                ("the machine's", machine_yardsticks),
                ("the file-aware", file_yardsticks),
            ):
                verdict = join_cost_verdict(parts, derived, path, yardsticks)
                if verdict:
                    print(
                        f"   the verdict under {name} yardstick: "
                        f"{verdict['verdict']} ({verdict['state']})"
                    )


WRITE_SIDE_SHAPES = (
    ("write_row_group", "the write of one row group"),
    ("write_file", "the write of the whole file"),
    ("write_plain", "the write of the whole file, the dictionary off"),
    ("write_manifest", "writing the manifest"),
    ("fsync_data", "the fsync of the data file"),
    ("fsync_manifest", "the fsync of the manifest"),
    ("rename_manifest", "the rename of the manifest"),
    ("fsync_dir", "the fsync of the directory"),
    ("commit", "the commit, all five steps"),
    ("read_scan_original", "the read of the original, two columns"),
    ("read_scan_written", "the read of the file the probe wrote, two columns"),
    ("read_scan_plain", "the read of the plain file, two columns"),
    ("read_all_original", "the read of the original, all sixteen columns"),
    ("read_all_written", "the read of the file the probe wrote, all sixteen columns"),
    ("read_all_plain", "the read of the plain file, all sixteen columns"),
)

WRITE_SIDE_DERIVED = (
    ("ms_per_row_group", "the cost of one row group"),
    ("write_over_read_all", "the write over a full read"),
    ("write_over_read_scan", "the write over a two-column read"),
    ("commit_over_write", "the commit's share of the write"),
    ("fsyncs_over_commit", "the fsyncs' share of the commit"),
    ("bytes_per_row_original", "the bytes a row takes, the original"),
    ("bytes_per_row_written", "the bytes a row takes, the file the probe wrote"),
    ("bytes_per_row_plain", "the bytes a row takes, the plain file"),
)


def print_write_side(sets: list[tuple[str, list[dict]]]) -> None:
    """Print the write side: the write, the commit, the bytes and the read-back.

    This is spec 037: the write side of an embedded store, measured. The
    probe writes the published file's own rows back out, in the file's own
    row groups, with the file's own writer properties -- and it compares the
    result with the original **byte for byte**, so the question "does the
    store's writer reproduce the file" is answered rather than assumed.

    The two sets are the same rows in two batch sizes: the one the published
    file was written in, and the engine's own. The batch size is what decides
    the bytes, because the Parquet writer cuts its mini-batches -- and the
    dictionary's fallback with them -- at the caller's batch boundary.
    """
    for label, runs in sets:
        if not runs:
            continue
        print(f"\n=== THE WRITE SIDE -- {label} ({len(runs)} campaigns) ===")
        batches = sorted({run.get("batch_rows") for run in runs})
        print(f"   the batch size the rows arrive in: {', '.join(str(b) for b in batches)}")
        identical = [run.get("identical") for run in runs]
        print(f"   the file the probe wrote is the original: {all(identical)}")
        for key in ("bytes_original", "bytes_written", "bytes_plain"):
            values = [run.get(key) for run in runs if run.get(key) is not None]
            if values:
                print(f"   {key:<44} {int(median(values)):>12} bytes")

        for name, heading in WRITE_SIDE_SHAPES:
            values = [run["measurements_ms"][name] for run in runs if name in run["measurements_ms"]]
            if not values:
                continue
            print(
                f"   {heading:<44} {median(values):9.3f} ms"
                f"   ({min(values):.3f}-{max(values):.3f})"
            )

        for key, heading in WRITE_SIDE_DERIVED:
            values = [run["derived_ms"][key] for run in runs if key in run.get("derived_ms", {})]
            if not values:
                continue
            print(f"   {heading:<44} {median(values):12.3f}")

        def measured(key: str) -> list[float]:
            return [run["measurements_ms"][key] for run in runs if key in run.get("measurements_ms", {})]

        def counted(key: str) -> list[float]:
            return [run[key] for run in runs if key in run]

        print("\n-- the plain file over the dictionary file --")
        for heading, numerator, denominator in (
            ("the bytes", counted("bytes_plain"), counted("bytes_written")),
            ("the write", measured("write_plain"), measured("write_file")),
            ("the read, all sixteen columns", measured("read_all_plain"), measured("read_all_original")),
        ):
            if numerator and denominator:
                print(
                    f"   {heading:<44} {median(numerator) / median(denominator):12.2f}"
                )

        loads = [run.get("host_load_average") for run in runs if run.get("host_load_average")]
        if loads:
            print(f"   the host's load average: {', '.join(f'{value:.2f}' for value in loads)}")


def print_many_files(runs: list[dict]) -> None:
    """Print the file boundary's cost: the same rows, as one file and as six.

    This is spec 039: the store writes a table as a **set of files** and the
    engine's only source is **one file**, so the cost of the file boundary is
    measured before the engine is taught to read the set. The experiment is
    the published file's own six row groups -- the same rows as the published
    file -- written as six files and read both ways.
    """
    if not runs:
        return
    print(f"\n=== THE FILE BOUNDARY'S COST ({len(runs)} campaigns) ===")
    for name, heading in (
        ("read_one_file", "the read of the one file"),
        ("read_six_files", "the read of the six files"),
    ):
        values = [run["measurements_ms"][name] for run in runs if name in run.get("measurements_ms", {})]
        if values:
            print(f"   {heading:<44} {median(values):10.3f} ms   ({min(values):.3f}-{max(values):.3f})")
    for key, heading in (
        ("boundary_ms", "the boundary's cost"),
        ("boundary_share", "the boundary's share of the one file's read"),
    ):
        values = [run["derived_ms"][key] for run in runs if key in run.get("derived_ms", {})]
        if values:
            print(f"   {heading:<44} {median(values):10.3f}")
    for key, heading in (("files", "the files"), ("rows", "the rows")):
        values = [run[key] for run in runs if key in run]
        if values:
            print(f"   {heading:<44} {int(median(values)):>10}")
    identical = [run.get("identical") for run in runs]
    print(f"   the same rows, row by row: {all(identical)}")
    loads = [run.get("host_load_average") for run in runs if run.get("host_load_average")]
    if loads:
        print(f"   the host's load average: {', '.join(f'{value:.2f}' for value in loads)}")


def print_file_set(runs: list[dict]) -> None:
    """Print the source over a set of files: what it costs, and whether it is right.

    This is spec 040: the store writes a table as a **set of files** and the
    engine's only source is **one file**, so the engine gained a source that
    reads the set as one table. The experiment is the published file's own six
    row groups -- the same rows as the published file -- written as six files
    and read three ways: as the one file, as six files read one by one, and as
    six files read as one table.

    **The yardstick is the one file's read**: the same rows in one file is the
    ideal, and the set's read is measured against it. The six files read one
    by one are measured too -- they are what the source replaces -- but they
    do not answer the query, so they are not the yardstick.
    """
    if not runs:
        return
    print(f"\n=== THE SOURCE OVER A SET OF FILES ({len(runs)} campaigns) ===")
    for name, heading in (
        ("read_one_file", "the read of the one file"),
        ("read_six_files", "the read of the six files, one by one"),
        ("read_file_set", "the read of the six files, as one table"),
        ("read_one_file_parallel", "the parallel read of the one file"),
        ("read_file_set_parallel", "the parallel read of the six, as one table"),
        ("open_one_file", "the open of the one file"),
        ("open_file_set", "the open of the six files"),
    ):
        values = [run["measurements_ms"][name] for run in runs if name in run.get("measurements_ms", {})]
        if values:
            print(f"   {heading:<44} {median(values):10.3f} ms   ({min(values):.3f}-{max(values):.3f})")
    for key, heading in (
        ("source_cost_ms", "the source's cost over the one file's read"),
        ("source_cost_share", "the source's cost, as a share of that read"),
        ("source_cost_parallel_ms", "the source's cost, in parallel"),
        ("source_cost_parallel_share", "the source's cost, in parallel, as a share"),
        ("naive_gap_ms", "the set over the six files read one by one"),
        ("boundary_ms", "the boundary's cost, as spec 039 measured it"),
        ("open_gap_ms", "the open of the six files over the open of the one"),
    ):
        values = [run["derived_ms"][key] for run in runs if key in run.get("derived_ms", {})]
        if values:
            print(f"   {heading:<44} {median(values):10.3f}")
    for key, heading in (("files", "the files"), ("rows", "the rows")):
        values = [run[key] for run in runs if key in run]
        if values:
            print(f"   {heading:<44} {int(median(values)):>10}")
    identical = [run.get("identical") for run in runs]
    print(f"   the same rows, row by row: {all(identical)}")
    counts = [run.get("count_match") for run in runs]
    print(f"   the filtered count agrees: {all(counts)}")
    loads = [run.get("host_load_average") for run in runs if run.get("host_load_average")]
    if loads:
        print(f"   the host's load average: {', '.join(f'{value:.2f}' for value in loads)}")


def print_hostile(runs: list[dict], before_runs: list[dict] | None = None) -> None:
    """Print what a hostile file asks of the reader.

    This is spec 041: the reader is the trust boundary, and the published
    record said a hostile file could ask for four gigabytes. The experiment is
    a **real file with one hostile page header** -- the page's header is
    replaced by a hand-written one whose statistics field declares a length --
    read through the engine's own reader, with a counting global allocator
    around it, so the figure is the **largest single allocation the reader
    asked for**.

    **The declared length is a curve**, and an honest file is the control: a
    reader that allocates what it is told draws a straight line through the
    declared lengths, and a reader that refuses the file asks for the same
    small thing every time.

    **And the set before the bump is printed beside the set after it**, because
    the bump is what closes the hole, and a figure without the figure it
    replaces says nothing about whether it moved.
    """
    if not runs:
        return
    before_runs = before_runs or []
    print(f"\n=== THE HOSTILE FILE ({len(runs)} campaigns) ===")
    control = [run["control"]["largest_allocation_bytes"] for run in runs if "control" in run]
    if control:
        print(f"   {'the honest file, the largest allocation':<44} {int(median(control)):>10}")
    for index, row in enumerate(runs[0].get("hostile", [])):
        declared = row["declared_bytes"]
        largest = [
            run["hostile"][index]["largest_allocation_bytes"]
            for run in runs
            if len(run.get("hostile", [])) > index
        ]
        times = [
            run["hostile"][index]["elapsed_ms"]
            for run in runs
            if len(run.get("hostile", [])) > index
        ]
        header = row["hostile_header_bytes"]
        if largest:
            share = median(largest) / declared if declared else 0.0
            print(
                f"   {f'a declared {declared:,} bytes, the largest allocation':<44} "
                f"{int(median(largest)):>10}   share {share:.3f}"
            )
        if times:
            print(
                f"   {f'  its read, and the header is {header} bytes':<44} "
                f"{median(times):10.3f} ms   ({min(times):.3f}-{max(times):.3f})"
            )
    loads = [run.get("host_load_average") for run in runs if run.get("host_load_average")]
    if loads:
        print(f"   the host's load average: {', '.join(f'{value:.2f}' for value in loads)}")

    if not before_runs:
        return
    print(f"\n-- the same probe, on the tree before the bump ({len(before_runs)} campaigns) --")
    for index, row in enumerate(before_runs[0].get("hostile", [])):
        declared = row["declared_bytes"]
        largest = [
            run["hostile"][index]["largest_allocation_bytes"]
            for run in before_runs
            if len(run.get("hostile", [])) > index
        ]
        if not largest:
            continue
        after = [
            run["hostile"][index]["largest_allocation_bytes"]
            for run in runs
            if len(run.get("hostile", [])) > index
        ]
        share = median(largest) / declared if declared else 0.0
        after_share = median(after) / declared if declared and after else 0.0
        print(
            f"   {f'a declared {declared:,} bytes, the largest allocation':<44} "
            f"{int(median(largest)):>10}   share {share:.3f}"
            f"   ->  {int(median(after)) if after else 0:>10}   share {after_share:.3f}"
        )


def print_store_write(runs: list[dict]) -> None:
    """Print the store's own write: the file it wrote, and the claim it makes.

    This is spec 038: the store writes the published file's own rows, and the
    file it wrote is compared with the published file **byte for byte**. So the
    store's claim is not "it wrote a file" but "**it wrote the file the
    generator wrote, and the two are equal**".
    """
    if not runs:
        return
    print(f"\n=== THE STORE'S OWN WRITE ({len(runs)} campaigns) ===")
    for name, heading in (
        ("write_file", "the store's write"),
        ("commit", "the store's commit"),
        ("append", "the store's own API, end to end"),
    ):
        values = [run["measurements_ms"][name] for run in runs if name in run.get("measurements_ms", {})]
        if values:
            print(f"   {heading:<44} {median(values):10.3f} ms   ({min(values):.3f}-{max(values):.3f})")
    for key, heading in (
        ("rows", "the rows"),
        ("batch_rows", "the grid the rows arrived in"),
        ("row_group_rows", "the row groups"),
    ):
        values = [run[key] for run in runs if key in run]
        if values:
            print(f"   {heading:<44} {int(median(values)):>10}")
    identical = [run.get("identical") for run in runs]
    bytes_written = [run.get("bytes_written") for run in runs if run.get("bytes_written")]
    print(f"   the file the store wrote: {int(median(bytes_written))} bytes, identical: {all(identical)}")
    loads = [run.get("host_load_average") for run in runs if run.get("host_load_average")]
    if loads:
        print(f"   the host's load average: {', '.join(f'{value:.2f}' for value in loads)}")


def print_engine_rows(runs: list[dict]) -> None:
    """Print the engine's own rows, through the store.

    This is spec 042's write path: the engine's rows reach a store's table, and
    the file that comes out is compared with the published file **byte for
    byte**. Three files are compared, and each answers a different question:

    - the store's own Arrow path writes the published file byte for byte --
      that is the store's claim, and it is the reference;
    - the store's **conversion** under the published file's own schema writes
      it byte for byte too, which says the rows are the same and the
      conversion is exact;
    - the store's own `append_rows` writes the engine's rows under the
      **store's** schema rule, and that is a different file, because the
      engine's vocabulary says which rows are NULL and not whether a column
      may be.
    """
    if not runs:
        return
    print(f"\n=== THE ENGINE'S OWN ROWS, THROUGH THE STORE ({len(runs)} campaigns) ===")
    for name, heading in (
        ("engine_read", "the engine's eager read"),
        ("convert", "the conversion: the vocabulary to Arrow's"),
        ("append", "the store's own API, end to end"),
        ("arrow_path", "the store's own Arrow path"),
        ("plan_read", "a plan's rows: the read"),
        ("plan_append", "a plan's rows: the write"),
    ):
        values = [
            run["measurements_ms"][name]
            for run in runs
            if name in run.get("measurements_ms", {})
        ]
        if values:
            print(f"   {heading:<44} {median(values):10.3f} ms   ({min(values):.3f}-{max(values):.3f})")
    for key, heading in (
        ("bytes_published", "the published file"),
        ("bytes_engine_rows", "the engine's rows, the store's schema rule"),
        ("bytes_converted_rows", "the engine's rows, the file's own schema"),
        ("bytes_arrow_rows", "Arrow's rows"),
        ("bytes_plan_rows", "a plan's rows"),
        ("rows_engine_rows", "the rows of the engine's own rows"),
        ("rows_plan_rows", "the rows of a plan's rows"),
    ):
        values = [run[key] for run in runs if key in run]
        if values:
            print(f"   {heading:<44} {int(median(values)):>12}")
    for key, heading in (
        ("identical_to_published", "the engine's rows are the published file"),
        ("converted_identical_to_published", "the conversion alone is the published file"),
        ("arrow_identical_to_published", "Arrow's rows are the published file"),
    ):
        verdicts = [run.get(key) for run in runs if run.get(key) is not None]
        if verdicts:
            print(f"   {heading:<44} {str(all(verdicts)):>12}")
    differences = [
        run.get("first_difference") for run in runs if run.get("first_difference") is not None
    ]
    if differences:
        print(f"   the engine's rows first differ at             {int(median(differences)):>12}")
    loads = [run.get("host_load_average") for run in runs if run.get("host_load_average")]
    if loads:
        print(f"   the host's load average: {', '.join(f'{value:.2f}' for value in loads)}")


def print_build_set(runs: list[dict]) -> None:
    """Print the join's build side, over a set of files.

    This is spec 042's build side: the same rows, built as one file and as six
    files, in the same campaign. The **pairs** are what matter -- one file
    against six, measured together -- because the host's own movement is the
    same for both sides of the pair.
    """
    if not runs:
        return
    print(f"\n=== THE BUILD SIDE, OVER A SET OF FILES ({len(runs)} campaigns) ===")
    for name, heading in (
        ("build_one_file", "the table's construction, one file"),
        ("build_six_files", "the table's construction, six files"),
        ("open_one_file", "the build side's open, one file"),
        ("open_six_files", "the build side's open, six files"),
    ):
        values = [
            run["measurements_ms"][name]
            for run in runs
            if name in run.get("measurements_ms", {})
        ]
        if values:
            print(f"   {heading:<44} {median(values):10.3f} ms   ({min(values):.3f}-{max(values):.3f})")
    for key, heading in (
        ("rows_one_file", "the join's rows, one file"),
        ("rows_six_files", "the join's rows, six files"),
        ("range_one_file", "the key's range, one file"),
        ("range_six_files", "the key's range, the union"),
        ("estimate_one_file", "the estimate, one file"),
        ("estimate_six_files", "the estimate, six files"),
    ):
        values = [run[key] for run in runs if key in run]
        if values:
            print(f"   {heading:<44} {int(median(values)):>12}")
    verdicts = [run.get("identical") for run in runs if run.get("identical") is not None]
    if verdicts:
        print(f"   {'the same rows and the same answer':<44} {str(all(verdicts)):>12}")
    for key, heading in (
        ("direct_one_file", "the direct table, one file"),
        ("direct_six_files", "the direct table, six files"),
    ):
        values = [run.get(key) for run in runs if run.get(key) is not None]
        if values:
            print(f"   {heading:<44} {str(all(values)):>12}")
    loads = [run.get("host_load_average") for run in runs if run.get("host_load_average")]
    if loads:
        print(f"   the host's load average: {', '.join(f'{value:.2f}' for value in loads)}")


def print_set_rows_before(runs: list[dict]) -> None:
    """Print the set's rows before spec 044: the figures that moved.

    The gather is printed **as it was published** -- the difference between the
    eager rows and the eager read -- because the correction names it, and the
    section has to quote the reports and not a memory of them.
    """
    if not runs:
        return
    print(f"\n=== THE SET'S ROWS, BEFORE SPEC 044 ({len(runs)} campaigns) ===")
    for name, heading in (
        ("eager_read", "the engine's eager read"),
        ("eager_rows", "the eager rows: the read and the gather"),
        ("read_one_file", "the one file's rows"),
        ("read_six_files", "the six files' rows"),
        ("write_one_file", "the write of the one file's rows"),
        ("write_six_files", "the write of the six files' rows"),
    ):
        values = [
            run["measurements_ms"][name]
            for run in runs
            if name in run.get("measurements_ms", {})
        ]
        if values:
            print(f"   {heading:<44} {median(values):10.3f} ms   ({min(values):.3f}-{max(values):.3f})")
    totals = []
    gathers = []
    for run in runs:
        measurements = run.get("measurements_ms", {})
        if "eager_rows" in measurements and "eager_read" in measurements:
            totals.append(measurements["eager_rows"] + measurements["eager_read"])
            gathers.append(measurements["eager_rows"] - measurements["eager_read"])
    if totals:
        print(f"   {'the eager path\'s total':<44} {median(totals):10.3f} ms   ({min(totals):.3f}-{max(totals):.3f})")
    if gathers:
        print(f"   {'the gather, as it was published':<44} {median(gathers):10.3f} ms   ({min(gathers):.3f}-{max(gathers):.3f})")
    for key, heading in (
        ("bytes_rule", "the file, the store's schema rule"),
        ("bytes_own_schema", "the file, the file's own schema"),
    ):
        values = [run[key] for run in runs if key in run]
        if values:
            print(f"   {heading:<44} {int(median(values)):>12}")


def print_set_rows(runs: list[dict]) -> None:
    """Print the set's rows: the round trip through the engine.

    This is spec 043's increment: a store's table read as a set, its rows handed
    out, and the rows written back. The **pairs** are what matter -- one file
    against six, measured together -- because the host's own movement is the
    same for both sides of the pair.

    Spec 044 took the gather away for a plan that removes nothing, so **the
    eager rows are the copy of the columns** and the eager path's total is its
    read and its copy. The **before** set is printed beside it: the gather as it
    was, and the figures the section publishes old-and-new.
    """
    if not runs:
        return
    print(f"\n=== THE SET'S ROWS -- THE ROUND TRIP ({len(runs)} campaigns) ===")
    for name, heading in (
        ("eager_read", "the engine's eager read"),
        ("eager_rows", "the eager rows: the copy of the columns"),
        ("read_one_file", "the one file's rows"),
        ("read_six_files", "the six files' rows"),
        ("write_one_file", "the write of the one file's rows"),
        ("write_six_files", "the write of the six files' rows"),
    ):
        values = [
            run["measurements_ms"][name]
            for run in runs
            if name in run.get("measurements_ms", {})
        ]
        if values:
            print(f"   {heading:<44} {median(values):10.3f} ms   ({min(values):.3f}-{max(values):.3f})")
    totals = []
    for run in runs:
        measurements = run.get("measurements_ms", {})
        if "eager_rows" in measurements and "eager_read" in measurements:
            totals.append(measurements["eager_rows"] + measurements["eager_read"])
    if totals:
        print(f"   {'the eager path\'s total':<44} {median(totals):10.3f} ms   ({min(totals):.3f}-{max(totals):.3f})")
    for key, heading in (
        ("dictionary_columns", "the rows' dictionary columns"),
        ("plain_columns", "the rows' plain columns"),
    ):
        values = [run[key] for run in runs if key in run]
        if values:
            print(f"   {heading:<44} {int(median(values)):>12}")
    for key, heading in (
        ("rows_one_file", "the rows, one file"),
        ("rows_six_files", "the rows, six files"),
        ("bytes_rule", "the file, the store's schema rule"),
        ("bytes_own_schema", "the file, the file's own schema"),
        ("estimate_one_file", "the estimate, one file"),
        ("estimate_six_files", "the estimate, six files"),
    ):
        values = [run[key] for run in runs if key in run]
        if values:
            print(f"   {heading:<44} {int(median(values)):>12}")
    for key, heading in (
        ("identical_arms", "the one file's rows and the six files' rows are the same file"),
        ("identical_to_published", "the round trip is the published file"),
        ("identical_own_schema", "the round trip, the file's own schema, is the published file"),
        ("data_identical_rule", "the store's rule writes the published file's data"),
        ("data_identical_own_schema", "the file's own schema writes the published file's data"),
    ):
        verdicts = [run.get(key) for run in runs if run.get(key) is not None]
        if verdicts:
            print(f"   {heading:<44} {str(all(verdicts)):>12}")
    for key, heading in (
        ("first_difference", "the first differing byte, the store's rule"),
        ("first_difference_own_schema", "the first differing byte, the file's own schema"),
        ("metadata_start", "the published file's metadata starts at"),
    ):
        differences = [run.get(key) for run in runs if run.get(key) is not None]
        if differences:
            print(f"   {heading:<44} {int(median(differences)):>12}")
    loads = [run.get("host_load_average") for run in runs if run.get("host_load_average")]
    if loads:
        print(f"   the host's load average: {', '.join(f'{value:.2f}' for value in loads)}")
    # The yardstick for a pair: the difference between the two arms, in every
    # iteration. The two arms are measured back to back, and the order they are
    # measured in alternates, so the host's movement is in both sides of every
    # one of these numbers.
    for key, heading in (
        ("read", "the difference in the read, in every iteration"),
        ("write", "the difference in the write, in every iteration"),
    ):
        values = [
            difference[key]
            for run in runs
            for difference in run.get("differences_ms", [])
            if key in difference
        ]
        if values:
            rendered = ", ".join(f"{value:+.3f}" for value in values)
            print(f"   {heading:<44} {rendered}")
            print(f"   {'the median of them':<44} {median(values):+10.3f} ms")


def print_excursion_hunt(runs: list[dict]) -> None:
    """Print the hunt: whether the file's state moved at all.

    This is spec 036's answer to the attribution: the excursions the published
    set carries did not recur, and the hunt is the campaigns that say so. Each
    campaign's own spread of the file's own read, and of the machine's own
    movement, is what an **excursion** looks like -- one iteration standing
    away from the others.
    """
    if not runs:
        return
    print(f"\n=== THE HUNT -- {len(runs)} more campaigns of the same instrument ===")
    for name, part in (
        ("the file's own read", JOIN_COST_FILE_FLOOR),
        ("the machine's own movement", JOIN_COST_FLOOR),
    ):
        spreads = []
        for run in runs:
            series = run["series_ms"][part]
            spreads.append(max(series) - min(series))
        print(
            f"   {name:<44} the largest spread within a campaign {max(spreads):8.3f} ms"
            f"   the medians {', '.join(f'{value:.3f}' for value in spreads)}"
        )
    loads = [run.get("host_load_average") for run in runs if run.get("host_load_average")]
    if loads:
        print(f"   the host's load average: {', '.join(f'{value:.2f}' for value in loads)}")


# Spec 049: the batch size's price.
#
# The memory is measured with a counting allocator, in the instrument's own
# probe; the time is measured beside it, in a probe that has no allocator in
# it. The two are never in the same measurement.
BATCH_MEMORY_SHAPES = (
    ("the two predicate columns", "the_join_part_ms"),
)

BATCH_SWEEP_FAMILIES = (
    ("the_join_part_ms", "the join's part"),
    ("the_filter_scan_ms", "the filter-range's scan"),
    ("the_wide_read_ms", "the wide read"),
)


def _pool(runs: list[dict], shape: int, batch: str, key: str) -> list[float]:
    """One point's samples, pooled over every campaign.

    The pooling is the house's rule: every repetition of every campaign is a
    sample, and the published figure is the median of the pooled samples.
    """
    values: list[float] = []
    for run in runs:
        shapes = run.get("the_shapes") or []
        if shape >= len(shapes):
            continue
        for point in shapes[shape].get("the_points", []):
            if str(point.get("batch_rows")) != str(batch):
                continue
            values.extend(float(value) for value in (point.get(key) or []))
    return values


def print_batch_memory(runs: list[dict], note: str = "") -> None:
    """Print the price: the peak a read holds, at each width.

    This is spec 049: the reader's batch size is a knob, and the knob's price
    is measured with a counting allocator, at three widths -- because the bytes
    a batch holds scale with the query's width, and a knob named in rows cannot
    see it.

    **The engine's own estimate is printed beside the measurement**, because an
    estimate that is not measured against is not checked.
    """
    if not runs:
        return

    campaigns = len(runs)
    probe = runs[0].get("probe", "batch-memory")
    suffix = f", {note}" if note else ""
    print(f"\n=== THE BATCH SIZE'S PRICE ({probe} probe, {campaigns} campaigns{suffix}) ===")

    shapes = runs[0].get("the_shapes") or []
    for index, shape in enumerate(shapes):
        name = shape.get("the_shape", "?")
        columns = shape.get("the_columns_materialised", 0)
        refused = bool(shape.get("refused_at_the_default_budget", False))
        print(f"\n-- the shape: {name} ({columns} materialised columns) --")
        if refused:
            print(f"{'refused at the default budget':<46}{'yes':>15}")
        print(
            f"{'the batch size':<16}{'the peak':>15}{'the held':>15}"
            f"{'the batches':>13}{'the estimate':>15}{'the gap':>15}"
        )
        for point in shape.get("the_points", []):
            batch = str(point.get("batch_rows"))
            peaks = _pool(runs, index, batch, "the_peak_bytes")
            helds = _pool(runs, index, batch, "the_held_after_bytes")
            if not peaks:
                continue
            peak = statistics.median(peaks)
            held = statistics.median(helds) if helds else 0
            batches = point.get("the_batches", "?")
            estimate = _scalar(runs, index, batch, "the_estimate_bytes")
            if not estimate:
                # Spec 049's own reports carry the estimate once a shape,
                # because the estimate did not depend on the batch size then.
                estimate = float(shape.get("the_estimate_bytes", 0) or 0)
            print(
                f"{int(batch):>15,}{peak:>15,.0f}{held:>15,.0f}"
                f"{batches:>13,}{estimate:>15,.0f}{peak - estimate:>+15,.0f}"
            )

    rows_agree = {bool(run.get("the_rows_agree")) for run in runs}
    print("\n-- the correctness --")
    print(f"the rows agree in every campaign: {rows_agree == {True}}")
    loads = [float(run.get("the_host_load_average", 0.0)) for run in runs]
    if loads:
        print(f"the host load average: {min(loads):.2f} - {max(loads):.2f}")


def _scalar(runs: list[dict], shape: int, batch: str, key: str) -> float:
    """One point's scalar, over every campaign.

    Some figures are one number per point -- the engine's own estimate is
    computed, not measured, so it does not spread -- and the published figure
    is the median of the campaigns'.
    """
    values: list[float] = []
    for run in runs:
        shapes = run.get("the_shapes") or []
        if shape >= len(shapes):
            continue
        for point in shapes[shape].get("the_points", []):
            if str(point.get("batch_rows")) != str(batch):
                continue
            value = point.get(key)
            if value is not None:
                values.append(float(value))
    return statistics.median(values) if values else 0.0


def print_batch_width(runs: list[dict]) -> None:
    """Print the width statement beside the allocator's measurement.

    This is spec 050: the engine now says what a row costs -- **8** for a
    number, **4** for a dictionary's index, **24** for a plain string's header,
    and the value's own bytes on top -- and the statement is **checked against
    the measurement**, because a statement that is not measured against is a
    guess with a formula.

    **The file's own footer is printed beside both**: it is the file's pages
    and not the working set's, and the difference is the point.
    """
    if not runs:
        return

    campaigns = len(runs)
    probe = runs[0].get("probe", "batch-width")
    print(f"\n=== THE WIDTH STATEMENT ({probe} probe, {campaigns} campaigns) ===")

    shapes = runs[0].get("the_shapes") or []
    for index, shape in enumerate(shapes):
        name = shape.get("the_shape", "?")
        rows = float(shape.get("the_file_rows", 0) or 0)
        statement = float(shape.get("the_statement_bytes", 0) or 0) / rows if rows else 0.0
        footer = float(shape.get("the_file_bytes", 0) or 0) / rows if rows else 0.0

        points = shape.get("the_points") or []
        peaks: list[tuple[int, float]] = []
        for point_index, point in enumerate(points):
            values = [
                float(run["the_shapes"][index]["the_points"][point_index]["the_peak_median"])
                for run in runs
                if index < len(run.get("the_shapes") or [])
                and point_index < len(run["the_shapes"][index].get("the_points") or [])
            ]
            if values:
                peaks.append((int(point.get("batch_rows", 0)), statistics.median(values)))

        measured = 0.0
        if len(peaks) == 2 and peaks[1][0] != peaks[0][0]:
            measured = (peaks[1][1] - peaks[0][1]) / (peaks[1][0] - peaks[0][0])

        print(f"\n-- the shape: {name} --")
        print(f"  the default's batch: {int(shape.get('the_default_s_batch_rows', 0)):,} rows")
        for worker in shape.get("the_default_s_batch_by_workers", []) or []:
            print(
                f"    at {int(worker.get('the_workers', 0)):>2} workers: "
                f"{int(worker.get('the_batch_rows', 0)):>9,} rows"
            )
        print(f"  the columns the reader fetches: {shape.get('the_columns_fetched', 0)}")
        print(f"  the columns the dataset holds:   {shape.get('the_columns_held', 0)}")
        print(f"  the statement:      {statement:>10.2f} bytes a row")
        print(f"  the file's footer:  {footer:>10.2f} bytes a row")
        print(f"  the measurement:    {measured:>10.2f} bytes a row")
        print(f"  the difference:     {measured - statement:>+10.2f} bytes a row")
        if footer:
            print(f"  the footer is {measured / footer:.2f}x low")
        for batch, peak in peaks:
            print(f"    the peak at {batch:>9,} rows: {peak:>15,.0f}")

    rows_agree = {bool(run.get("the_rows_agree")) for run in runs}
    print("\n-- the correctness --")
    print(f"the rows agree in every campaign: {rows_agree == {True}}")
    loads = [float(run.get("the_host_load_average", 0.0)) for run in runs]
    if loads:
        print(f"the host load average: {min(loads):.2f} - {max(loads):.2f}")


def _sweep_samples(runs: list[dict], family: str, batch: str, key: str) -> list[float]:
    """One family's samples at one batch size, pooled over every campaign."""
    values: list[float] = []
    for run in runs:
        sweep = run.get(key) or {}
        entry = sweep.get(str(batch)) or {}
        series = entry.get(family) or {}
        values.extend(float(value) for value in (series.get("samples") or []))
    return values


def _own_batches(run: dict) -> dict[str, str]:
    """The engine's own batch, per family, as the report carries it.

    **The default is a byte budget**, so the batch it gives depends on the width the read fetches -- and the
    reports say which. **The old reports carried one number for every family**, because the old default was
    a row limit and every width got the same batch.
    """
    own = run.get("the_engine_s_own_batch_rows")
    if isinstance(own, dict):
        return {
            "the_join_part_ms": str(own.get("the_join_part")),
            "the_filter_scan_ms": str(own.get("the_filter_scan")),
            "the_wide_read_ms": str(own.get("the_wide_read")),
        }
    if own:
        return {
            "the_join_part_ms": str(own[0]),
            "the_filter_scan_ms": str(own[0]),
            "the_wide_read_ms": str(own[0]),
        }
    return {}


def print_batch_sweep(runs: list[dict]) -> None:
    """Print the time: each shape's own part, at each batch size.

    This is spec 049's other half: the join's part and the filter-range's scan
    are measured **in one session, back to back, with the order alternated**,
    so the join's win is priced against what the other shape pays.

    **The difference is the paired one**: each point against the engine's own
    batch size, in the same iteration.
    """
    if not runs:
        return

    campaigns = len(runs)
    probe = runs[0].get("probe", "batch-sweep")
    sizes = [str(size) for size in runs[0].get("the_batch_sizes", [])]
    own = _own_batches(runs[0])

    print(
        f"\n=== THE BATCH SIZE'S PRICE, IN TIME ({probe} probe, "
        f"{campaigns} campaigns, {len(sizes)} points) ==="
    )

    for family, label in BATCH_SWEEP_FAMILIES:
        print(f"\n-- {label} --")
        print(
            f"{'the batch size':<18}{'the median':>12}"
            f"{'less the engine\'s own':>24}{'the faster':>12}"
        )
        for size in sizes:
            values = _sweep_samples(runs, family, size, "the_sweep_ms")
            if not values:
                continue
            line = f"{int(size):>17,}{statistics.median(values):>12.3f}"
            if size == own.get(family):
                line += f"{'-':>24}{'-':>12}"
            else:
                differences = _sweep_samples(
                    runs, family, size, "the_sweep_difference_ms"
                )
                if differences:
                    faster = sum(1 for value in differences if value < 0)
                    line += (
                        f"{statistics.median(differences):>+24.3f}"
                        f"{faster:>7}/{len(differences):<4}"
                    )
            print(line)

    rows_agree = {bool(run.get("the_rows_agree")) for run in runs}
    print("\n-- the correctness --")
    print(f"the rows agree in every campaign: {rows_agree == {True}}")
    for label, key in (
        ("the join's rows", "the_join_rows"),
        ("the filter-range's rows", "the_filter_rows"),
        ("the wide read's rows", "the_wide_rows"),
    ):
        values = [int(run.get(key, 0)) for run in runs]
        if values:
            print(f"{label}: {min(values)} - {max(values)}")
    loads = [float(run.get("the_host_load_average", 0.0)) for run in runs]
    if loads:
        print(f"the host load average: {min(loads):.2f} - {max(loads):.2f}")


# Spec 051: the default moved to the byte budget, and every figure the
# reader's batch size touches moved with it. The old code was measured **back
# to back** with the new one, on this host, three campaigns each, interleaved,
# and the set the default replaces is the same family with "-spec051-before"
# inserted before "-run".
#
# The section this feeds publishes the headline figure of every family **old
# and new, side by side** -- and this is where both halves come from.
SPEC051_FAMILIES = (
    ("the engine's cold `q6`, lazy", "compare", "engine_cold_q6_lazy", 1),
    ("`DuckDB`'s cold `q6` -- **the yardstick**", "compare", "duckdb_cold_q6", 1),
    ("the whole query, cold", "query-cost", "query_cold_join_orders", 3),
    ("the stage, cold", "sort-cost", "stage_cold", 2),
    ("the join's stage, cold", "join-cost", "stage_cold", 3),
    ("the floor the probe -- **the machine's own measure**", "join-cost", "floor_probe", 3),
    ("the write of the whole file", "write-cost", "write_file", 3),
    ("the commit, all five steps", "store-write", "commit", 3),
    ("Arrow's decode", "materialised", "arrow_decode", 2),
    ("the crate's read, expanded", "dictionary-cost-6", "crate_expanded", 2),
    ("the derived batch", "row-group-cost-6", "derived_batch", 2),
    ("the engine at 1 thread", "scaling", "engine_1_thread", 1),
    ("the engine at 6M rows", "curve", "engine_6m", 1),
)


def _spec051_figure(family: str, runs: list[dict], key: str) -> float | None:
    """One family's headline figure, over every campaign.

    The extraction is the one the guard uses, so that the table the section
    publishes and the guard cannot disagree about a figure.
    """
    def median_of(values: list[float]) -> float | None:
        clean = [value for value in values if value is not None]
        return median(clean) if clean else None

    if family == "compare":
        # **The engine's figure and the yardstick's are two figures**: they are
        # measured by different readers on the same rows, and a median over
        # both of them would be a number about no system.
        def engine(query: str) -> float | None:
            values = [
                entry.get("cold_ms")
                for report in runs
                for entry in report.get("engine_queries", [])
                if entry.get("query") == query and entry.get("mode") == "lazy"
            ]
            return median_of(values)

        def comparator(system: str, query: str) -> float | None:
            values = [
                entry.get("query_ms")
                for report in runs
                for entry in report.get("external", [])
                if (
                    entry.get("query") == query
                    and entry.get("comparator") == system
                    and entry.get("mode") == "cold"
                )
            ]
            return median_of(values)

        if key == "engine_cold_q6_lazy":
            return engine("q6")
        return comparator("duckdb", "q6")
    if family == "query-cost":
        values = [
            report["queries"]["cold"]["join-orders"]["query_ms"] for report in runs
        ]
        return median_of(values)
    if family == "sort-cost":
        return median_of([report["parts_ms"]["stage_cold"] for report in runs])
    if family == "join-cost":
        parts, _ = _join_cost_figures([("runs", runs)])
        if key == "stage_cold":
            return median_of(parts.get("stage_cold", []))
        return median_of(parts.get("floor_probe", []))
    if family == "write-cost":
        return median_of([report["measurements_ms"]["write_file"] for report in runs])
    if family == "store-write":
        return median_of([report["measurements_ms"]["commit"] for report in runs])
    if family == "materialised":
        values = [
            entry["measurements_ms"]["arrow_decode"]
            for report in runs
            for entry in report.get("columns", [])
            if entry.get("name") == "l_quantity"
            and "arrow_decode" in entry.get("measurements_ms", {})
        ]
        return median_of(values)
    if family == "dictionary-cost-6":
        values = [
            report["measurements_ms"]["crate_expanded"] for report in runs
        ]
        return median_of(values)
    if family == "row-group-cost-6":
        # The **derived batch**: the engine's own batch, which the probe adds to
        # the ladder. It is the rung that is not one of the fixed ones -- and
        # under the old row limit it was 125,000, which is one of them.
        fixed = {12_500, 25_000, 31_250, 50_000, 62_500, 65_536, 125_000, 250_000}
        derived = None
        for report in runs:
            for row in report.get("ladder", []):
                if row.get("batch") not in fixed:
                    derived = row.get("batch")
                    break
            if derived is not None:
                break
        if derived is None:
            derived = 125_000
        values = []
        for report in runs:
            for row in report.get("ladder", []):
                if row.get("batch") == derived:
                    values.append(row.get("scan_ms"))
        return median_of(values)
    if family == "scaling":
        values = [
            point.get("engine_ms")
            for report in runs
            for point in report.get("points", [])
            if point.get("threads") == 1
        ]
        return median_of(values)
    if family == "curve":
        values = [
            point.get("engine_ms")
            for report in runs
            for point in report.get("points", [])
            if point.get("rows") == 6_000_000
        ]
        return median_of(values)
    return None


def print_spec051() -> None:
    """The default's own table: the headline figure of every family, old and new."""
    print("\n=== THE DEFAULT MOVED: every family's headline, old and new ===")
    print(f"{'the family':<52}{'the set it replaces':>22}{'this file publishes':>22}")
    for label, family, key, places in SPEC051_FAMILIES:
        before_paths = sorted(
            Path("bench-results").glob(f"{family}-spec051-before-run*.json")
        )
        after_paths = sorted(Path("bench-results").glob(f"{family}-run*.json"))
        if not before_paths or not after_paths:
            print(f"{label:<52}{'not measured':>22}{'not measured':>22}")
            continue
        before = _spec051_figure(
            family, [load(path) for path in before_paths], key
        )
        after = _spec051_figure(family, [load(path) for path in after_paths], key)
        if before is None or after is None:
            print(f"{label:<52}{'no figure':>22}{'no figure':>22}")
            continue
        print(
            f"{label:<52}{f'{before:,.{places}f}':>22}{f'{after:,.{places}f}':>22}"
        )


def main() -> int:
    print_spec051()
    paths = [Path(p) for p in sys.argv[1:]]
    if not paths:
        paths = sorted(Path("bench-results").glob("compare-run*.json"))
    if not paths:
        print("no compare reports given, and none in bench-results/", file=sys.stderr)
        return 1

    runs = [load(p) for p in paths]
    agg = collect(runs)

    print(f"reports: {len(runs)}  ({', '.join(p.name for p in paths)})")
    print("every figure is the median across the reports, with the spread in brackets\n")

    print("=== COLD (ms) ===")
    print(
        f"{'query':<16}{'engine eager':>26}{'engine lazy':>26}"
        + "".join(f"{DISPLAY[comparator]:>26}" for comparator in COMPARATORS)
    )
    for query in QUERIES:
        cells = [
            cell(agg.get(("engine", mode, "cold", query)))
            for mode in ENGINE_MODES
        ] + [
            cell(agg.get((comparator, "-", "cold", query)))
            for comparator in COMPARATORS
        ]
        print(f"{query:<16}" + "".join(cells))

    print("\n=== RESIDENT (ms) ===")
    print(
        f"{'query':<16}{'engine eager':>26}"
        + "".join(f"{DISPLAY[comparator]:>26}" for comparator in COMPARATORS)
    )
    for query in QUERIES:
        cells = [cell(agg.get(("engine", "eager", "resident", query)))] + [
            cell(agg.get((comparator, "-", "resident", query)))
            for comparator in COMPARATORS
        ]
        print(f"{query:<16}" + "".join(cells))

    print("\n=== LOAD (ms) ===")
    for key in (
        ("engine", "eager", "load", "scan"),
        ("engine", "lazy", "load", "scan"),
        ("duckdb", "-", "load", "worst"),
        ("polars", "-", "load", "worst"),
        ("datafusion-peer", "-", "load", "worst"),
    ):
        values = agg.get(key)
        if values:
            print(
                f"{key[0]} {key[1]:<6} {median(values):>10.1f}  "
                f"({min(values):.1f}-{max(values):.1f})"
            )

    print("\n=== RATIOS, from the medians ===")
    q6 = "q6"
    eager_cold = median(agg[("engine", "eager", "cold", q6)])
    lazy_cold = median(agg[("engine", "lazy", "cold", q6)])
    for comparator in COMPARATORS:
        values = agg.get((comparator, "-", "cold", q6))
        if not values:
            print(f"q6 cold  {DISPLAY[comparator]:<12} : not measured in these reports")
            continue
        theirs = median(values)
        print(f"q6 cold  eager / {DISPLAY[comparator]:<12} = {eager_cold / theirs:.2f}x")
        print(f"q6 cold  lazy  / {DISPLAY[comparator]:<12} = {lazy_cold / theirs:.2f}x")

    print("\n=== COLD WINNER, per query ===")
    for query in QUERIES:
        figures = {"lazy": median(agg[("engine", "lazy", "cold", query)])}
        for comparator in COMPARATORS:
            values = agg.get((comparator, "-", "cold", query))
            if values:
                figures[DISPLAY[comparator]] = median(values)
        best = min(figures, key=figures.get)
        detail = "  ".join(f"{name} {value:8.1f}" for name, value in figures.items())
        print(f"{query:<16} {detail}   best: {best}")

    for pattern, label, mode in (
        ("scaling-run[0-9].json", "q6", "cold"),
        ("scaling-rg12-run*.json", "q6", "cold"),
        ("scaling-rg24-run*.json", "q6", "cold"),
        ("scaling-revenue-run*.json", "q6-revenue", "cold"),
        ("scaling-resident-run*.json", "q6", "resident"),
        ("scaling-resident-revenue-run*.json", "q6-revenue", "resident"),
        ("scaling-load-run*.json", "q6", "load"),
    ):
        scaling_paths = sorted(Path("bench-results").glob(pattern))
        if scaling_paths:
            scaling_runs = [load(p) for p in scaling_paths]
            print_scaling(scaling_runs, label, mode)

    # The set the bump replaces, printed beside the set after it, because the
    # bump changed the writer's bytes and the sweeps measure a file.
    for pattern, label, mode in (
        ("scaling-spec041-before-run*.json", "q6", "cold"),
        ("scaling-rg12-spec041-before-run*.json", "q6", "cold"),
        ("scaling-rg24-spec041-before-run*.json", "q6", "cold"),
        ("scaling-revenue-spec041-before-run*.json", "q6-revenue", "cold"),
        ("scaling-resident-spec041-before-run*.json", "q6", "resident"),
        ("scaling-resident-revenue-spec041-before-run*.json", "q6-revenue", "resident"),
        ("scaling-load-spec041-before-run*.json", "q6", "load"),
    ):
        scaling_paths = sorted(Path("bench-results").glob(pattern))
        if scaling_paths:
            scaling_runs = [load(p) for p in scaling_paths]
            print_scaling(scaling_runs, label, mode)

    # The row-group cap's own table: the same sweep on a file with six row
    # groups, on one with twelve, and on one with twenty-four.
    sweep_files = [
        ("A", "scaling-run[0-9].json"),
        ("B", "scaling-rg12-run*.json"),
        ("C", "scaling-rg24-run*.json"),
    ]
    loaded_sweeps = [
        (name, [load(p) for p in sorted(Path("bench-results").glob(pattern))])
        for name, pattern in sweep_files
    ]
    loaded_sweeps = [(name, runs) for name, runs in loaded_sweeps if runs]
    if len(loaded_sweeps) >= 2:
        print_row_groups(loaded_sweeps)

    browser_paths = sorted(Path("bench-results").glob("browser-run*.json"))
    if browser_paths:
        print_browser([load(p) for p in browser_paths])

    expansion_paths = sorted(Path("bench-results").glob("rle-expansion-run*.json"))
    if expansion_paths:
        print_rle_expansion([load(p) for p in expansion_paths])

    materialised_paths = sorted(Path("bench-results").glob("materialised-run*.json"))
    if materialised_paths:
        print_materialised([load(p) for p in materialised_paths])

    # Spec 015's paired probe: the same instrument on the six-row-group file,
    # on the twelve-row-group one and on the twenty-four-row-group one,
    # measured in one session.
    probe_files = [
        ("A", "row-groups-6-run*.json"),
        ("B", "row-groups-12-run*.json"),
        ("C", "row-groups-24-run*.json"),
    ]
    loaded_probes = [
        (name, [load(p) for p in sorted(Path("bench-results").glob(pattern))])
        for name, pattern in probe_files
    ]
    loaded_probes = [(name, runs) for name, runs in loaded_probes if runs]
    if loaded_probes:
        print_row_group_probe(loaded_probes)

    curve_paths = sorted(Path("bench-results").glob("curve-run*.json"))
    if curve_paths:
        print_curve([load(p) for p in curve_paths])

    batch_size_paths = sorted(Path("bench-results").glob("batch-size-run*.json"))
    if batch_size_paths:
        print_batch_size([load(p) for p in batch_size_paths])

    # Spec 016's cost probe: the same instrument on the six-row-group file, on
    # the twelve-row-group one and on the twenty-four-row-group one, in one
    # session, with the ladder of reader batch sizes.
    cost_files = [
        ("A", "row-group-cost-6-run*.json"),
        ("B", "row-group-cost-12-run*.json"),
        ("C", "row-group-cost-24-run*.json"),
    ]
    loaded_costs = [
        (name, [load(p) for p in sorted(Path("bench-results").glob(pattern))])
        for name, pattern in cost_files
    ]
    loaded_costs = [(name, runs) for name, runs in loaded_costs if runs]
    if loaded_costs:
        print_row_group_cost(loaded_costs)

    # Spec 022's dictionary probe: the same read in two shapes, the new code
    # and the old one, measured back to back on the same host.
    dictionary_files = [
        ("A", "dictionary-cost-6-run*.json"),
        ("B", "dictionary-cost-12-run*.json"),
        ("C", "dictionary-cost-24-run*.json"),
    ]
    loaded_dictionary = [
        (name, [load(p) for p in sorted(Path("bench-results").glob(pattern))])
        for name, pattern in dictionary_files
    ]
    loaded_dictionary = [(name, runs) for name, runs in loaded_dictionary if runs]
    dictionary_before = [
        (name, [load(p) for p in sorted(Path("bench-results").glob(pattern))])
        for name, pattern in [
            ("A", "dictionary-cost-before-6-run*.json"),
            ("B", "dictionary-cost-before-12-run*.json"),
            ("C", "dictionary-cost-before-24-run*.json"),
        ]
    ]
    dictionary_before = [(name, runs) for name, runs in dictionary_before if runs]
    if loaded_dictionary:
        print_dictionary_cost(loaded_dictionary, dictionary_before)

    # Spec 023's scan probe: the join's scan, taken apart.
    scan_paths = sorted(Path("bench-results").glob("scan-cost-run*.json"))
    if scan_paths:
        print_scan_cost([("A", [load(p) for p in scan_paths])])

    # Spec 024's query probe: the whole query, the plan output's stage
    # table, and the time between them -- measured against the code the
    # increment replaced, back to back on the same host.
    query_after = sorted(Path("bench-results").glob("query-cost-run*.json"))
    query_before = sorted(Path("bench-results").glob("query-cost-before-run*.json"))
    if query_after:
        print_query_cost(
            [("A", [load(p) for p in query_after])],
            [("A", [load(p) for p in query_before])] if query_before else [],
        )

    # Spec 025's sort probe: the sort's stage, taken apart -- measured against
    # the code the increment replaces, back to back on the same host.
    sort_after = sorted(Path("bench-results").glob("sort-cost-run*.json"))
    sort_before = sorted(Path("bench-results").glob("sort-cost-before-run*.json"))
    if sort_after:
        print_sort_cost(
            [("A", [load(p) for p in sort_after])],
            [("A", [load(p) for p in sort_before])] if sort_before else [],
        )

    # Spec 026's join probe: the join's stage, taken apart -- measured against
    # the code the increment replaces, back to back on the same host. Spec 028
    # measured it again, with two more parts in the instrument, and the two
    # sets spec 026 published are preserved under their own names.
    join_sets = [
        # Spec 041 changed the dependency the read goes through, so the set the
        # file publishes is the bumped code and the set it replaces is the one
        # the bump took out. The two are printed side by side, because the
        # difference between them is the bump's and the host's.
        ("the code this increment ships", "join-cost-run*.json"),
        ("the code this increment replaces", "join-cost-spec041-before-run*.json"),
        ("the code spec 034 shipped", "join-cost-spec034-run*.json"),
        ("the code spec 034 replaces", "join-cost-spec034-before-run*.json"),
        ("the code the last increment shipped", "join-cost-spec030-run*.json"),
        ("the code spec 030 replaced", "join-cost-spec030-before-run*.json"),
        ("spec 029's set, as spec 029 measured it", "join-cost-spec029-run*.json"),
        ("spec 029's before set", "join-cost-spec029-before-run*.json"),
        ("spec 028's set, as spec 028 measured it", "join-cost-spec028-run*.json"),
        ("spec 028's before set", "join-cost-spec028-before-run*.json"),
        ("spec 026's set, as spec 026 measured it", "join-cost-spec026-run*.json"),
        ("the code before spec 026", "join-cost-spec025-run*.json"),
    ]
    join_sets = [
        (label, [("A", [load(p) for p in sorted(Path("bench-results").glob(pattern))])])
        for label, pattern in join_sets
        if sorted(Path("bench-results").glob(pattern))
    ]
    if join_sets:
        print_join_cost(join_sets)

    # Spec 046's join-parts probe: the join-orders query, on both sides, taken
    # apart -- the engine's own parts, `DuckDB`'s own parts, and `DuckDB` a
    # third time with its join filter pushdown disabled.
    join_parts_paths = sorted(Path("bench-results").glob("join-parts-run*.json"))
    if join_parts_paths:
        print_join_parts([load(p) for p in join_parts_paths])

    # Spec 047's key-filter probe: the build side's keys as a filter -- the
    # ladder's rungs, and the paired verdicts.
    key_filter_paths = sorted(Path("bench-results").glob("key-filter-run*.json"))
    if key_filter_paths:
        print_key_filter([load(p) for p in key_filter_paths])

    # Spec 048's probe-paths: the engine's own probe, in the two paths it runs
    # in -- the query's own shape, and the key alone.
    probe_paths = sorted(Path("bench-results").glob("probe-paths-run*.json"))
    if probe_paths:
        print_probe_paths([load(p) for p in probe_paths], "the query's own shape")
    probe_paths_key_only = sorted(Path("bench-results").glob("probe-paths-key-only-run*.json"))
    if probe_paths_key_only:
        print_probe_paths([load(p) for p in probe_paths_key_only], "the key alone")

    # Spec 048's other half: the query's own parts, at each of the reader's
    # batch sizes -- the engine's own included.
    batch_sweep = []
    for label, pattern in (
        ("62,500", "join-parts-batch-62500-spec048-run*.json"),
        ("125,000 (the engine's own)", "join-parts-spec048-run*.json"),
        ("250,000", "join-parts-batch-250000-spec048-run*.json"),
        ("1,000,000", "join-parts-batch-1000000-spec048-run*.json"),
    ):
        paths = sorted(Path("bench-results").glob(pattern))
        if paths:
            batch_sweep.append((label, [load(p) for p in paths]))
    if batch_sweep:
        print_join_parts_batch_sweep(batch_sweep)

    # Spec 049: the batch size's price -- the memory, measured with a counting
    # allocator, and the time, measured beside it.
    batch_memory = sorted(Path("bench-results").glob("batch-memory-run*.json"))
    if batch_memory:
        print_batch_memory([load(p) for p in batch_memory])

    # Spec 049's own memory reports, kept: the section that published the
    # price is a record of that measurement, and its figures are derived from
    # these reports.
    batch_memory_spec049 = sorted(
        Path("bench-results").glob("batch-memory-spec049-run*.json")
    )
    if batch_memory_spec049:
        print_batch_memory(
            [load(p) for p in batch_memory_spec049], note="spec 049's own measurement"
        )
    batch_sweep_reports = sorted(Path("bench-results").glob("batch-sweep-run*.json"))
    if batch_sweep_reports:
        print_batch_sweep([load(p) for p in batch_sweep_reports])

    # Spec 050: the width statement, checked against the allocator's
    # measurement, with the file's own footer beside both.
    batch_width = sorted(Path("bench-results").glob("batch-width-run*.json"))
    if batch_width:
        print_batch_width([load(p) for p in batch_width])

    # Spec 035's ladder: the drift's own parts, over the set the file publishes,
    # the set spec 034 measured beside it, and the set spec 036 measured.
    ladder_sets = [
        (label, [load(p) for p in sorted(Path("bench-results").glob(pattern))])
        for label, pattern in (
            ("the set the file publishes now", "join-cost-run*.json"),
            ("the set spec 041 replaces", "join-cost-spec041-before-run*.json"),
            ("the set spec 034 measured", "join-cost-spec034-run*.json"),
            ("the set spec 036 measured", "join-cost-spec036-run*.json"),
        )
        if sorted(Path("bench-results").glob(pattern))
    ]
    if ladder_sets:
        print_drift_ladder(ladder_sets)

    # Spec 036: the file-state floor -- the two floors, the yardstick that sees
    # the file's state, and the code before this increment, measured back to back
    # on this host.
    floor_sets = [
        (label, [load(p) for p in sorted(Path("bench-results").glob(pattern))])
        for label, pattern in (
            ("the set spec 036 measured", "join-cost-spec036-run*.json"),
            ("the set the code before it measured", "join-cost-spec036-before-run*.json"),
        )
        if sorted(Path("bench-results").glob(pattern))
    ]
    if floor_sets:
        print_file_state_floor(floor_sets)

    # Spec 053: the context's two sides, in one account -- the ladder and the
    # verdict on the set this increment measured, and the set the pairing
    # replaces beside it.
    context_sets = [
        (label, [load(p) for p in sorted(Path("bench-results").glob(pattern))])
        for label, pattern in (
            ("the set spec 053 measured", "join-cost-spec053-run*.json"),
            ("the set the pairing replaces", "join-cost-run*.json"),
        )
        if sorted(Path("bench-results").glob(pattern))
    ]
    if context_sets:
        print_context_accounts(context_sets)

    # The hunt: nine more campaigns of the same instrument, published because
    # they say whether the file's state moved at all on this host.
    hunt = [load(p) for p in sorted(Path("bench-results").glob("join-cost-spec036-hunt-run*.json"))]
    print_excursion_hunt(hunt)

    # Spec 037: the write side -- the write, the commit, the bytes and the
    # read-back, in the two batch sizes the same rows were written in.
    write_sets = [
        (label, [load(p) for p in sorted(Path("bench-results").glob(pattern))])
        for label, pattern in (
            ("the batch the published file was written in", "write-cost-run*.json"),
            ("the engine's own batch size", "write-cost-engine-batch-run*.json"),
        )
        if sorted(Path("bench-results").glob(pattern))
    ]
    if write_sets:
        print_write_side(write_sets)

    # Spec 038: the store's own write -- the file it wrote, compared with the
    # published file byte for byte.
    store_runs = [load(p) for p in sorted(Path("bench-results").glob("store-write-run*.json"))]
    print_store_write(store_runs)

    # Spec 039: the file boundary's cost -- the same rows, read as one file
    # and as six.
    many_files_runs = [load(p) for p in sorted(Path("bench-results").glob("many-files-run*.json"))]
    print_many_files(many_files_runs)

    # Spec 040: the source over a set of files -- the same rows, read as one
    # file, as six files one by one, and as six files read as one table.
    file_set_runs = [load(p) for p in sorted(Path("bench-results").glob("file-set-run*.json"))]
    print_file_set(file_set_runs)

    # Spec 042: the set on both sides -- the engine's own rows through the
    # store, and the join's build side over a set of files.
    engine_rows_runs = [
        load(p) for p in sorted(Path("bench-results").glob("engine-rows-run*.json"))
    ]
    print_engine_rows(engine_rows_runs)

    build_set_runs = [load(p) for p in sorted(Path("bench-results").glob("build-set-run*.json"))]
    print_build_set(build_set_runs)

    # Spec 043: the set's rows -- a store's table read as a set, its rows
    # handed out, and the rows written back.
    set_rows_runs = [load(p) for p in sorted(Path("bench-results").glob("set-rows-run*.json"))]
    set_rows_before = [
        load(p) for p in sorted(Path("bench-results").glob("set-rows-spec044-before-run*.json"))
    ]
    print_set_rows(set_rows_runs)
    print_set_rows_before(set_rows_before)

    # Spec 041: the reader's four gigabytes -- a real file with one hostile
    # page header, and the largest single allocation the reader asked for.
    # The set before the bump is printed beside the set after it, because the
    # bump is what closes the hole.
    hostile_runs = [load(p) for p in sorted(Path("bench-results").glob("hostile-metadata-run*.json"))]
    hostile_before = [
        load(p) for p in sorted(Path("bench-results").glob("hostile-metadata-spec041-before-run*.json"))
    ]
    print_hostile(hostile_runs, hostile_before)

    # The x86-64 kernel A/B, from the GitHub-hosted runner.
    avx2_paths = sorted(Path("bench-results").glob("avx2-run*.json"))
    if avx2_paths:
        print_avx2([load(p) for p in avx2_paths])

    # §1's table: the ARM64 kernel A/B, as the median of its campaigns.
    kernel_paths = sorted(Path("bench-results").glob("kernel-run*.json"))
    if kernel_paths:
        print_kernel([load(p) for p in kernel_paths])

    # Spec 052: the reader that owns the decode -- ADR-0018's shape, measured
    # again on the code that is here now.
    own_decode_runs = [
        load(p) for p in sorted(Path("bench-results").glob("own-decode-run*.json"))
    ]
    print_own_decode([("own-decode", own_decode_runs)])

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
