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
            push(threads, "engine", point.get("engine_ms"))
            push(threads, "engine_scan", point.get("engine_scan_ms"))
            push(threads, "engine_operators", point.get("engine_operators_ms"))
            push(threads, "duckdb", point.get("duckdb_ms"))
            push(threads, "polars", point.get("polars_ms"))
            push(threads, "datafusion", point.get("datafusion_ms"))

    return out


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
        f"{'threads':>8}{'workers':>9}{'engine':>26}{'engine scan':>26}{'engine ops':>26}"
        f"{'duckdb':>26}{'polars':>26}{'datafusion':>26}"
    )
    for threads in sorted(sweep):
        figures = sweep[threads]
        workers = figures.get("workers", [0])
        row = (
            f"{threads:>8}"
            f"{median(workers):>9.0f}"
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
    rows_of: dict[int, int] = {}
    for run in runs:
        for point in run.get("points", []):
            rows_of[point["rows"]] = point.get("scale_factor") or 0

    budget = runs[0].get("memory_budget_bytes")
    print(f"\n=== THE SIZE CURVE, {len(runs)} campaigns ===")
    if budget:
        print(f"the engine's memory budget: {budget // (1024 * 1024)} MiB")
    header = f"{'rows':>10}{'sf':>4}{'engine':>12}{'scan':>10}{'ops':>8}"
    for name in ("duckdb", "polars", "datafusion"):
        header += f"{name:>12}"
    header += f"{'ms/Mrow':>10}"
    print(header)
    print("-" * len(header))
    for rows in sorted(engine):
        line = f"{rows:>10}{rows_of.get(rows, 0):>4}"
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


def main() -> int:
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

    # The x86-64 kernel A/B, from the GitHub-hosted runner.
    avx2_paths = sorted(Path("bench-results").glob("avx2-run*.json"))
    if avx2_paths:
        print_avx2([load(p) for p in avx2_paths])

    # §1's table: the ARM64 kernel A/B, as the median of its campaigns.
    kernel_paths = sorted(Path("bench-results").glob("kernel-run*.json"))
    if kernel_paths:
        print_kernel([load(p) for p in kernel_paths])

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
