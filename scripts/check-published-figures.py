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
