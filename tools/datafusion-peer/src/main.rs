//! `datafusion-peer` -- the DataFusion arm of the columnar-engine benchmark.
//!
//! The harness measures the engine under test against DuckDB, Polars and
//! DataFusion on the same Parquet file, with the same SQL, and with the thread
//! count pinned. DataFusion is deliberately *not* a dependency of the engine
//! under test: this binary lives in its own cargo workspace (see the
//! `[workspace]` table in `Cargo.toml`) so that adding a third incumbent to the
//! comparison cannot move the published dependency graph of `columnar-engine`.
//!
//! The harness runs this program as a separate process and reads
//! machine-readable markers from stdout:
//!
//! ```text
//! LOAD_MS=1033.600
//! QUERY_MS=0.200
//! VALUE=123456.789
//! ```
//!
//! `LOAD_MS` is the time spent getting the file into memory. In resident mode
//! that is the eager load; in cold mode there is no separate load phase, so it
//! is reported as `0.000` and the Parquet scan happens inside `QUERY_MS`.
//! `VALUE` is the single numeric scalar the query produced, or empty when the
//! result is not a single numeric scalar.
//!
//! A query that joins two relations needs two files. `--build-path` names the
//! second one and `--build-table` the name it is registered under; it is read
//! the same way as the first one, so in resident mode `LOAD_MS` covers both
//! files and in cold mode the query covers both scans.
//!
//! The result table follows the markers for human inspection; the harness
//! ignores it.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
// The prose names three other engines on purpose, and backticking them would
// read as code rather than as product names.
#![allow(clippy::doc_markdown)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use arrow::datatypes::DataType;
use arrow::record_batch::RecordBatch;
use arrow::util::display::{ArrayFormatter, FormatOptions};
use datafusion::datasource::MemTable;
use datafusion::execution::runtime_env::RuntimeEnvBuilder;
use datafusion::prelude::{ParquetReadOptions, SessionConfig, SessionContext};

/// The table name the benchmark SQL refers to.
const TABLE_NAME: &str = "lineitem";

/// The name the second table is registered under when the caller does not say
/// which name to use.
const DEFAULT_BUILD_TABLE: &str = "orders";

/// Exit code for a malformed command line.
const USAGE_EXIT_CODE: u8 = 2;

/// What the command line asked for.
#[derive(Debug)]
enum Invocation {
    /// Run a query.
    Run(Options),
    /// Print the version and exit.
    Version,
    /// Print the usage and exit.
    Help,
}

/// A fully specified run.
#[derive(Debug)]
struct Options {
    /// The SQL to execute; it refers to the table `lineitem`.
    sql: String,
    /// The Parquet file to query.
    path: PathBuf,
    /// How many threads the engine is allowed to use.
    threads: usize,
    /// Whether the file is loaded into memory before the query runs.
    resident: bool,
    /// The second relation, when the query joins two of them.
    build: Option<BuildTable>,
}

/// The second relation of a query that joins two of them.
#[derive(Debug)]
struct BuildTable {
    /// The Parquet file holding the second relation.
    path: PathBuf,
    /// The name the second relation is registered under.
    name: String,
}

/// Prints the version of the engine under comparison.
fn main() -> ExitCode {
    match parse_args(std::env::args().skip(1)) {
        Ok(Invocation::Version) => {
            println!("{}", version_line());
            ExitCode::SUCCESS
        }
        Ok(Invocation::Help) => {
            println!("{}", usage());
            ExitCode::SUCCESS
        }
        Ok(Invocation::Run(options)) => match run(options) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("datafusion-peer: {message}");
                ExitCode::FAILURE
            }
        },
        Err(message) => {
            eprintln!("datafusion-peer: {message}");
            eprintln!();
            eprintln!("{}", usage());
            ExitCode::from(USAGE_EXIT_CODE)
        }
    }
}

/// The version banner, naming both the engine and the Arrow version it reads
/// Parquet through.
fn version_line() -> String {
    format!("datafusion {} (arrow {})", datafusion::DATAFUSION_VERSION, arrow::ARROW_VERSION)
}

/// The usage text.
fn usage() -> String {
    format!(
        "datafusion-peer -- {version}

Usage:
  datafusion-peer --sql <SQL> --path <FILE> [--threads <N>] [--resident]
  datafusion-peer --sql <SQL> --path <FILE> --build-path <FILE> [--build-table <NAME>]
  datafusion-peer --version

Options:
  --sql <SQL>     SQL to run; it refers to the table `{TABLE_NAME}`.
  --path <FILE>   Parquet file to query.
  --threads <N>   Thread count to pin the engine to (default: 1).
  --resident      Load the file into memory first and time the load separately;
                    without it the Parquet scan happens inside the query.
  --build-path <FILE>
                  A second Parquet file to register, for a query that joins two
                    relations. It is loaded and registered the same way as the
                    first one.
  --build-table <NAME>
                  The name the second file is registered under (default:
                    `{DEFAULT_BUILD_TABLE}`); it only has an effect with
                    `--build-path`.
  --version       Print the version and exit.
  --help          Print this text and exit.",
        version = version_line()
    )
}

/// Parses the command line into an [`Invocation`].
fn parse_args<I: Iterator<Item = String>>(args: I) -> Result<Invocation, String> {
    let mut sql: Option<String> = None;
    let mut path: Option<PathBuf> = None;
    let mut threads: usize = 1;
    let mut resident = false;
    let mut build_path: Option<PathBuf> = None;
    let mut build_table = DEFAULT_BUILD_TABLE.to_owned();
    let mut args = args;

    while let Some(argument) = args.next() {
        // Accept both `--flag value` and `--flag=value`; the harness may use
        // either spelling.
        let (flag, inline) = match argument.split_once('=') {
            Some((flag, value)) => (flag, Some(value.to_owned())),
            None => (argument.as_str(), None),
        };

        match flag {
            "--sql" => sql = Some(take_value(&mut args, inline, "--sql")?),
            "--path" => path = Some(PathBuf::from(take_value(&mut args, inline, "--path")?)),
            "--threads" => {
                let raw = take_value(&mut args, inline, "--threads")?;
                threads = raw
                    .parse::<usize>()
                    .map_err(|_| format!("--threads expects a positive integer, got {raw:?}"))?;
                if threads == 0 {
                    return Err("--threads must be at least 1".to_owned());
                }
            }
            "--resident" => resident = true,
            "--build-path" => {
                build_path = Some(PathBuf::from(take_value(&mut args, inline, "--build-path")?));
            }
            "--build-table" => build_table = take_value(&mut args, inline, "--build-table")?,
            "--version" | "-V" => return Ok(Invocation::Version),
            "--help" | "-h" => return Ok(Invocation::Help),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }

    let sql = sql.ok_or_else(|| "--sql is required".to_owned())?;
    let path = path.ok_or_else(|| "--path is required".to_owned())?;
    // The name of the second table is only meaningful when there is a second
    // table, so a `--build-table` without a `--build-path` is simply unused.
    let build = build_path.map(|path| BuildTable { path, name: build_table });

    Ok(Invocation::Run(Options { sql, path, threads, resident, build }))
}

/// Returns the value of a flag, either the inline `--flag=value` form or the
/// following argument.
fn take_value<I: Iterator<Item = String>>(
    args: &mut I,
    inline: Option<String>,
    flag: &str,
) -> Result<String, String> {
    match inline {
        Some(value) => Ok(value),
        None => args.next().ok_or_else(|| format!("{flag} requires a value")),
    }
}

/// Runs the query and prints the markers.
fn run(options: Options) -> Result<(), String> {
    // The runtime is created here, outside `block_on`, so that the engine's
    // spawned tasks run on exactly the number of worker threads the harness
    // asked for.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(options.threads)
        .enable_all()
        .build()
        .map_err(|err| format!("failed to start the tokio runtime: {err}"))?;

    runtime.block_on(async move {
        let path_display = options.path.display().to_string();
        if !options.path.is_file() {
            return Err(format!("{path_display} is not a readable file"));
        }
        if let Some(build) = &options.build {
            let build_display = build.path.display().to_string();
            if !build.path.is_file() {
                return Err(format!("{build_display} is not a readable file"));
            }
        }

        // `target_partitions` is the engine's own notion of parallelism; it has
        // to match the pinned thread count or the measurement is meaningless.
        let config = SessionConfig::new().with_target_partitions(options.threads);
        let runtime_env = RuntimeEnvBuilder::new()
            .build_arc()
            .map_err(|err| format!("failed to build the DataFusion runtime environment: {err}"))?;
        let ctx = SessionContext::new_with_config_rt(config, runtime_env);

        let mut load_ms = 0.0;
        register_table(&ctx, TABLE_NAME, &options.path, options.resident, &mut load_ms).await?;
        if let Some(build) = &options.build {
            register_table(&ctx, &build.name, &build.path, options.resident, &mut load_ms).await?;
        }

        let started = Instant::now();
        let batches = ctx
            .sql(&options.sql)
            .await
            .map_err(|err| format!("query planning failed: {err}"))?
            .collect()
            .await
            .map_err(|err| format!("query execution failed: {err}"))?;
        let query_ms = started.elapsed().as_secs_f64() * 1_000.0;

        println!("LOAD_MS={load_ms:.3}");
        println!("QUERY_MS={query_ms:.3}");
        println!("VALUE={}", scalar_value(&batches));
        print_table(&batches);

        Ok(())
    })
}

/// Registers one Parquet file under `name`.
///
/// In resident mode the file is read eagerly and the time it took is added to
/// `load_ms`; in cold mode it is registered as a lazy Parquet scan, so the
/// read happens inside the query and is therefore part of `QUERY_MS`.
///
/// The two modes are the ones the harness asks for, and the second table of a
/// join is registered exactly like the first one, so that the two are read the
/// same way.
async fn register_table(
    ctx: &SessionContext,
    name: &str,
    path: &Path,
    resident: bool,
    load_ms: &mut f64,
) -> Result<(), String> {
    let path_str = path.to_string_lossy().into_owned();
    let path_display = path.display().to_string();

    if resident {
        let started = Instant::now();
        let batches = ctx
            .read_parquet(path_str.as_str(), ParquetReadOptions::default())
            .await
            .map_err(|err| format!("failed to load {path_display}: {err}"))?
            .collect()
            .await
            .map_err(|err| format!("failed to load {path_display}: {err}"))?;
        *load_ms += started.elapsed().as_secs_f64() * 1_000.0;

        let schema = batches
            .first()
            .map(RecordBatch::schema)
            .ok_or_else(|| format!("{path_display} produced no batches"))?;
        let mem_table = MemTable::try_new(schema, vec![batches])
            .map_err(|err| format!("failed to build the in-memory table: {err}"))?;
        let _ = ctx.register_table(name, Arc::new(mem_table));
    } else {
        // Cold mode: the table is a lazy scan, so the Parquet read happens
        // inside the query and is therefore part of `QUERY_MS`.
        ctx.register_parquet(name, path_str.as_str(), ParquetReadOptions::default())
            .await
            .map_err(|err| format!("failed to register {path_display}: {err}"))?;
    }

    Ok(())
}

/// Returns the single numeric scalar the query produced, or an empty string
/// when the result is anything else.
fn scalar_value(batches: &[RecordBatch]) -> String {
    let rows: usize = batches.iter().map(RecordBatch::num_rows).sum();
    if rows != 1 {
        return String::new();
    }
    // The one row may sit in any batch of the result, so pick the batch that
    // actually holds it rather than assuming it is the first one.
    let Some(batch) = batches.iter().find(|batch| batch.num_rows() == 1) else {
        return String::new();
    };
    if batch.num_columns() != 1 {
        return String::new();
    }

    let column = batch.column(0);
    if !is_numeric(column.data_type()) || column.is_null(0) {
        return String::new();
    }

    let options = FormatOptions::default();
    match ArrayFormatter::try_new(column.as_ref(), &options) {
        Ok(formatter) => formatter.value(0).to_string(),
        Err(_) => String::new(),
    }
}

/// Whether a data type can be reported as a single number.
fn is_numeric(data_type: &DataType) -> bool {
    matches!(
        data_type,
        DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
            | DataType::Float16
            | DataType::Float32
            | DataType::Float64
            | DataType::Decimal128(_, _)
            | DataType::Decimal256(_, _)
    )
}

/// Prints the result table for human inspection.
///
/// `arrow::util::pretty` needs Arrow's `prettyprint` feature. It is not
/// enabled here, but DataFusion asks for it unconditionally, and Cargo unifies
/// the features of a package across the whole dependency graph, so the Arrow
/// this binary links against has it.
fn print_table(batches: &[RecordBatch]) {
    match arrow::util::pretty::pretty_format_batches(batches) {
        Ok(table) => println!("{table}"),
        Err(err) => eprintln!("datafusion-peer: failed to format the result table: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_args, Invocation, Options};

    /// Parses a command line and returns the options it names.
    fn options(args: &[&str]) -> Options {
        match parse_args(args.iter().map(|arg| (*arg).to_owned())) {
            Ok(Invocation::Run(options)) => options,
            other => panic!("expected a run invocation, got {other:?}"),
        }
    }

    /// A command line with one file names one table and no second one.
    #[test]
    fn a_single_file_has_no_build_table() {
        let options = options(&["--sql", "SELECT 1", "--path", "lineitem.parquet"]);
        assert_eq!(options.path.to_string_lossy(), "lineitem.parquet");
        assert!(options.build.is_none());
        assert!(!options.resident);
    }

    /// The second file is registered under the default name.
    #[test]
    fn the_build_table_defaults_to_orders() {
        let options = options(&[
            "--sql",
            "SELECT 1",
            "--path",
            "lineitem.parquet",
            "--build-path",
            "orders.parquet",
        ]);
        let build = options.build.expect("a build table");
        assert_eq!(build.path.to_string_lossy(), "orders.parquet");
        assert_eq!(build.name, "orders");
    }

    /// The second file can be registered under another name, and the inline
    /// `--flag=value` spelling works for both flags.
    #[test]
    fn the_build_table_can_be_named() {
        let options = options(&[
            "--sql",
            "SELECT 1",
            "--path",
            "lineitem.parquet",
            "--build-path=orders.parquet",
            "--build-table=build_side",
        ]);
        let build = options.build.expect("a build table");
        assert_eq!(build.path.to_string_lossy(), "orders.parquet");
        assert_eq!(build.name, "build_side");
    }

    /// A flag without a value is a malformed command line, not a guess.
    #[test]
    fn a_build_path_without_a_value_is_rejected() {
        let result = parse_args(
            ["--sql", "SELECT 1", "--path", "lineitem.parquet", "--build-path"]
                .iter()
                .map(|arg| (*arg).to_owned()),
        );
        assert!(result.is_err());
    }
}
