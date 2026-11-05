#!/usr/bin/env python3
"""Refresh the representative report copies in bench-results.

Each family has a copy beside its three campaigns: the run whose figures sit
**nearest the medians**, so a reader can open one file and see a representative
run. This script recomputes that choice and rewrites the copy, and prints the
README rows that have to change with it.

The rule: the sum, over every figure the three runs share, of the run's
distance from that figure's median, divided by the median -- so a figure's
weight is its own relative distance, and a run with a tight spread wins.

    python3 scripts/nearest-run.py           # check
    python3 scripts/nearest-run.py --write   # rewrite the copies
"""

from __future__ import annotations

import json
import shutil
import statistics
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RESULTS = ROOT / "bench-results"

# (the copy, the family whose campaigns it stands for)
COPIES = (
    ("compare.json", "compare"),
    ("scaling.json", "scaling"),
    ("scaling-revenue.json", "scaling-revenue"),
    ("scaling-rg12.json", "scaling-rg12"),
    ("scaling-rg24.json", "scaling-rg24"),
    ("row-groups-6.json", "row-groups-6"),
    ("row-groups-12.json", "row-groups-12"),
    ("row-groups-24.json", "row-groups-24"),
    ("kernel.json", "kernel"),
)


def leaves(value: object, prefix: str = "", out: dict[str, object] | None = None) -> dict[str, object]:
    """Every leaf of a report, by its path."""
    if out is None:
        out = {}
    if isinstance(value, dict):
        for key, item in value.items():
            leaves(item, f"{prefix}.{key}", out)
    elif isinstance(value, list):
        for index, item in enumerate(value):
            leaves(item, f"{prefix}[{index}]", out)
    else:
        out[prefix] = value
    return out


def distance(runs: list[dict[str, object]]) -> list[float]:
    """How far each run sits from the medians, figure by figure."""
    shared = set(runs[0])
    for run in runs[1:]:
        shared &= set(run)
    scores = []
    for run in runs:
        total = 0.0
        for key in shared:
            values = [run_[key] for run_ in runs]
            if not all(isinstance(value, (int, float)) and not isinstance(value, bool) for value in values):
                continue
            median = statistics.median(float(value) for value in values)
            total += abs(float(run[key]) - median) / max(abs(median), 1e-9)
        scores.append(total)
    return scores


def main() -> int:
    write = "--write" in sys.argv
    for copy, family in COPIES:
        campaigns = sorted(RESULTS.glob(f"{family}-run*.json"))
        if not campaigns:
            print(f"{copy}: no campaigns for {family}")
            continue
        runs = [leaves(json.loads(path.read_text())) for path in campaigns]
        scores = distance(runs)
        nearest = min(range(len(scores)), key=lambda index: scores[index])
        source = campaigns[nearest]
        target = RESULTS / copy
        stale = (not target.exists()) or target.read_text() != source.read_text()
        print(
            f"{copy}: {source.name} is nearest "
            f"({', '.join(f'{p.name}: {s:.4f}' for p, s in zip(campaigns, scores))})"
            f"{' — STALE' if stale else ' — current'}"
        )
        if write and stale:
            shutil.copyfile(source, target)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
