#!/usr/bin/env python3
"""Run one SQL query against a Parquet file with Polars, and print the result.

This script exists because Polars ships as a library, not as a command line
client that can open a file. The comparison in `PERF.md` needs Polars to be a
*separate process* reading the *same bytes* the engine reads, and it needs the
version to be recorded, so the driver is checked into the repository rather
than being an ad-hoc line typed at a terminal.

The query is the same SQL the DuckDB driver runs. Polars executes it through
`pl.SQLContext`.

Two modes, and the difference matters:

  --resident  The file is read eagerly into a DataFrame first, and the query
              runs against that in-memory frame. The two times are reported
              separately: how long the file took to become a frame, and how
              long the query took once it was there.

  (default)   The file is read lazily through `pl.scan_parquet`, and the
              reported query time covers the scan as well. This is Polars'
              idiomatic way to query a Parquet file, and it is the cold
              number: one query, one read.

The resident mode exists so that the comparison against the engine is like for
like. The engine loads its dataset once and then answers queries against the
resident column chunks, so comparing Polars' cold scan against the engine's
warm query would be measuring two different things.

Output format: two `KEY=value` lines followed by the result, so the driver can
read the timings without having to parse a table.

Usage:
    polars-run.py <sql> <parquet-path> [--resident]
"""

import sys
import time

import polars as pl


def main() -> int:
    args = [a for a in sys.argv[1:] if a != "--resident"]
    resident = "--resident" in sys.argv[1:]

    if len(args) < 2:
        print("usage: polars-run.py <sql> <parquet-path> [--resident]", file=sys.stderr)
        return 2

    sql = args[0]
    path = args[1]
    # The table name the SQL refers to. It must match the name the Rust side
    # substitutes into the query.
    table = args[2] if len(args) > 2 else "lineitem"

    load_ms = 0.0
    if resident:
        # Eager load: the file becomes a frame in memory, and the time it
        # takes is reported separately from the query.
        started = time.perf_counter()
        frame = pl.read_parquet(path)
        load_ms = (time.perf_counter() - started) * 1000.0
    else:
        # Lazy scan: the file is read when the query runs, so the query time
        # below covers the scan too.
        frame = pl.scan_parquet(path)

    context = pl.SQLContext(frames={table: frame})

    started = time.perf_counter()
    result = context.execute(sql, eager=True)
    query_ms = (time.perf_counter() - started) * 1000.0

    # The single scalar the query produced, when it produced one. The Rust
    # driver cross-checks this against the engine's own answer, so the two
    # sides cannot silently be computing different things.
    value = ""
    if result.height == 1 and result.width == 1:
        value = str(result.item(0, 0))

    print(f"LOAD_MS={load_ms:.3f}")
    print(f"QUERY_MS={query_ms:.3f}")
    print(f"VALUE={value}")
    print(result)
    return 0


if __name__ == "__main__":
    sys.exit(main())
