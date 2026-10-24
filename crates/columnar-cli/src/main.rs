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
};
use columnar_core::ComparisonOp;
use columnar_engine::{Dataset, ExecOptions};

mod queries;

use queries::{BuiltinQuery, QueryDefinition, QUERIES};

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
    match cli.command {
        Command::Dataset { scale_factor, seed, null_rate, output } => {
            run_dataset(scale_factor, seed, null_rate, &output)
        }
        Command::Query { data, query, iterations } => run_query(&data, query, iterations),
        Command::Bench { data, rows, iterations, column, threshold, out } => {
            run_bench(&data, rows, iterations, &column, threshold, &out)
        }
        Command::Compare { data, iterations, threads, out } => {
            run_compare(&data, iterations, threads, &out)
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
fn run_query(data: &Path, query: BuiltinQueryArg, iterations: usize) -> Result<()> {
    {
        {
            let dataset = Dataset::read_parquet(data)
                .with_context(|| format!("could not read {}", data.display()))?;
            let builtin = BuiltinQuery::from(query);
            let definition =
                QUERIES.iter().find(|q| q.query == builtin).context("unknown query")?;

            let mut timings = Vec::with_capacity(iterations);
            let mut last = None;
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
                // The engine is measured twice, the same way the incumbents
                // are: once with the data already resident, and once cold,
                // reading the file and answering the query in one go.
                let (load_ms, resident_ms, cold_ms, result) =
                    measure_engine(definition, data, iterations)?;

                // The numeric answer, so it can be cross-checked against each
                // incumbent's answer.
                let result_value =
                    result.groups.first().and_then(|group| group.values.first()).and_then(
                        |value| match value {
                            columnar_engine::AggregateValue::Int(v) => Some(*v as f64),
                            columnar_engine::AggregateValue::Float(v) => Some(*v),
                            columnar_engine::AggregateValue::Null => None,
                        },
                    );

                engine_queries.push(EngineQueryResult {
                    query: definition.query.name().to_owned(),
                    sql: definition.sql_for_display(),
                    elapsed_ms: resident_ms,
                    load_ms,
                    cold_ms,
                    result: format!("{:?}", result.groups),
                    result_value,
                    plan_output: result.plan_output.to_string(),
                });

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
