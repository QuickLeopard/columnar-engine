# datafusion-peer

A standalone Rust binary that runs a SQL query against a Parquet file with
[DataFusion](https://datafusion.apache.org/) and reports how long it took.

It is the DataFusion arm of the `columnar-engine` benchmark harness. The
harness measures the engine under test against DuckDB, Polars and DataFusion on
the same file, with the same SQL, and with the thread count pinned, so that the
numbers are comparable. DataFusion is a third incumbent in that comparison, not
a part of the engine under test.

## Why it lives outside the workspace

This directory is its own cargo workspace. Its `Cargo.toml` carries an empty
`[workspace]` table, which makes cargo treat `tools/datafusion-peer/` as a
workspace root instead of walking up the directory tree into the
`columnar-engine` workspace.

That is deliberate. If DataFusion were a workspace member, adding a third
incumbent to the comparison would move the published dependency graph of
`columnar-engine` -- a graph the repository publishes performance numbers
against. Keeping the peer outside means the engine's own dependencies, and
therefore its own measurements, do not move.

The two crates share nothing but the Parquet file and the SQL text.

## Building

Rust is not on `PATH` by default in this repository's environment:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
cd tools/datafusion-peer
cargo build --release
```

The binary lands at `tools/datafusion-peer/target/release/datafusion-peer`.

`Cargo.lock` is committed and pins the exact dependency versions, so the
measured numbers are reproducible.

## Command line

```
datafusion-peer --sql <SQL> --path <FILE> [--threads <N>] [--resident]
datafusion-peer --version
```

| Flag | Meaning |
| --- | --- |
| `--sql <SQL>` | The SQL to run. It refers to a table named `lineitem`. |
| `--path <FILE>` | The Parquet file to query. |
| `--threads <N>` | Thread count to pin the engine to. Default `1`. |
| `--resident` | Load the file into memory first and time the load separately. Without it the Parquet scan happens inside the query. |
| `--version` | Print the version banner and exit. |
| `--help` | Print the usage and exit. |

Both `--flag value` and `--flag=value` spellings are accepted.

The thread count is pinned twice, on purpose: the tokio runtime gets exactly
`--threads` worker threads, and DataFusion's `target_partitions` is set to the
same number so the engine's own parallelism matches the runtime it is running
on.

## Marker protocol

The harness runs this program as a separate process and reads machine-readable
markers from its standard output. The markers come first, in this order:

```
LOAD_MS=1033.600
QUERY_MS=0.200
VALUE=123456.789
```

| Marker | Meaning |
| --- | --- |
| `LOAD_MS` | Time to get the file into memory, in milliseconds. In resident mode this is the eager load; in cold mode there is no separate load phase, so it is `0.000`. |
| `QUERY_MS` | Time the SQL took, in milliseconds. In cold mode this includes the Parquet scan; in resident mode the data is already in memory. |
| `VALUE` | The single numeric scalar the query produced. Empty when the result is not a single numeric scalar. |

The result table is printed after the markers for human inspection. The harness
ignores it.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Success. |
| `1` | Runtime failure: unreadable file, bad SQL, query error. The message goes to stderr. |
| `2` | Malformed command line. The message and the usage go to stderr. |

Errors never panic; they are reported on stderr with a non-zero exit code.
