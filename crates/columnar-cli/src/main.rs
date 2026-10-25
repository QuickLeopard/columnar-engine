//! The columnar engine's command line interface.
//!
//! Four subcommands, and the point of them is that a reviewer needs none of
//! them to be explained:
//!
//! - `dataset` — generate the benchmark dataset;
//! - `query`   — run a query and print the plan output;
//! - `bench`   — the kernel A/B;
//! - `compare` — this engine against `DuckDB` and `Polars`, on the same file.
//!
//! Every one of them writes machine-readable JSON as well as printing a human
//! summary, so the numbers in `PERF.md` came out of a tool rather than out of
//! someone's notes.

// The dataset's integers are widened to `f64` so the kernel measurement can
// use one path. The values are small, so the conversion is exact.
#![allow(clippy::cast_precision_loss)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use columnar_bench::{
    external::{self, Comparator, ExternalQuery},
    kernel_ab::KernelAb,
    machine::MachineSpec,
    result::{BenchReport, DatasetReport, EngineQueryResult},
    scaling::{ScalingPoint, ScalingReport},
};
use columnar_core::ComparisonOp;
use columnar_engine::{Dataset, ExecOptions};

use columnar_cli::queries::{BuiltinQuery, QueryDefinition, QUERIES};

#[derive(Debug, Parser)]
#[command(
    name = "columnar",
    version,
    about = "A vectorized columnar query engine with SIMD comparison kernels",
    long_about = None
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Generate the benchmark dataset.
    Dataset {
        /// The TPC-H scale factor. Scale factor 1 is 6,000,000 rows.
        #[arg(long, default_value_t = 1)]
        scale_factor: u32,
        /// The PRNG seed. The dataset is a pure function of this and the
        /// scale factor.
        #[arg(long, default_value_t = 24001)]
        seed: u64,
        /// The fraction of rows whose `l_discount` and `l_comment` are NULL.
        #[arg(long, default_value_t = 0.0)]
        null_rate: f64,
        /// Where to write the Parquet file.
        #[arg(long, default_value = "data/lineitem.parquet")]
        output: PathBuf,
    },

    /// Run a query and print the plan output.
    Query {
        /// The Parquet file to read.
        #[arg(long, default_value = "data/lineitem.parquet")]
        data: PathBuf,
        /// Which query to run.
        #[arg(long, value_enum, default_value_t = BuiltinQueryArg::Q6)]
        query: BuiltinQueryArg,
        /// How many times to run it. The median is reported.
        #[arg(long, default_value_t = 5)]
        iterations: usize,
        /// Read only the columns the query needs, with the query's predicates
        /// pushed into the Parquet reader.
        ///
        /// The lazy path reads the file *during* the query, so its timing is
        /// a cold number: there is no resident form.
        #[arg(long)]
        lazy: bool,
        /// How many threads to read the file with.
        ///
        /// The read splits the file's row groups across the threads and merges
        /// the parts back in the file's order, so the answer does not depend
        /// on the thread count. One is the default, and one is what the
        /// headline comparison runs at.
        #[arg(long, default_value_t = 1)]
        threads: usize,
    },

    /// The kernel A/B: every flavor, on the real data.
    Bench {
        /// The Parquet file to read.
        #[arg(long, default_value = "data/lineitem.parquet")]
        data: PathBuf,
        /// How many rows to process per pass.
        #[arg(long, default_value_t = 20_000_000)]
        rows: usize,
        /// How many timed passes to make.
        #[arg(long, default_value_t = 5)]
        iterations: usize,
        /// Which column to filter.
        #[arg(long, default_value = "l_discount")]
        column: String,
        /// The threshold to compare against.
        #[arg(long, default_value_t = 0.04)]
        threshold: f64,
        /// Where to write the JSON report.
        #[arg(long, default_value = "bench-results/kernel.json")]
        out: PathBuf,
    },

    /// Check the current measurement against the stored performance baseline.
    ///
    /// This measures the machine, so it is a command and not a test: a shared
    /// machine is noisy, and a gate that fails at random is a gate nobody
    /// trusts.
    Gate {
        /// The Parquet file to read.
        #[arg(long, default_value = "data/lineitem.parquet")]
        data: PathBuf,
        /// How many rows to process per pass.
        #[arg(long, default_value_t = 20_000_000)]
        rows: usize,
        /// How many timed passes to make per block.
        #[arg(long, default_value_t = 5)]
        iterations: usize,
        /// How many independent blocks to measure.
        #[arg(long, default_value_t = 32)]
        blocks: usize,
        /// Which column to filter.
        #[arg(long, default_value = "l_discount")]
        column: String,
        /// The threshold to compare against.
        #[arg(long, default_value_t = 0.04)]
        threshold: f64,
        /// The baseline to compare against.
        #[arg(long, default_value = "bench-results/kernel-baseline.json")]
        baseline: PathBuf,
        /// Compare even if the baseline was measured on another machine.
        #[arg(long, default_value_t = false)]
        allow_different_machine: bool,
        /// Write the current measurement here, as a new baseline.
        #[arg(long)]
        write_baseline: Option<PathBuf>,
    },

    /// Compare this engine against `DuckDB` and `Polars` on the same file.
    Compare {
        /// The Parquet file to read.
        #[arg(long, default_value = "data/lineitem.parquet")]
        data: PathBuf,
        /// How many timed runs to make per engine.
        #[arg(long, default_value_t = 5)]
        iterations: usize,
        /// The thread count to pin every engine to.
        #[arg(long, default_value_t = 1)]
        threads: usize,
        /// Where to write the JSON report.
        #[arg(long, default_value = "bench-results/compare.json")]
        out: PathBuf,
    },

    /// Sweep the thread count: this engine and both incumbents, at the same
    /// thread counts, on the same file.
    ///
    /// This is the **second** table, not a replacement for the first. The
    /// headline comparison stays pinned to one thread, because that is the
    /// like-for-like claim. This one shows how each system's number moves
    /// with the thread count, so the reader can see the shape of the curve
    /// and not only its endpoint.
    Sweep {
        /// The Parquet file to read.
        #[arg(long, default_value = "data/lineitem.parquet")]
        data: PathBuf,
        /// Which query to sweep.
        #[arg(long, value_enum, default_value_t = BuiltinQueryArg::Q6)]
        query: BuiltinQueryArg,
        /// The thread counts to sweep, comma separated.
        #[arg(long, default_value = "1,2,4,8,14")]
        threads: String,
        /// How many timed runs to make per system, per thread count.
        #[arg(long, default_value_t = 5)]
        iterations: usize,
        /// Where to write the JSON report.
        #[arg(long, default_value = "bench-results/scaling.json")]
        out: PathBuf,
    },
}

/// The builtin queries, as command line values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum BuiltinQueryArg {
    /// `SELECT count(*) FROM lineitem` -- the COUNT(*) fast path.
    Scan,
    /// A single numeric predicate with a COUNT(*).
    FilterNumeric,
    /// A single predicate on a dictionary-encoded string column.
    FilterString,
    /// Two numeric predicates and a SUM.
    FilterRange,
    /// The TPC-H Q6 predicate set, with COUNT(*) in place of the computed sum.
    Q6,
}

impl From<BuiltinQueryArg> for BuiltinQuery {
    fn from(value: BuiltinQueryArg) -> Self {
        match value {
            BuiltinQueryArg::Scan => BuiltinQuery::Scan,
            BuiltinQueryArg::FilterNumeric => BuiltinQuery::FilterNumeric,
            BuiltinQueryArg::FilterString => BuiltinQuery::FilterString,
            BuiltinQueryArg::FilterRange => BuiltinQuery::FilterRange,
            BuiltinQueryArg::Q6 => BuiltinQuery::Q6,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Runs the requested subcommand.
fn run(cli: Cli) -> Result<()> {
    // `bench`, `compare` and `sweep` publish timings. Those timings must
    // describe hardware, so they are refused when the SIMD probe has been
    // overridden -- under Rosetta 2 the override points at an emulator. The
    // check lives here as well as in the harness, so the CLI can say why
    // rather than panic.
    if matches!(
        cli.command,
        Command::Bench { .. }
            | Command::Compare { .. }
            | Command::Gate { .. }
            | Command::Sweep { .. }
    ) && columnar_bench::machine::simd_probe_is_overridden()
    {
        anyhow::bail!("{}", columnar_bench::machine::FORCED_SIMD_REFUSAL);
    }

    match cli.command {
        Command::Dataset { scale_factor, seed, null_rate, output } => {
            run_dataset(scale_factor, seed, null_rate, &output)
        }
        Command::Query { data, query, iterations, lazy, threads } => {
            run_query(&data, query, iterations, lazy, threads)
        }
        Command::Bench { data, rows, iterations, column, threshold, out } => {
            run_bench(&data, rows, iterations, &column, threshold, &out)
        }
        Command::Gate {
            data,
            rows,
            iterations,
            blocks,
            column,
            threshold,
            baseline,
            allow_different_machine,
            write_baseline,
        } => run_gate(&GateOptions {
            data: &data,
            rows,
            iterations,
            blocks,
            column: &column,
            threshold,
            baseline_path: &baseline,
            allow_different_machine,
            write_baseline: write_baseline.as_deref(),
        }),
        Command::Compare { data, iterations, threads, out } => {
            run_compare(&data, iterations, threads, &out)
        }
        Command::Sweep { data, query, threads, iterations, out } => {
            run_sweep(&data, query, &threads, iterations, &out)
        }
    }
}

/// Generates the benchmark dataset.
fn run_dataset(scale_factor: u32, seed: u64, null_rate: f64, output: &Path) -> Result<()> {
    {
        {
            let spec = columnar_testdata::DatasetSpec { scale_factor, seed, null_rate };
            let manifest = columnar_testdata::generate(&spec, output).with_context(|| {
                format!("could not generate the dataset at {}", output.display())
            })?;
            manifest.write_json().context("could not write the dataset manifest")?;
            println!("wrote {}", manifest.path);
            println!("  rows         {}", manifest.rows);
            println!("  bytes        {}", manifest.bytes);
            println!("  scale factor {}", manifest.scale_factor);
            println!("  seed         {}", manifest.seed);
            println!("  null rate    {}", manifest.null_rate);
            println!("  provenance   {}", manifest.provenance);
        }
    }
    Ok(())
}

/// Runs one query and prints its plan output.
fn run_query(
    data: &Path,
    query: BuiltinQueryArg,
    iterations: usize,
    lazy: bool,
    threads: usize,
) -> Result<()> {
    {
        {
            let builtin = BuiltinQuery::from(query);
            let definition =
                QUERIES.iter().find(|q| q.query == builtin).context("unknown query")?;

            let mut timings = Vec::with_capacity(iterations);
            let mut last = None;

            if lazy {
                // The lazy path opens the file and reads it during the query.
                // The open is timed separately, because it is the only part
                // that happens before the query starts.
                let options =
                    ExecOptions { threads: Some(threads.max(1)), ..ExecOptions::default() };
                let mut opens = Vec::with_capacity(iterations.max(1));
                for _ in 0..iterations.max(1) {
                    let open_started = std::time::Instant::now();
                    let source = columnar_engine::ParquetSource::open(data)
                        .with_context(|| format!("could not open {}", data.display()))?;
                    opens.push(open_started.elapsed().as_secs_f64() * 1000.0);

                    let started = std::time::Instant::now();
                    let result =
                        columnar_engine::execute_on(&(definition.plan)(), &source, &options)
                            .context("the query failed")?;
                    timings.push(started.elapsed().as_secs_f64() * 1000.0);
                    last = Some(result);
                }
                opens.sort_by(f64::total_cmp);
                timings.sort_by(f64::total_cmp);
                let result = last.context("no result")?;
                println!("{}", definition.sql_for_display());
                println!();
                println!("{}", result.plan_output);
                println!();
                println!(
                    "median wall clock: {:.1} ms over {} runs (open: {:.1} ms)",
                    timings.get(timings.len() / 2).copied().unwrap_or(0.0),
                    timings.len(),
                    opens.get(opens.len() / 2).copied().unwrap_or(0.0)
                );
                for group in &result.groups {
                    println!("  group {:?} -> {:?}", group.key, group.values);
                }
            } else {
                let dataset = Dataset::read_parquet(data)
                    .with_context(|| format!("could not read {}", data.display()))?;
                for _ in 0..iterations.max(1) {
                    let started = std::time::Instant::now();
                    let result = columnar_engine::execute(
                        &(definition.plan)(),
                        &dataset,
                        &ExecOptions::default(),
                    )
                    .context("the query failed")?;
                    timings.push(started.elapsed().as_secs_f64() * 1000.0);
                    last = Some(result);
                }
                timings.sort_by(f64::total_cmp);

                let result = last.context("no result")?;
                println!("{}", definition.sql_for_display());
                println!();
                println!("{}", result.plan_output);
                println!();
                println!(
                    "median wall clock: {:.1} ms over {} runs",
                    timings.get(timings.len() / 2).copied().unwrap_or(0.0),
                    timings.len()
                );
                for group in &result.groups {
                    println!("  group {:?} -> {:?}", group.key, group.values);
                }
            }
        }
    }
    Ok(())
}

/// Runs the kernel A/B.
fn run_bench(
    data: &Path,
    rows: usize,
    iterations: usize,
    column: &str,
    threshold: f64,
    out: &Path,
) -> Result<()> {
    {
        {
            let dataset = Dataset::read_parquet(data)
                .with_context(|| format!("could not read {}", data.display()))?;
            let chunk = dataset
                .chunk(column)
                .with_context(|| format!("the dataset has no column `{column}`"))?;

            // Decode the column into a dense array, which is what the kernel
            // reads. The kernel is measured on the same data the engine
            // actually sees, not on synthetic numbers.
            let decoded = chunk.decode();
            let values: Vec<f64> = match &decoded.data {
                columnar_core::BatchData::Float64(values) => values.clone(),
                columnar_core::BatchData::Int64(values) => {
                    // The kernel measures `f64` columns. An `int64` column is
                    // widened so the same measurement path can be used; the
                    // values in this dataset are small, so the conversion is
                    // exact.
                    values.iter().map(|&v| v as f64).collect()
                }
                columnar_core::BatchData::Utf8(_) => {
                    anyhow::bail!("the kernel A/B measures numeric columns only")
                }
            };
            let values: Vec<f64> = values.iter().copied().cycle().take(rows).collect();

            let ab = KernelAb::measure(column, &values, ComparisonOp::GtEq, threshold, iterations);

            let report = BenchReport {
                machine: MachineSpec::capture(1),
                dataset: dataset_report(data, &dataset),
                kernel_ab: ab,
                external: None,
                engine_queries: None,
            };
            report
                .write_json(out)
                .with_context(|| format!("could not write the report to {}", out.display()))?;
            println!("{}", report.render());
            println!("report written to {}", out.display());
        }
    }
    Ok(())
}

/// Compares the engine against `DuckDB` and `Polars` on the same file.
fn run_compare(data: &Path, iterations: usize, threads: usize, out: &Path) -> Result<()> {
    {
        {
            let dataset = Dataset::read_parquet(data)
                .with_context(|| format!("could not read {}", data.display()))?;

            let mut engine_queries = Vec::new();
            let mut external_results = Vec::new();

            for definition in QUERIES {
                let measured = measure_both_paths(definition, data, iterations)?;
                // The engine's own answer is the reference for the cross-check
                // against the incumbents below. Both paths computed it, and
                // they agreed -- `measure_both_paths` refuses otherwise.
                let result_value = measured
                    .iter()
                    .find(|entry| entry.mode == "eager")
                    .and_then(|entry| entry.result_value);
                engine_queries.extend(measured);

                for mode in [external::Mode::Resident, external::Mode::Cold] {
                    for comparator in [Comparator::DuckDb, Comparator::Polars] {
                        let query = ExternalQuery::new(
                            definition.query.name(),
                            definition.sql_for(comparator, mode, data),
                        );
                        match external::run(comparator, mode, &query, data, threads, iterations) {
                            Ok(outcome) => {
                                // Cross-check the answer. Two systems that
                                // computed different values are not being
                                // compared, they are answering different
                                // questions.
                                if let (Some(ours), Some(theirs)) =
                                    (result_value, outcome.result_value)
                                {
                                    // A SUM over hundreds of thousands of
                                    // values is not associative in floating
                                    // point, so two engines that sum in a
                                    // different order can differ in the last
                                    // bits. The tolerance is relative and
                                    // loose enough to allow that, and tight
                                    // enough to catch a real disagreement.
                                    let tolerance = 1e-9 * ours.abs().max(1.0);
                                    if (ours - theirs).abs() > tolerance {
                                        eprintln!(
                                            "WARNING: {} {} computed {}, the engine computed {}",
                                            comparator.name(),
                                            definition.query.name(),
                                            theirs,
                                            ours
                                        );
                                    }
                                }
                                external_results.push(external::ExternalComparison {
                                    query: definition.query.name().to_owned(),
                                    comparator: comparator.name().to_owned(),
                                    mode: mode.name().to_owned(),
                                    version: outcome.version,
                                    load_ms: outcome.load_ms,
                                    query_ms: outcome.query_ms,
                                    elapsed_ms: outcome.elapsed_ms,
                                    result: outcome.result,
                                    result_value: outcome.result_value,
                                    iterations: iterations.max(1),
                                });
                            }
                            Err(error) => {
                                eprintln!(
                                    "skipping {} {}: {error}",
                                    comparator.name(),
                                    mode.name()
                                );
                            }
                        }
                    }
                }
            }

            let report = BenchReport {
                machine: MachineSpec::capture(threads),
                dataset: dataset_report(data, &dataset),
                kernel_ab: KernelAb {
                    rows: 0,
                    iterations: 0,
                    column: String::new(),
                    operator: String::new(),
                    threshold: 0.0,
                    flavors: Vec::new(),
                },
                external: Some(external_results),
                engine_queries: Some(engine_queries),
            };
            report
                .write_json(out)
                .with_context(|| format!("could not write the report to {}", out.display()))?;
            println!("{}", report.render());
            println!("report written to {}", out.display());
        }
    }
    Ok(())
}

/// Sweeps the thread count: this engine and both incumbents, at the same
/// thread counts, on the same file.
///
/// This is the **second** table. The headline comparison stays pinned to one
/// thread -- that is the like-for-like claim, and it is unchanged. This one
/// shows how each system's number moves with the thread count, so a reader
/// sees the shape of the curve and not only its endpoint.
///
/// Two rules keep it a comparison rather than three measurements:
///
/// - **Every system runs at the same thread count.** Principle V requires
///   symmetry: the incumbents are pinned to the engine's thread count at
///   every point of the sweep, exactly as the headline comparison pins them
///   to one.
/// - **Every system runs on the same file, in the same harness, cold.** The
///   engine's number is the lazy path's, because the lazy path is the one
///   that reads the file during the query -- which is what the incumbents'
///   cold numbers do.
///
/// The answers are cross-checked at every point. A sweep that published
/// timings for systems computing different answers would be a sweep of
/// nothing.
fn run_sweep(
    data: &Path,
    query: BuiltinQueryArg,
    threads: &str,
    iterations: usize,
    out: &Path,
) -> Result<()> {
    let counts = parse_thread_counts(threads)?;
    let builtin = BuiltinQuery::from(query);
    let definition = QUERIES.iter().find(|q| q.query == builtin).context("unknown query")?;

    let dataset = Dataset::read_parquet(data)
        .with_context(|| format!("could not read {}", data.display()))?;
    let source = columnar_engine::ParquetSource::open(data)
        .with_context(|| format!("could not open {}", data.display()))?;
    let row_groups = source.row_groups();

    let mut points = Vec::with_capacity(counts.len());
    let mut reference: Option<String> = None;

    for count in &counts {
        // The engine.
        let (_open_ms, engine_ms, engine_scan_ms, engine_result) =
            measure_engine_lazy_at(definition, data, iterations, *count)?;

        // The engine's answer at this thread count has to be the same as at
        // every other one. The rows come back in the file's order whatever
        // the thread count is, so any difference is a defect and not a
        // rounding error.
        let rendered = format!("{:?}", engine_result.groups);
        match &reference {
            None => reference = Some(rendered.clone()),
            Some(expected) if *expected != rendered => {
                anyhow::bail!(
                    "the engine computed different answers at {count} threads: {rendered} against {expected}"
                );
            }
            Some(_) => {}
        }

        // The incumbents, at the same thread count.
        let mut incumbents = Vec::new();
        for comparator in [Comparator::DuckDb, Comparator::Polars] {
            let query_sql = ExternalQuery::new(
                definition.query.name(),
                definition.sql_for(comparator, external::Mode::Cold, data),
            );
            match external::run(
                comparator,
                external::Mode::Cold,
                &query_sql,
                data,
                *count,
                iterations,
            ) {
                Ok(outcome) => {
                    if let (Some(ours), Some(theirs)) = (
                        engine_result
                            .groups
                            .first()
                            .and_then(|group| group.values.first())
                            .and_then(|value| match value {
                                columnar_engine::AggregateValue::Int(v) => Some(*v as f64),
                                columnar_engine::AggregateValue::Float(v) => Some(*v),
                                columnar_engine::AggregateValue::Null => None,
                            }),
                        outcome.result_value,
                    ) {
                        let tolerance = 1e-9 * ours.abs().max(1.0);
                        if (ours - theirs).abs() > tolerance {
                            anyhow::bail!(
                                "{} computed {theirs} at {count} threads, the engine computed {ours}",
                                comparator.name()
                            );
                        }
                    }
                    incumbents.push((comparator.name(), outcome.query_ms));
                }
                Err(error) => {
                    eprintln!("skipping {} at {count} threads: {error}", comparator.name());
                }
            }
        }

        let duckdb_ms =
            incumbents.iter().find(|(name, _)| *name == "duckdb").map_or(f64::NAN, |(_, ms)| *ms);
        let polars_ms =
            incumbents.iter().find(|(name, _)| *name == "polars").map_or(f64::NAN, |(_, ms)| *ms);

        points.push(ScalingPoint {
            threads: *count,
            workers: source.worker_count(*count),
            engine_ms,
            engine_scan_ms,
            duckdb_ms,
            polars_ms,
        });

        println!(
            "{count:>3} threads ({:>2} workers) \u{b7} engine {engine_ms:.1} ms \u{b7} scan {engine_scan_ms:.1} ms \u{b7} duckdb {duckdb_ms:.1} ms \u{b7} polars {polars_ms:.1} ms",
            source.worker_count(*count)
        );
    }

    let report = ScalingReport {
        machine: MachineSpec::capture(counts.first().copied().unwrap_or(1)),
        dataset: dataset_report(data, &dataset),
        query: definition.query.name().to_owned(),
        sql: definition.sql_for_display(),
        mode: "cold".to_owned(),
        row_groups,
        points,
    };
    report
        .write_json(out)
        .with_context(|| format!("could not write the report to {}", out.display()))?;
    println!();
    println!("{}", report.render());
    println!("report written to {}", out.display());
    Ok(())
}

/// Parses a comma-separated list of thread counts.
///
/// A count of zero is refused: zero threads would mean no read at all, and a
/// sweep that contained one would be a sweep of a different query.
fn parse_thread_counts(text: &str) -> Result<Vec<usize>> {
    let mut counts = Vec::new();
    for part in text.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let count: usize =
            trimmed.parse().with_context(|| format!("`{trimmed}` is not a thread count"))?;
        if count == 0 {
            anyhow::bail!("a thread count of zero would mean no read at all");
        }
        counts.push(count);
    }
    if counts.is_empty() {
        anyhow::bail!("no thread counts to sweep");
    }
    counts.sort_unstable();
    counts.dedup();
    Ok(counts)
}

/// Measures the engine on one query, both ways.
///
/// The eager path is measured the way the incumbents are: once with the data
/// already resident, and once cold, reading the file and answering the query
/// in one go. The lazy path is measured once, because it has no resident
/// form: it reads the file during the query, so its number is a cold number
/// by construction.
///
/// The two paths must agree, or one of them is wrong. The engine's own
/// answer is also the reference for the cross-check against the incumbents.
fn measure_both_paths(
    definition: &QueryDefinition,
    data: &Path,
    iterations: usize,
) -> Result<Vec<EngineQueryResult>> {
    let (load_ms, resident_ms, cold_ms, result) = measure_engine(definition, data, iterations)?;
    let (lazy_open_ms, lazy_ms, lazy_result) = measure_engine_lazy(definition, data, iterations)?;

    if lazy_result.groups != result.groups {
        anyhow::bail!(
            "the lazy and the eager path disagree on `{}`: {:?} against {:?}",
            definition.query.name(),
            lazy_result.groups,
            result.groups
        );
    }

    // The numeric answer, so it can be cross-checked against each incumbent's
    // answer.
    let result_value = result.groups.first().and_then(|group| group.values.first()).and_then(
        |value| match value {
            columnar_engine::AggregateValue::Int(v) => Some(*v as f64),
            columnar_engine::AggregateValue::Float(v) => Some(*v),
            columnar_engine::AggregateValue::Null => None,
        },
    );

    Ok(vec![
        EngineQueryResult {
            query: definition.query.name().to_owned(),
            mode: "eager".to_owned(),
            sql: definition.sql_for_display(),
            elapsed_ms: resident_ms,
            load_ms,
            cold_ms,
            result: format!("{:?}", result.groups),
            result_value,
            plan_output: result.plan_output.to_string(),
        },
        EngineQueryResult {
            query: definition.query.name().to_owned(),
            mode: "lazy".to_owned(),
            sql: definition.sql_for_display(),
            elapsed_ms: lazy_ms,
            load_ms: lazy_open_ms,
            cold_ms: lazy_ms,
            result: format!("{:?}", lazy_result.groups),
            result_value,
            plan_output: lazy_result.plan_output.to_string(),
        },
    ])
}

/// Measures the engine on one query, the way the incumbents are measured.
///
/// Returns `(load ms, resident query ms, cold ms, the result)`.
///
/// All three numbers come from the same timed runs. Each iteration reads the
/// Parquet file and then answers the query, and the two halves are timed
/// separately:
///
/// - **load** is the read: how long the file takes to become queryable.
/// - **resident** is the query against the data that was just read -- which
///   is the same question the incumbents' resident measurements answer.
/// - **cold** is the two together: read the file, then answer.
///
/// Reading the file inside the timed region is deliberate. The incumbents
/// pay for their read too, and an engine number that excludes its own I/O
/// would not be comparable with theirs.
fn measure_engine(
    definition: &QueryDefinition,
    data: &Path,
    iterations: usize,
) -> Result<(f64, f64, f64, columnar_engine::AggregateResult)> {
    let iterations = iterations.max(1);

    // Warm-up: the first read pays for the page cache and the allocator.
    {
        let dataset = Dataset::read_parquet(data)
            .with_context(|| format!("could not read {}", data.display()))?;
        let _ = columnar_engine::execute(&(definition.plan)(), &dataset, &ExecOptions::default());
    }

    let mut loads = Vec::with_capacity(iterations);
    let mut resident = Vec::with_capacity(iterations);
    let mut colds = Vec::with_capacity(iterations);
    let mut result = None;

    for _ in 0..iterations {
        let read_started = std::time::Instant::now();
        let dataset = Dataset::read_parquet(data)
            .with_context(|| format!("could not read {}", data.display()))?;
        let load_ms = read_started.elapsed().as_secs_f64() * 1000.0;

        let query_started = std::time::Instant::now();
        let value =
            columnar_engine::execute(&(definition.plan)(), &dataset, &ExecOptions::default())
                .context("the query failed")?;
        let query_ms = query_started.elapsed().as_secs_f64() * 1000.0;

        loads.push(load_ms);
        resident.push(query_ms);
        colds.push(load_ms + query_ms);
        result = Some(value);
    }

    Ok((median(&loads), median(&resident), median(&colds), result.context("no result")?))
}

/// Measures the engine on one query, the lazy way.
///
/// Returns `(open ms, the whole query ms, the result)`.
///
/// The lazy path reads the file *during* the query, so it has no separate
/// "resident" form: the number it produces is a cold number by
/// construction, and it is the number to compare with the incumbents' cold
/// measurements.
///
/// The **open** is the part the lazy path does before the query starts:
/// reading the file's footer, and nothing else. It is reported separately
/// because it is the honest answer to "how long does the file take to
/// become queryable" for a path that never holds the whole file.
fn measure_engine_lazy(
    definition: &QueryDefinition,
    data: &Path,
    iterations: usize,
) -> Result<(f64, f64, columnar_engine::AggregateResult)> {
    let (open_ms, total_ms, _scan_ms, result) =
        measure_engine_lazy_at(definition, data, iterations, 1)?;
    Ok((open_ms, total_ms, result))
}

/// Measures the engine on one query, the lazy way, at a given thread count.
///
/// Returns `(open ms, the whole query ms, the scan stage ms, the result)`.
///
/// The **scan stage** is reported alongside the total because the scan is
/// where the parallelism is. A reader who sees the total fall while the scan
/// does not knows immediately that the parallelism went somewhere other than
/// where they hoped.
fn measure_engine_lazy_at(
    definition: &QueryDefinition,
    data: &Path,
    iterations: usize,
    threads: usize,
) -> Result<(f64, f64, f64, columnar_engine::AggregateResult)> {
    let iterations = iterations.max(1);
    let options = ExecOptions { threads: Some(threads.max(1)), ..ExecOptions::default() };

    // Warm-up: the first open pays for the page cache.
    {
        let source = columnar_engine::ParquetSource::open(data)
            .with_context(|| format!("could not open {}", data.display()))?;
        let _ = columnar_engine::execute_on(&(definition.plan)(), &source, &options);
    }

    let mut opens = Vec::with_capacity(iterations);
    let mut totals = Vec::with_capacity(iterations);
    let mut scans = Vec::with_capacity(iterations);
    let mut result = None;

    for _ in 0..iterations {
        let open_started = std::time::Instant::now();
        let source = columnar_engine::ParquetSource::open(data)
            .with_context(|| format!("could not open {}", data.display()))?;
        let open_ms = open_started.elapsed().as_secs_f64() * 1000.0;

        let query_started = std::time::Instant::now();
        let value = columnar_engine::execute_on(&(definition.plan)(), &source, &options)
            .context("the query failed")?;
        let query_ms = query_started.elapsed().as_secs_f64() * 1000.0;

        opens.push(open_ms);
        totals.push(query_ms);
        scans.push(scan_stage_ms(&value));
        result = Some(value);
    }

    Ok((median(&opens), median(&totals), median(&scans), result.context("no result")?))
}

/// The engine's scan stage, in milliseconds.
///
/// The scan stage is the read: the Parquet decode, the pushed-down
/// predicates and the merge. It is the stage the thread count acts on.
fn scan_stage_ms(result: &columnar_engine::AggregateResult) -> f64 {
    result
        .plan_output
        .stages
        .iter()
        .find(|stage| stage.stage == "scan")
        .map_or(0.0, |stage| stage.elapsed.as_secs_f64() * 1000.0)
}

/// The median of a set of samples.
///
/// The median rather than the mean, because a single scheduling hiccup on a
/// shared machine should not move the number.
fn median(samples: &[f64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted.get(sorted.len() / 2).copied().unwrap_or(0.0)
}

/// Describes the dataset in the report.
///
/// The provenance comes from the manifest that sits next to the dataset. When
/// there is no manifest -- a dataset produced by something other than this
/// repository's generator -- the scale factor, seed and null rate are
/// reported as unknown rather than as zero. A report that says "scale factor
/// 0" for a dataset it knows nothing about is not a report.
fn dataset_report(path: &Path, dataset: &Dataset) -> DatasetReport {
    let bytes = std::fs::metadata(path).map_or(0, |meta| meta.len());
    let manifest = columnar_testdata::DatasetManifest::read_json(path).ok();
    DatasetReport {
        path: path.display().to_string(),
        rows: dataset.len() as u64,
        bytes,
        scale_factor: manifest.as_ref().map(|m| m.scale_factor),
        seed: manifest.as_ref().map(|m| m.seed),
        null_rate: manifest.as_ref().map(|m| m.null_rate),
    }
}

/// The options the performance gate runs with.
struct GateOptions<'a> {
    /// The Parquet file to read.
    data: &'a Path,
    /// How many rows to process per pass.
    rows: usize,
    /// How many timed passes to make per block.
    iterations: usize,
    /// How many independent blocks to measure.
    blocks: usize,
    /// Which column to filter.
    column: &'a str,
    /// The threshold to compare against.
    threshold: f64,
    /// The baseline to compare against.
    baseline_path: &'a Path,
    /// Compare even if the baseline was measured on another machine.
    allow_different_machine: bool,
    /// Write the current measurement here, as a new baseline.
    write_baseline: Option<&'a Path>,
}

/// Checks the current measurement against the stored performance baseline.
///
/// The gate measures the machine, so it is a command and not a test. It has
/// three possible answers -- no regression, a regression, and *inconclusive* --
/// and the third one is not a pass. A reader who sees "inconclusive" knows
/// the gate could not tell, which is more useful than a pass that was
/// decided by a threshold nobody examined.
fn run_gate(options: &GateOptions<'_>) -> Result<()> {
    let (_, values) = load_kernel_values(options.data, options.rows, options.column)?;
    let machine = MachineSpec::capture(1);
    let samples = KernelAb::measure_samples(
        &values,
        ComparisonOp::GtEq,
        options.threshold,
        options.iterations,
        options.blocks,
    );

    // Writing a baseline is a deliberate act: the samples become the thing
    // every later measurement is compared against.
    if let Some(path) = options.write_baseline {
        return write_baseline(path, &machine, &samples, options.blocks);
    }

    let baseline = read_baseline(options.baseline_path)?;
    let report =
        columnar_bench::gate::check(&samples, &baseline, &machine, options.allow_different_machine)
            .map_err(|error| anyhow::anyhow!("{error}"))?;

    print_gate_report(&report, &baseline, &machine, options.baseline_path);
    if report.failed() {
        anyhow::bail!("the performance gate failed");
    }
    Ok(())
}

/// Reads the column the kernel measures and cycles it to the row count.
fn load_kernel_values(data: &Path, rows: usize, column: &str) -> Result<(Dataset, Vec<f64>)> {
    let dataset = Dataset::read_parquet(data)
        .with_context(|| format!("could not read {}", data.display()))?;
    let chunk =
        dataset.chunk(column).with_context(|| format!("the dataset has no column `{column}`"))?;
    let decoded = chunk.decode();
    let values: Vec<f64> = match &decoded.data {
        columnar_core::BatchData::Float64(values) => values.clone(),
        columnar_core::BatchData::Int64(values) => values.iter().map(|&v| v as f64).collect(),
        columnar_core::BatchData::Utf8(_) => {
            anyhow::bail!("the performance gate measures numeric columns only")
        }
    };
    let values: Vec<f64> = values.iter().copied().cycle().take(rows).collect();
    Ok((dataset, values))
}

/// Writes a new baseline from a fresh measurement.
fn write_baseline(
    path: &Path,
    machine: &MachineSpec,
    samples: &std::collections::BTreeMap<String, Vec<f64>>,
    blocks: usize,
) -> Result<()> {
    let mut budgets = std::collections::BTreeMap::new();
    for (flavor, flavor_samples) in samples {
        let median = columnar_bench::gate::median(flavor_samples);
        // A budget of 80% of the measured median: a floor with a little
        // headroom for the machine's own noise, and a gate that fires when
        // the code gets slower rather than when it gets different.
        budgets.insert(flavor.clone(), median * 0.8);
    }
    let baseline = columnar_bench::gate::Baseline {
        machine: machine.clone(),
        alpha: 0.05,
        samples_per_flavor: blocks,
        flavors: samples.clone(),
        budgets,
        // Five per cent. Measured, not chosen: five consecutive gate runs
        // against an unchanged baseline reported differences of 0.9% to
        // 3.9%, and a threshold below that would fire on the machine's own
        // drift.
        min_relative_regression: 0.05,
    };
    std::fs::write(path, baseline.to_json())
        .with_context(|| format!("could not write {}", path.display()))?;
    println!("wrote the baseline to {}", path.display());
    for (flavor, flavor_samples) in samples {
        println!(
            "  {:<10} median {:>12.3} G rows/s over {} samples",
            flavor,
            columnar_bench::gate::median(flavor_samples) / 1e9,
            flavor_samples.len()
        );
    }
    Ok(())
}

/// Reads the stored baseline.
fn read_baseline(path: &Path) -> Result<columnar_bench::gate::Baseline> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    columnar_bench::gate::Baseline::from_json(&text)
        .with_context(|| format!("could not read {}", path.display()))
}

/// Prints the gate's verdict, one line per flavor.
fn print_gate_report(
    report: &columnar_bench::gate::GateReport,
    baseline: &columnar_bench::gate::Baseline,
    machine: &MachineSpec,
    baseline_path: &Path,
) {
    println!("performance gate");
    println!("  baseline    {}", baseline_path.display());
    println!("  alpha       {}", baseline.alpha);
    println!(
        "  threshold   {:.1}% -- a regression has to be this much slower, as well as significant",
        baseline.min_relative_regression * 100.0
    );
    println!(
        "  machine     {} / {} / {} ({} threads)",
        machine.cpu, machine.arch, machine.simd, machine.threads
    );
    if !report.comparable {
        println!(
            "  WARNING     the baseline was measured on a different machine: {}",
            report.differences.join("; ")
        );
    }
    println!();

    for verdict in &report.verdicts {
        match verdict {
            columnar_bench::gate::FlavorVerdict::NoRegression {
                flavor,
                current_median,
                baseline_median,
                p_value,
            } => println!(
                "  ok          {flavor:<10} {:>10.3} G rows/s vs baseline {:>10.3} (p = {p_value:.4})",
                current_median / 1e9,
                baseline_median / 1e9
            ),
            columnar_bench::gate::FlavorVerdict::Regression {
                flavor,
                current_median,
                baseline_median,
                p_value,
                slower_by,
            } => println!(
                "  REGRESSION  {flavor:<10} {:>10.3} G rows/s vs baseline {:>10.3} (p = {p_value:.4}, {:.1}% slower)",
                current_median / 1e9,
                baseline_median / 1e9,
                slower_by * 100.0
            ),
            columnar_bench::gate::FlavorVerdict::WithinNoise {
                flavor,
                current_median,
                baseline_median,
                p_value,
                slower_by,
            } => println!(
                "  within noise {flavor:<10} {:>10.3} G rows/s vs baseline {:>10.3} (p = {p_value:.4}, {:.1}% slower -- under the {:.1}% threshold)",
                current_median / 1e9,
                baseline_median / 1e9,
                slower_by * 100.0,
                baseline.min_relative_regression * 100.0
            ),
            columnar_bench::gate::FlavorVerdict::Inconclusive { flavor, reason } => {
                println!("  INCONCLUSIVE {flavor:<10} {reason}");
            }
            columnar_bench::gate::FlavorVerdict::BudgetMiss {
                flavor,
                current_median,
                budget,
            } => println!(
                "  BUDGET MISS {flavor:<10} {:>10.3} G rows/s is below the budget of {:>10.3}",
                current_median / 1e9,
                budget / 1e9
            ),
        }
    }

    if !report.inconclusive().is_empty() {
        println!();
        println!("the gate could not decide for: {}", report.inconclusive().join(", "));
    }
    if !report.within_noise().is_empty() {
        println!();
        println!(
            "statistically different, but within the noise floor: {}",
            report.within_noise().join(", ")
        );
    }
}
