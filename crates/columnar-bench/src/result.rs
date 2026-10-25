//! The machine-readable benchmark report.
//!
//! Every measurement this repository publishes travels as one of these
//! structures, written as JSON. Prose numbers are hand-copied and drift; a
//! JSON artifact is diffable, and the machine spec travels with the number
//! instead of being left in a README paragraph where it goes stale.

// The byte count is converted to `f64` for the MiB display; a byte count
// beyond 2^53 is not a number this report can reach.
#![allow(clippy::cast_precision_loss)]

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::external::ExternalComparison;
use crate::kernel_ab::KernelAb;
use crate::machine::MachineSpec;

/// What was measured, and under what conditions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchReport {
    /// The machine the measurement was taken on.
    pub machine: MachineSpec,
    /// The dataset the measurement used.
    pub dataset: DatasetReport,
    /// The kernel A/B.
    pub kernel_ab: KernelAb,
    /// The external comparison, when one was run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<Vec<ExternalComparison>>,
    /// The engine's own timings for the same queries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_queries: Option<Vec<EngineQueryResult>>,
}

/// The dataset a measurement was taken on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatasetReport {
    /// The Parquet file that was read.
    pub path: String,
    /// The number of rows in it.
    pub rows: u64,
    /// Its size in bytes.
    pub bytes: u64,
    /// The scale factor it was generated at, if the manifest says so.
    ///
    /// `None` when the dataset has no manifest next to it. A report that
    /// cannot name the conditions the data was generated under is not a
    /// complete report, and guessing "scale factor 0" would be worse than
    /// saying nothing.
    #[serde(default)]
    pub scale_factor: Option<u32>,
    /// The seed it was generated from, if the manifest says so.
    #[serde(default)]
    pub seed: Option<u64>,
    /// The null rate it was generated with, if the manifest says so.
    #[serde(default)]
    pub null_rate: Option<f64>,
}

/// The engine's own result for one query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineQueryResult {
    /// The query's name.
    pub query: String,
    /// Which way the engine read the file: `eager` or `lazy`.
    ///
    /// The eager path decodes every column of the file before the query
    /// runs. The lazy path reads only the columns the plan needs, with the
    /// plan's predicates pushed into the reader.
    pub mode: String,
    /// The SQL the query corresponds to, for reference.
    pub sql: String,
    /// How long the query took, in milliseconds.
    ///
    /// For the **eager** path this is the query against data that is already
    /// in memory, which is the like-for-like number against the incumbents'
    /// resident measurements.
    ///
    /// For the **lazy** path there is no resident form -- the file is read
    /// during the query -- so this is the whole lazy query: open the file,
    /// read the columns the plan needs, and answer.
    pub elapsed_ms: f64,
    /// How long the Parquet file took to become queryable, in milliseconds.
    ///
    /// For the eager path this is the full decode. For the lazy path it is
    /// the open: reading the footer, which is all the lazy path does before
    /// the query starts.
    pub load_ms: f64,
    /// How long a cold run took end to end: read the file, then answer.
    ///
    /// This is the like-for-like number against the incumbents' cold
    /// measurements.
    pub cold_ms: f64,
    /// The value the engine computed, trimmed.
    pub result: String,
    /// The numeric value the engine computed, when it computed one.
    ///
    /// Cross-checked against each incumbent's own answer: a comparison where
    /// the two sides computed different values is not a benchmark.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_value: Option<f64>,
    /// The plan output, as the engine rendered it.
    pub plan_output: String,
}

impl BenchReport {
    /// Writes the report as pretty-printed JSON.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be created or written.
    pub fn write_json(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let json = serde_json::to_string_pretty(self).map_err(|error| {
            std::io::Error::other(format!("could not serialise the report: {error}"))
        })?;
        std::fs::write(path, format!("{json}\n"))
    }

    /// Renders the report as a human-readable summary.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("COLUMNAR ENGINE \u{b7} BENCHMARK REPORT\n");
        writeln!(
            out,
            "machine   {} \u{b7} {} \u{b7} {} cores \u{b7} SIMD {} \u{b7} {} threads",
            self.machine.cpu,
            self.machine.arch,
            self.machine.cores,
            self.machine.simd,
            self.machine.threads
        )
        .ok();
        writeln!(out, "toolchain {}", self.machine.rustc).ok();
        writeln!(out, "os        {}", self.machine.os).ok();
        writeln!(
            out,
            "dataset   {} \u{b7} {} rows \u{b7} {:.1} MiB \u{b7} sf {} \u{b7} seed {}",
            self.dataset.path,
            self.dataset.rows,
            self.dataset.bytes as f64 / (1024.0 * 1024.0),
            self.dataset.scale_factor.map_or_else(|| String::from("unknown"), |sf| sf.to_string()),
            self.dataset.seed.map_or_else(|| String::from("unknown"), |seed| seed.to_string())
        )
        .ok();
        writeln!(
            out,
            "\nkernel A/B \u{b7} {} rows \u{b7} {} iterations \u{b7} column {} {} {}",
            self.kernel_ab.rows,
            self.kernel_ab.iterations,
            self.kernel_ab.column,
            self.kernel_ab.operator,
            self.kernel_ab.threshold
        )
        .ok();
        writeln!(
            out,
            "scalar = compiled with -fno-vectorize \u{b7} cpp-auto = the same source with the vectoriser on"
        )
        .ok();
        writeln!(out, "flavor      rows/sec      ns/row   vs scalar  vs cpp-auto   matches").ok();
        writeln!(out, "---------------------------------------------------------------").ok();
        for flavor in &self.kernel_ab.flavors {
            writeln!(
                out,
                "{:<10}  {:>9.3} G  {:>10.3}   {:>6.2}x   {:>8.2}x   {:>9}",
                flavor.flavor,
                flavor.rows_per_sec / 1e9,
                flavor.ns_per_row,
                flavor.ratio_vs_scalar,
                flavor.ratio_vs_auto,
                flavor.matches
            )
            .ok();
        }

        if let Some(engine) = &self.engine_queries {
            let external = self.external.as_deref().unwrap_or(&[]);
            render_resident(&mut out, engine, external);
            render_cold(&mut out, engine, external);
            render_load(&mut out, engine, external);
        }
        out
    }
}
/// Renders the resident comparison: the data is already in memory for
/// every system.
///
/// This is the like-for-like table -- it asks the same question of all
/// three systems. The lazy path has no resident form and is not in this
/// table; it is in the cold one, where the file is read.
fn render_resident(
    out: &mut String,
    engine: &[EngineQueryResult],
    external: &[ExternalComparison],
) {
    writeln!(out, "\nquery comparison \u{b7} wall clock ms \u{b7} lower is better").ok();
    writeln!(out, "resident \u{b7} the data is already in memory").ok();
    writeln!(out, "query          engine   duckdb   polars   note").ok();
    writeln!(out, "--------------------------------------------------------").ok();
    for query in engine.iter().filter(|query| query.mode == "eager") {
        let duckdb = lookup(external, &query.query, "duckdb", "resident");
        let polars = lookup(external, &query.query, "polars", "resident");
        let candidates = [("engine", query.elapsed_ms), ("duckdb", duckdb), ("polars", polars)];
        let best =
            candidates.iter().filter(|(_, ms)| ms.is_finite()).min_by(|a, b| a.1.total_cmp(&b.1));
        let note = match best {
            Some((name, _)) if *name == "engine" => String::from("engine wins"),
            Some((name, ms)) => {
                // How many times slower the engine is than the winner.
                format!("{name} wins \u{b7} engine {:.2}x slower", query.elapsed_ms / ms.max(1e-9))
            }
            None => String::from("no result"),
        };
        writeln!(
            out,
            "{:<7}  {:>9.1}   {:>6.1}   {:>6.1}   {}",
            query.query, query.elapsed_ms, duckdb, polars, note
        )
        .ok();
    }
}

/// Renders the cold comparison: from the file on disk to the answer, in
/// one go. Every system pays for reading the Parquet file.
///
/// The lazy path belongs here and not in the resident table: it reads the
/// file *during* the query, so its number is a cold number by
/// construction.
fn render_cold(out: &mut String, engine: &[EngineQueryResult], external: &[ExternalComparison]) {
    writeln!(out, "\ncold \u{b7} read the Parquet file and answer in one go").ok();
    writeln!(out, "query          engine   engine-lazy   duckdb   polars").ok();
    writeln!(out, "--------------------------------------------------------").ok();
    for query in engine.iter().filter(|query| query.mode == "eager") {
        let lazy = engine
            .iter()
            .find(|other| other.mode == "lazy" && other.query == query.query)
            .map_or(f64::NAN, |other| other.elapsed_ms);
        let duckdb = lookup(external, &query.query, "duckdb", "cold");
        let polars = lookup(external, &query.query, "polars", "cold");
        writeln!(
            out,
            "{:<7}  {:>9.1}   {:>11.1}   {:>6.1}   {:>6.1}",
            query.query, query.cold_ms, lazy, duckdb, polars
        )
        .ok();
    }
}

/// Renders the load, on its own: how long does the file take to become
/// queryable? For the incumbents this is a separate statement; for the
/// eager engine it is the Parquet decode, and for the lazy engine it is the
/// open -- reading the footer, and nothing else.
fn render_load(out: &mut String, engine: &[EngineQueryResult], external: &[ExternalComparison]) {
    writeln!(out, "\nfile load \u{b7} the Parquet file becoming queryable \u{b7} ms").ok();
    writeln!(out, "system        load ms   subprocess wall ms").ok();
    writeln!(out, "--------------------------------------------------------").ok();
    if let Some(query) = engine.iter().find(|query| query.mode == "eager") {
        writeln!(out, "{:<7}  {:>9.1}   {:>18}", "engine", query.load_ms, "-").ok();
    }
    if let Some(query) = engine.iter().find(|query| query.mode == "lazy") {
        writeln!(out, "{:<12}  {:>9.1}   {:>18}", "engine lazy", query.load_ms, "-").ok();
    }
    for comparator in ["duckdb", "polars"] {
        let load = worst(
            external
                .iter()
                .filter(|e| e.comparator == comparator && e.mode == "resident")
                .map(|e| e.load_ms),
        );
        let wall = worst(
            external
                .iter()
                .filter(|e| e.comparator == comparator && e.mode == "cold")
                .map(|e| e.elapsed_ms),
        );
        writeln!(out, "{comparator:<7}  {load:>9.1}   {wall:>18}").ok();
    }
}

/// The largest of a set of samples.
fn worst(samples: impl Iterator<Item = f64>) -> f64 {
    samples.fold(0.0_f64, f64::max)
}

/// Looks one incumbent's measurement up in the report.
fn lookup(external: &[ExternalComparison], query: &str, comparator: &str, mode: &str) -> f64 {
    external
        .iter()
        .find(|e| e.query == query && e.comparator == comparator && e.mode == mode)
        .map_or(f64::NAN, |e| e.query_ms)
}
