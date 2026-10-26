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

QUERIES = ["scan", "filter-numeric", "filter-string", "filter-range", "q6", "q6-revenue"]
COMPARATORS = ["duckdb", "polars", "datafusion-peer"]
DISPLAY = {"duckdb": "duckdb", "polars": "polars", "datafusion-peer": "datafusion"}
ENGINE_MODES = ["eager", "lazy"]


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

    return out


def print_scaling(runs: list[dict], label: str = "q6", mode: str = "cold") -> None:
    """Print the thread-count sweep's medians."""
    if not runs:
        return
    sweep = collect_scaling(runs)
    if not sweep:
        return

    print(f"\n=== THREAD-COUNT SWEEP, {label} ({mode}, {len(runs)} runs) ===")
    print(
        f"{'threads':>8}{'workers':>9}{'engine':>26}{'engine scan':>26}{'engine ops':>26}"
        f"{'duckdb':>26}{'polars':>26}"
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
        ("scaling-run*.json", "q6", "cold"),
        ("scaling-revenue-run*.json", "q6-revenue", "cold"),
        ("scaling-resident-run*.json", "q6", "resident"),
        ("scaling-resident-revenue-run*.json", "q6-revenue", "resident"),
        ("scaling-load-run*.json", "q6", "load"),
    ):
        scaling_paths = sorted(Path("bench-results").glob(pattern))
        if scaling_paths:
            scaling_runs = [load(p) for p in scaling_paths]
            print_scaling(scaling_runs, label, mode)

    browser_paths = sorted(Path("bench-results").glob("browser-run*.json"))
    if browser_paths:
        print_browser([load(p) for p in browser_paths])

    expansion_paths = sorted(Path("bench-results").glob("rle-expansion-run*.json"))
    if expansion_paths:
        print_rle_expansion([load(p) for p in expansion_paths])

    materialised_paths = sorted(Path("bench-results").glob("materialised-run*.json"))
    if materialised_paths:
        print_materialised([load(p) for p in materialised_paths])

    curve_paths = sorted(Path("bench-results").glob("curve-run*.json"))
    if curve_paths:
        print_curve([load(p) for p in curve_paths])

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
