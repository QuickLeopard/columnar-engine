//! The external comparison driver: `DuckDB` and `Polars`, as separate processes.
//!
//! The rules that make this a comparison rather than two separate numbers:
//!
//! - **The same file.** Every engine reads the same Parquet file. The point
//!   is to compare engines, not datasets.
//! - **The same thread count.** The engine is single-threaded for this
//!   milestone, so both incumbents are pinned to one thread. `DuckDB` defaults
//!   to the core count; letting it would compare a one-thread engine with a
//!   fourteen-thread one.
//! - **The same harness.** The incumbent is warmed up, then timed for the
//!   same number of iterations as this engine, on the same machine.
//! - **The same answer.** The driver's output includes the value the
//!   incumbent computed, so the numbers can be cross-checked against this
//!   engine's result. A comparison where the two sides computed different
//!   answers is not a benchmark.
//!
//! ## Two numbers per incumbent, not one
//!
//! An end-to-end wall clock around a subprocess measures three things at
//! once: how long the process takes to start, how long the file takes to
//! become queryable, and how long the query takes. Those are very different
//! costs -- for `Polars`, the Python interpreter and the `polars` import
//! alone cost about 90 ms, which is more than the query.
//!
//! So each incumbent is measured in **resident mode**: the file is loaded
//! into memory first, and the two halves are reported separately.
//!
//! | | load | query |
//! |---|---|---|
//! | `DuckDB` | `CREATE TEMP TABLE ... AS SELECT * FROM '<file>'` | the query, against the table |
//! | `Polars` | `pl.read_parquet` | the query, against the frame |
//! | this engine | `Dataset::read_parquet` | the query, against the column chunks |
//!
//! All three now answer the same question -- *given the data in memory, how
//! long does the query take?* -- and the load cost is published next to it
//! rather than hidden inside it.
//!
//! The end-to-end wall clock is kept as well, because it is what a user
//! waiting on a command line actually pays.
//!
//! ## How each incumbent is invoked
//!
//! **`DuckDB`** is driven through its official command line client, which reads
//! a Parquet file from SQL with a quoted path. The thread count is pinned in
//! the same session as the query, before the query runs, so the plan is built
//! with the same parallelism the measurement uses. `DuckDB`'s own `.timer`
//! reports the time of each statement, which is how the load and the query
//! are told apart.
//!
//! **`Polars`** ships as a library rather than as a client that can open a
//! file, so it is driven through `tools/polars-run.py` -- a script checked into
//! this repository, so the comparison is reproducible rather than being an
//! ad-hoc line typed at a terminal. It runs the *same SQL* through
//! `pl.SQLContext`, with the thread count pinned through `POLARS_MAX_THREADS`
//! and the timings printed as machine-readable markers. The `Polars` version
//! is recorded in the report.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

/// An incumbent to compare against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comparator {
    /// The `DuckDB` command line client.
    DuckDb,
    /// `Polars`, through `tools/polars-run.py`.
    Polars,
    /// `DataFusion`, through `tools/datafusion-peer`.
    ///
    /// The peer is a **Rust** engine reading the same Parquet through the
    /// same Arrow stack this engine's reader uses. It is a separate project
    /// under `tools/`, built separately: it is an instrument, not a
    /// dependency, and the engine's dependency graph does not move because of
    /// it.
    DataFusion,
}

impl Comparator {
    /// A short, stable name for the report.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::DuckDb => "duckdb",
            Self::Polars => "polars",
            Self::DataFusion => "datafusion-peer",
        }
    }

    /// The environment variable that overrides the binary's location.
    #[must_use]
    pub const fn env_var(self) -> &'static str {
        match self {
            Self::DuckDb => "DUCKDB_BIN",
            Self::Polars => "POLARS_BIN",
            Self::DataFusion => "DATAFUSION_PEER_BIN",
        }
    }
}

/// A query to run against an incumbent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalQuery {
    /// The query's name, e.g. `Q6`.
    pub name: String,
    /// The SQL to run.
    pub sql: String,
}

impl ExternalQuery {
    /// Builds a query.
    #[must_use]
    pub fn new(name: impl Into<String>, sql: impl Into<String>) -> Self {
        Self { name: name.into(), sql: sql.into() }
    }
}

/// Whether the incumbent is measured cold or resident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// **Cold**: one query against the file, starting from nothing.
    ///
    /// The file is read as part of the query. There is no separate load
    /// phase, so the load time is zero and the query time covers the read.
    /// This is what a user pays for a one-shot command.
    Cold,
    /// **Resident**: the file is loaded into memory first, then the query runs
    /// against the in-memory data.
    ///
    /// The load and the query are timed separately. This is the like-for-like
    /// comparison against the engine, whose own dataset is loaded once and
    /// then queried.
    Resident,
}

impl Mode {
    /// A short, stable name for the report.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Cold => "cold",
            Self::Resident => "resident",
        }
    }
}

/// One incumbent's result for one query.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ExternalComparison {
    /// The query's name.
    pub query: String,
    /// Which incumbent produced this.
    pub comparator: String,
    /// Whether this was the cold or the resident measurement.
    pub mode: String,
    /// The incumbent's version string.
    pub version: String,
    /// How long the file took to become queryable, in milliseconds.
    ///
    /// `DuckDB`: `CREATE TEMP TABLE ... AS SELECT * FROM '<file>'`.
    /// `Polars`: `pl.read_parquet`.
    ///
    /// Zero in cold mode: there is no separate load phase there.
    pub load_ms: f64,
    /// How long the query took against data already in memory, in
    /// milliseconds.
    pub query_ms: f64,
    /// The end-to-end wall clock of the subprocess, in milliseconds.
    ///
    /// This includes the process start-up, which for `Polars` is dominated by
    /// the Python interpreter and the `polars` import.
    pub elapsed_ms: f64,
    /// The value the incumbent computed, trimmed.
    pub result: String,
    /// The numeric value the incumbent computed, when it computed one.
    ///
    /// This is what gets cross-checked against the engine's own answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_value: Option<f64>,
    /// How many timed runs were made.
    pub iterations: usize,
}

/// The outcome of running one incumbent.
#[derive(Debug, Clone, PartialEq)]
pub struct ExternalOutcome {
    /// How long the file took to become queryable, in milliseconds.
    pub load_ms: f64,
    /// How long the query took against data already in memory, in
    /// milliseconds.
    pub query_ms: f64,
    /// The end-to-end wall clock of the subprocess, in milliseconds.
    pub elapsed_ms: f64,
    /// The value the incumbent computed, trimmed.
    pub result: String,
    /// The numeric value the incumbent computed, when it computed one.
    pub result_value: Option<f64>,
    /// The incumbent's version string.
    pub version: String,
}

/// Errors from the external driver.
#[derive(Debug, thiserror::Error)]
pub enum ExternalError {
    /// The comparator could not be located.
    #[error("could not find {0}; set {1} or put the binary in tools/bin")]
    NotFound(&'static str, &'static str),
    /// The comparator exited with a failure.
    #[error("{comparator} failed: {stderr}")]
    Failed {
        /// Which comparator failed.
        comparator: &'static str,
        /// Its standard error.
        stderr: String,
    },
    /// The driver could not start the comparator.
    #[error("could not start {comparator}: {source}")]
    Spawn {
        /// Which comparator failed to start.
        comparator: &'static str,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
}

/// Locates a comparator's binary.
///
/// The order is: the environment variable, then `tools/bin` next to the
/// repository root, then `PATH`.
#[must_use]
pub fn find_binary(comparator: Comparator) -> Option<PathBuf> {
    if let Ok(path) = std::env::var(comparator.env_var()) {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }

    if let Ok(root) = repository_root() {
        let candidate = root.join("tools").join("bin").join(comparator.name());
        if candidate.exists() {
            return Some(candidate);
        }
    }

    which(comparator.name())
}

/// The repository root, inferred from the current working directory.
fn repository_root() -> Result<PathBuf, std::io::Error> {
    let cwd = std::env::current_dir()?;
    // Walk up until a directory containing `Cargo.toml` and `crates/` is
    // found. The benchmark may be run from the repository root or from a
    // crate directory.
    let mut path = cwd.as_path();
    loop {
        if path.join("Cargo.toml").exists() && path.join("crates").is_dir() {
            return Ok(path.to_path_buf());
        }
        match path.parent() {
            Some(parent) => path = parent,
            None => return Err(std::io::Error::other("not inside a repository")),
        }
    }
}

/// Looks a program up on `PATH`.
fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(program);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Runs a query against an incumbent and returns its median time.
///
/// The incumbent is run once to warm it up, then `iterations` times under
/// measurement. The **median** is reported rather than the mean, because a
/// single scheduling hiccup on a shared machine should not move the number.
pub fn run(
    comparator: Comparator,
    mode: Mode,
    query: &ExternalQuery,
    data_path: &Path,
    threads: usize,
    iterations: usize,
) -> Result<ExternalOutcome, ExternalError> {
    let version = version_of(comparator, data_path);

    // Warm-up: the first invocation pays for process start-up, the file cache
    // and the query planner's first look at the file.
    run_once(comparator, mode, query, data_path, threads)?;

    let iterations = iterations.max(1);
    let mut load_samples = Vec::with_capacity(iterations);
    let mut query_samples = Vec::with_capacity(iterations);
    let mut elapsed_samples = Vec::with_capacity(iterations);
    let mut result = String::new();
    let mut result_value = None;

    for _ in 0..iterations {
        let (elapsed, output) = run_once(comparator, mode, query, data_path, threads)?;
        let parsed = parse_output(comparator, &output);
        load_samples.push(parsed.load_ms);
        query_samples.push(parsed.query_ms);
        elapsed_samples.push(elapsed);
        result = parsed.result;
        result_value = parsed.result_value;
    }

    Ok(ExternalOutcome {
        load_ms: median(&load_samples),
        query_ms: median(&query_samples),
        elapsed_ms: median(&elapsed_samples),
        result,
        result_value,
        version,
    })
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

/// What an incumbent's output says.
#[derive(Debug, Clone, PartialEq)]
struct ParsedOutput {
    /// How long the load statement took.
    load_ms: f64,
    /// How long the query took.
    query_ms: f64,
    /// The answer, with the timing lines removed.
    result: String,
    /// The numeric answer, when there is one.
    result_value: Option<f64>,
}

/// Pulls the timings and the answer out of an incumbent's output.
fn parse_output(comparator: Comparator, output: &str) -> ParsedOutput {
    match comparator {
        Comparator::DuckDb => parse_duckdb(output),
        Comparator::Polars | Comparator::DataFusion => parse_markers(output),
    }
}

/// Parses `DuckDB`'s output.
///
/// `DuckDB`'s `.timer` prints one `Run Time (s): real <seconds> ...` line per
/// timed statement. In resident mode exactly two statements are timed: the
/// `CREATE TEMP TABLE` and the query, in that order.
fn parse_duckdb(output: &str) -> ParsedOutput {
    let mut real_seconds = Vec::new();
    let mut answer_lines = Vec::new();

    for line in output.lines() {
        let line = strip_ansi(line);
        if let Some(rest) = line.trim().strip_prefix("Run Time (s): real ") {
            if let Some(value) = rest.split_whitespace().next() {
                if let Ok(seconds) = value.parse::<f64>() {
                    real_seconds.push(seconds * 1000.0);
                }
            }
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        answer_lines.push(line.clone());
    }

    // The last timed statement is the query; the one before it is the load.
    let query_ms = real_seconds.last().copied().unwrap_or(0.0);
    let load_ms = if real_seconds.len() >= 2 { real_seconds[real_seconds.len() - 2] } else { 0.0 };

    // In CSV mode `DuckDB` prints a header line and then the value. The
    // answer is the last line that parses as a number, which is robust to
    // the header being there or not.
    let result = answer_lines.join("\n");
    let result_value = answer_lines.iter().rev().find_map(|line| line.trim().parse::<f64>().ok());

    ParsedOutput { load_ms, query_ms, result, result_value }
}

/// Parses the output of a driver that prints its timings as markers.
///
/// The `Polars` driver and the `DataFusion` peer both print `LOAD_MS=`,
/// `QUERY_MS=` and `VALUE=` markers ahead of the result, and both are parsed
/// the same way: the marker protocol is the contract between a driver and
/// this harness, so a new driver is a new binary and not a new parser.
fn parse_markers(output: &str) -> ParsedOutput {
    let mut load_ms = 0.0;
    let mut query_ms = 0.0;
    let mut value = None;
    let mut answer_lines = Vec::new();

    for line in output.lines() {
        let line = strip_ansi(line);
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("LOAD_MS=") {
            load_ms = rest.trim().parse::<f64>().unwrap_or(0.0);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("QUERY_MS=") {
            query_ms = rest.trim().parse::<f64>().unwrap_or(0.0);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("VALUE=") {
            let rest = rest.trim();
            if !rest.is_empty() {
                value = rest.parse::<f64>().ok();
            }
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        answer_lines.push(line.clone());
    }

    ParsedOutput { load_ms, query_ms, result: answer_lines.join("\n"), result_value: value }
}

/// Removes ANSI escape sequences from a line.
///
/// `DuckDB`'s command line client draws its tables with box-drawing
/// characters wrapped in escape sequences, and it prefixes even its timing
/// lines with a colour reset. They are noise in a report that is meant to be
/// read -- and, worse, a half-stripped sequence leaves `00m` glued to the
/// front of the line, which is enough to stop a line from being recognised
/// as a timing line at all.
///
/// Only the CSI form (`ESC [ ... final`) is handled, because that is the only
/// form `DuckDB` emits.
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        // ESC '[' introduces a control sequence; its final byte is the first
        // one in 0x40..=0x7e. Note that '[' is itself in that range, so it
        // has to be consumed explicitly -- treating it as the final byte
        // would leave the rest of the sequence behind.
        if chars.next() == Some('[') {
            for next in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&next) {
                    break;
                }
            }
        }
    }
    out
}

/// The program to run, its arguments, and the environment it runs in.
type Invocation = (PathBuf, Vec<String>, Vec<(String, String)>);

/// The command to run, and the environment it runs in.
fn invocation(
    comparator: Comparator,
    mode: Mode,
    query: &ExternalQuery,
    data_path: &Path,
    threads: usize,
) -> Result<Invocation, ExternalError> {
    match comparator {
        Comparator::DuckDb => {
            let binary = find_binary(comparator)
                .ok_or(ExternalError::NotFound(comparator.name(), comparator.env_var()))?;
            let mut args = vec![
                // CSV mode, so the answer is one bare value rather than a
                // box-drawn table.
                "-csv".to_owned(),
                "-c".to_owned(),
                // Pinned before the query runs, in the same session, so the
                // plan is built with the same thread count the measurement
                // uses.
                format!("SET threads TO {threads};"),
                "-c".to_owned(),
                "SET enable_object_cache=false;".to_owned(),
                "-c".to_owned(),
                ".timer on".to_owned(),
            ];
            match mode {
                // Cold: the query reads the Parquet file directly.
                Mode::Cold => {
                    args.push("-c".to_owned());
                    args.push(query.sql.clone());
                }
                // Resident: the file is loaded into a table first, so the
                // load and the query are timed separately. Only these two
                // statements are timed, and the driver tells them apart by
                // their order.
                Mode::Resident => {
                    args.push("-c".to_owned());
                    args.push(format!(
                        "CREATE TEMP TABLE lineitem AS SELECT * FROM '{}';",
                        data_path.display()
                    ));
                    args.push("-c".to_owned());
                    args.push(query.sql.clone());
                }
            }
            Ok((binary, args, Vec::new()))
        }
        Comparator::DataFusion => {
            let binary = find_binary(comparator)
                .ok_or(ExternalError::NotFound(comparator.name(), comparator.env_var()))?;
            let mut args = vec![
                "--sql".to_owned(),
                query.sql.clone(),
                "--path".to_owned(),
                data_path.display().to_string(),
                // The thread count is pinned, so the peer's runtime and its
                // partition count are both the engine's.
                "--threads".to_owned(),
                threads.to_string(),
            ];
            if mode == Mode::Resident {
                args.push("--resident".to_owned());
            }
            Ok((binary, args, Vec::new()))
        }
        Comparator::Polars => {
            let script = repository_root().map_or_else(
                |_| PathBuf::from("tools/polars-run.py"),
                |root| root.join("tools").join("polars-run.py"),
            );
            let python = std::env::var("PYTHON_BIN").unwrap_or_else(|_| "python3".to_owned());
            let mut args = vec![
                script.display().to_string(),
                query.sql.clone(),
                data_path.display().to_string(),
            ];
            if mode == Mode::Resident {
                // Resident mode: load the file into a frame first, so the
                // load and the query can be timed separately, the way the
                // engine's own two phases are timed separately.
                args.push("--resident".to_owned());
            }
            // Polars reads its thread cap from the environment.
            let env = vec![("POLARS_MAX_THREADS".to_owned(), threads.to_string())];
            Ok((PathBuf::from(python), args, env))
        }
    }
}

/// Runs the query once and returns (elapsed ms, trimmed output).
fn run_once(
    comparator: Comparator,
    mode: Mode,
    query: &ExternalQuery,
    data_path: &Path,
    threads: usize,
) -> Result<(f64, String), ExternalError> {
    let (program, args, env) = invocation(comparator, mode, query, data_path, threads)?;

    let mut command = Command::new(&program);
    command.args(&args);
    for (key, value) in env {
        command.env(key, value);
    }

    let start = Instant::now();
    let output = command
        .output()
        .map_err(|source| ExternalError::Spawn { comparator: comparator.name(), source })?;
    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;

    if !output.status.success() {
        return Err(ExternalError::Failed {
            comparator: comparator.name(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }

    Ok((elapsed_ms, String::from_utf8_lossy(&output.stdout).trim().to_owned()))
}

/// The comparator's version string.
fn version_of(comparator: Comparator, data_path: &Path) -> String {
    match comparator {
        Comparator::DuckDb => {
            let Some(binary) = find_binary(comparator) else {
                return "unknown".to_owned();
            };
            Command::new(binary)
                .arg("--version")
                .output()
                .ok()
                .filter(|output| output.status.success())
                .map_or_else(
                    || "unknown".to_owned(),
                    |output| String::from_utf8_lossy(&output.stdout).trim().to_owned(),
                )
        }
        Comparator::DataFusion => {
            // The peer prints its own version, and the version matters: a
            // number measured against DataFusion 49 is not the same claim as
            // one measured against DataFusion 50.
            let Some(binary) = find_binary(comparator) else {
                return "unknown".to_owned();
            };
            Command::new(binary)
                .arg("--version")
                .output()
                .ok()
                .filter(|output| output.status.success())
                .map_or_else(
                    || "unknown".to_owned(),
                    |output| String::from_utf8_lossy(&output.stdout).trim().to_owned(),
                )
        }
        Comparator::Polars => {
            // The Python driver knows the Polars version; ask it directly.
            let script = repository_root().map_or_else(
                |_| PathBuf::from("tools/polars-version.py"),
                |root| root.join("tools").join("polars-version.py"),
            );
            let python = std::env::var("PYTHON_BIN").unwrap_or_else(|_| "python3".to_owned());
            Command::new(python)
                .arg(script)
                .arg(data_path)
                .output()
                .ok()
                .filter(|output| output.status.success())
                .map_or_else(
                    || "unknown".to_owned(),
                    |output| String::from_utf8_lossy(&output.stdout).trim().to_owned(),
                )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_duckdb, parse_markers, strip_ansi};

    /// `DuckDB` prefixes its timing lines with an ANSI colour reset. The
    /// first version of the parser stopped at the `[` of the escape sequence,
    /// which left `00m` glued to the front of the line and made the line
    /// unrecognisable. This test pins the behaviour.
    #[test]
    fn duckdb_timings_are_found_behind_ansi_codes() {
        let output = concat!(
            "\u{1b}[90mWARNING:\n",
            "\u{1b}[00m\u{1b}[90mThe 'enable_object_cache' setting is deprecated.\n",
            "\n",
            "\u{1b}[00mRun Time (s): real 1.223 user 1.134378 sys 0.090696\n",
            "count_star()\n",
            "6000000\n",
            "Run Time (s): real 0.001 user 0.001310 sys 0.000046\n",
        );

        let parsed = parse_duckdb(output);
        // The load is the first timed statement, the query the second.
        assert!((parsed.load_ms - 1223.0).abs() < 0.001, "load was {}", parsed.load_ms);
        assert!((parsed.query_ms - 1.0).abs() < 0.001, "query was {}", parsed.query_ms);
        assert_eq!(parsed.result_value, Some(6_000_000.0));
    }

    /// In cold mode there is only one timed statement, so there is no load
    /// time to report.
    #[test]
    fn duckdb_cold_mode_reports_no_load_time() {
        let output = "Run Time (s): real 0.031 user 0.030215 sys 0.002807\ncount_star()\n315203\n";
        let parsed = parse_duckdb(output);
        assert!(parsed.load_ms.abs() < f64::EPSILON, "load was {}", parsed.load_ms);
        assert!((parsed.query_ms - 31.0).abs() < 0.001, "query was {}", parsed.query_ms);
        assert_eq!(parsed.result_value, Some(315_203.0));
    }

    /// The `Polars` driver prints its timings as machine-readable markers.
    #[test]
    fn polars_markers_are_read() {
        let output = concat!(
            "LOAD_MS=1033.600\n",
            "QUERY_MS=0.200\n",
            "VALUE=6000000\n",
            "shape: (1, 1)\n",
            "6000000\n",
        );
        let parsed = parse_markers(output);
        assert!((parsed.load_ms - 1033.6).abs() < 0.001, "load was {}", parsed.load_ms);
        assert!((parsed.query_ms - 0.2).abs() < 0.001, "query was {}", parsed.query_ms);
        assert_eq!(parsed.result_value, Some(6_000_000.0));
        assert!(!parsed.result.contains("LOAD_MS"), "the markers leaked into the answer");
    }

    /// A whole CSI sequence is removed, not just its introducer.
    #[test]
    fn ansi_sequences_are_removed_whole() {
        assert_eq!(strip_ansi("\u{1b}[00mRun Time"), "Run Time");
        assert_eq!(strip_ansi("\u{1b}[1;32mgreen\u{1b}[0m"), "green");
        assert_eq!(strip_ansi("plain"), "plain");
    }
}
