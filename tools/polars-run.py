#!/usr/bin/env python3
"""Run one SQL query against a Parquet file with Polars, and print the result.

This script exists because Polars ships as a library, not as a command line
client that can open a file. The comparison in `PERF.md` needs Polars to be a
*separate process* reading the *same bytes* the engine reads, and it needs the
version to be recorded, so the driver is checked into the repository rather
than being an ad-hoc line typed at a terminal.

The query is the same SQL the DuckDB driver runs. Polars executes it through
`pl.SQLContext`.

A query that joins two relations needs two files, so the driver can register a
second one beside the first: `--build` names that file and `--build-table` the
name it is registered under. The SQL then refers to the two relations by those
names, exactly as the harness substitutes them. Without `--build` the driver
reads a single file, as it always did.

Two modes, and the difference matters:

  --resident  The file is read eagerly into a DataFrame first, and the query
              runs against that in-memory frame. The two times are reported
              separately: how long the file took to become a frame, and how
              long the query took once it was there. With `--build` both files
              are read eagerly and the reported load time covers both reads.

  (default)   The file is read lazily through `pl.scan_parquet`, and the
              reported query time covers the scan as well. This is Polars'
              idiomatic way to query a Parquet file, and it is the cold
              number: one query, one read. With `--build` both files are
              scanned lazily, so the query time covers both scans.

The resident mode exists so that the comparison against the engine is like for
like. The engine loads its dataset once and then answers queries against the
resident column chunks, so comparing Polars' cold scan against the engine's
warm query would be measuring two different things.

Output format: two `KEY=value` lines followed by the result, so the driver can
read the timings without having to parse a table.

Usage:
    polars-run.py <sql> <parquet-path> [table-name] [--resident]
                  [--build <path>] [--build-table <name>]
"""

import sys
import time

import polars as pl


# The name the second table is registered under when the caller does not say
# which name to use.
DEFAULT_BUILD_TABLE = "orders"


def usage() -> str:
    """The one line that says how the script is called."""
    return (
        "usage: polars-run.py <sql> <parquet-path> [table-name] [--resident] "
        "[--build <path>] [--build-table <name>]"
    )


def parse_args(argv):
    """Splits the command line into the positional arguments and the flags.

    The positional arguments are the SQL, the Parquet file, and the name the
    SQL refers to that file by. The flags are the mode, the second Parquet
    file, and the name that file is registered under.

    A flag's value is either the next argument or the rest of the flag itself
    (`--build=path`), because the harness may use either spelling.
    """
    positional = []
    resident = False
    build_path = None
    build_table = DEFAULT_BUILD_TABLE

    index = 0
    while index < len(argv):
        argument = argv[index]
        index += 1

        if argument == "--resident":
            resident = True
            continue

        flag, separator, inline = argument.partition("=")
        if flag not in ("--build", "--build-table"):
            # Everything that is not a flag is positional, as it always was.
            positional.append(argument)
            continue

        if separator == "=":
            value = inline
        else:
            if index >= len(argv):
                raise ValueError(f"{flag} requires a value")
            value = argv[index]
            index += 1

        if flag == "--build":
            build_path = value
        else:
            build_table = value

    return positional, resident, build_path, build_table


def main() -> int:
    try:
        args, resident, build_path, build_table = parse_args(sys.argv[1:])
    except ValueError as error:
        print(f"polars-run.py: {error}", file=sys.stderr)
        print(usage(), file=sys.stderr)
        return 2

    if len(args) < 2:
        print(usage(), file=sys.stderr)
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

    frames = {table: frame}

    if build_path is not None:
        # The second relation of a join. It is read the same way as the first
        # one, so the reported times cover both files and not just one.
        if resident:
            started = time.perf_counter()
            build_frame = pl.read_parquet(build_path)
            load_ms += (time.perf_counter() - started) * 1000.0
        else:
            build_frame = pl.scan_parquet(build_path)
        frames[build_table] = build_frame

    context = pl.SQLContext(frames=frames)

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
