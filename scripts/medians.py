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
COMPARATORS = ["duckdb", "polars"]
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
    print(f"{'query':<16}{'engine eager':>26}{'engine lazy':>26}{'duckdb':>26}{'polars':>26}")
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
    print(f"{'query':<16}{'engine eager':>26}{'duckdb':>26}{'polars':>26}")
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
    duckdb_cold = median(agg[("duckdb", "-", "cold", q6)])
    polars_cold = median(agg[("polars", "-", "cold", q6)])
    print(f"q6 cold  eager / lazy     = {eager_cold / lazy_cold:.2f}x")
    print(f"q6 cold  eager / duckdb   = {eager_cold / duckdb_cold:.2f}x")
    print(f"q6 cold  lazy  / duckdb   = {lazy_cold / duckdb_cold:.2f}x")
    print(f"q6 cold  lazy  / polars   = {lazy_cold / polars_cold:.2f}x")
    print(f"q6 cold  eager / polars   = {eager_cold / polars_cold:.2f}x")

    print("\n=== COLD WINNER, per query ===")
    for query in QUERIES:
        figures = {
            "lazy": median(agg[("engine", "lazy", "cold", query)]),
            "duckdb": median(agg[("duckdb", "-", "cold", query)]),
            "polars": median(agg[("polars", "-", "cold", query)]),
        }
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

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
