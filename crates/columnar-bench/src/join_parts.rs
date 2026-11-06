//! The join-orders query, on both sides, taken apart.
//!
//! The engine's cold `join-orders` query is behind `DuckDB`'s, and the
//! question the project has carried is: **which part of the query holds the
//! difference?** The engine's own join stage was taken apart in spec 026, but
//! every part of that analysis was set against `DuckDB`'s **whole** -- and a
//! part cannot be compared with a whole.
//!
//! This module takes **both** sides apart, in **one session**, and measures
//! four arms per iteration:
//!
//! | arm | what it is | what it yields |
//! |---|---|---|
//! | **the engine** | the engine's own plan over the file, the lazy path | the stage table, the join's own parts |
//! | **`DuckDB`, profiled** | the same SQL, with `DuckDB`'s own profiler on | the operator tree's timings, the bytes read |
//! | **`DuckDB`, unprofiled** | the same SQL, the way the comparison harness runs it | the whole, with no parts |
//! | **`DuckDB`, the filter off** | the profiled arm with the join filter pushdown disabled | the parts, and the rows that reached the join |
//!
//! The fourth arm is the one that **decides**. It changes exactly one thing --
//! whether the build side's keys are pushed into the probe side's scan as a
//! filter -- and therefore names the mechanism instead of assuming it.
//!
//! # The parts, and the same names on both sides
//!
//! | part | the engine | `DuckDB` |
//! |---|---|---|
//! | the probe side's read | the scan stage | the probe side's table scan |
//! | the build side's read | the build side's whole | the build side's table scan |
//! | the build side's open | the build side's open | *no counterpart -- it is inside the scan* |
//! | the join | the probe | the hash join |
//! | the projection | the project stage | the projection |
//! | the aggregate | the aggregate stage | the ungrouped aggregate |
//!
//! **The engine's scan stage does not carry the probe's cost** on the cold
//! path: the probe runs inside the scan and its time is taken out of the
//! scan's. The report says so on every engine arm, because a reader who does
//! not know that would read the scan's figure as the whole read.
//!
//! **The engine's table's construction is charged to its build side** and
//! `DuckDB`'s is charged to its hash join. The difference is not measured here
//! and is not averaged away; the names above say where each side puts it.
//!
//! # The answer is checked first
//!
//! Both sides' answers are checked against the **published** value of the cold
//! `join-orders` query before anything is timed. A probe that measured a
//! different query would be a measurement of nothing.
//!
//! # The order alternates
//!
//! The engine's arm and `DuckDB`'s arms are measured back to back in one
//! iteration, and the **order is alternated** between the iterations: a
//! machine that drifts while an iteration runs gives the first measurement a
//! different price from the second. The iteration count is **even**, for the
//! reason spec 026 gave -- an odd count would leave the two orders' groups
//! different in size, and the median would inherit the difference.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use columnar_engine::queries::{BuiltinQuery, TableAddress, QUERIES};
use columnar_engine::{AggregateValue, ExecOptions, ParquetSource};
use serde::{Deserialize, Serialize};

use crate::external::{find_binary, Comparator};
use crate::machine::{host_load_average, MachineSpec};

/// The published value of the cold `join-orders` query.
///
/// The two sides do not print the same digits -- `DuckDB` prints
/// `11472226962.890097` and the engine `11472226962.8901` -- so the check is
/// **relative**, because floating-point addition is not associative and the
/// two sides sum the same rows in different orders.
const PUBLISHED_ANSWER: f64 = 11_472_226_962.890_1;

/// How far apart two answers may be and still be the same answer.
const ANSWER_TOLERANCE: f64 = 1e-9;

/// How the probe is run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinPartsOptions {
    /// The probe side's file.
    pub data: PathBuf,
    /// The build side's file.
    pub build: PathBuf,
    /// How many iterations a campaign runs.
    ///
    /// **Even**, so that the two orders' groups are the same size.
    pub iterations: usize,
    /// How many threads both sides use.
    pub threads: usize,
}

/// What the probe could not do.
#[derive(Debug, thiserror::Error)]
pub enum JoinPartsError {
    /// The `DuckDB` binary was not found.
    #[error("the duckdb binary was not found")]
    DuckDbMissing,
    /// A `DuckDB` run failed.
    #[error("the duckdb run failed: {stderr}")]
    DuckDbFailed {
        /// What `DuckDB` said on its error stream.
        stderr: String,
    },
    /// The profile `DuckDB` was asked to write could not be read.
    #[error("the duckdb profile at {path} could not be read: {reason}")]
    ProfileUnreadable {
        /// Where the profile was expected.
        path: PathBuf,
        /// Why it could not be read.
        reason: String,
    },
    /// The profile is not the shape this probe knows.
    #[error("the duckdb profile is not the shape this probe knows: {reason}")]
    ProfileShape {
        /// What was missing or unexpected.
        reason: String,
    },
    /// The engine's own query failed.
    #[error("the engine's query failed: {reason}")]
    Engine {
        /// What the engine said.
        reason: String,
    },
    /// A side answered something other than the published answer.
    #[error("the {side} side answered {answer}, and the published answer is {expected}")]
    Answer {
        /// Which side answered.
        side: String,
        /// What it answered.
        answer: f64,
        /// What the published answer is.
        expected: f64,
    },
    /// The report could not be written.
    #[error("the report could not be written: {reason}")]
    Write {
        /// Why it could not be written.
        reason: String,
    },
}

/// One side's parts, for one iteration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JoinParts {
    /// The probe side's read: the scan, in milliseconds.
    pub probe_side_read_ms: f64,
    /// The build side's read, in milliseconds.
    pub build_side_read_ms: f64,
    /// The build side's file open, in milliseconds.
    ///
    /// The engine's open is its own row because it is its own clock; on
    /// `DuckDB`'s side the open is inside the scan, and there is no separate
    /// figure for it.
    pub build_side_open_ms: f64,
    /// The join, in milliseconds.
    pub join_ms: f64,
    /// The projection, in milliseconds.
    pub projection_ms: f64,
    /// The aggregate, in milliseconds.
    pub aggregate_ms: f64,
    /// How many rows reached the join.
    pub rows_to_join: u64,
    /// How many rows the join produced.
    pub rows_out: u64,
    /// How many bytes the side read, when the side reports them.
    pub bytes_read: Option<u64>,
    /// The whole less the parts, in milliseconds.
    ///
    /// The engine's plan output names every stage of its query, so this is
    /// almost nothing. `DuckDB`'s whole carries its **planning** -- the bind,
    /// the optimiser and the physical planner -- which is inside its statement
    /// and outside its operator tree, so this is where it shows up.
    pub outside_parts_ms: f64,
}

/// One side's measurement, for one iteration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JoinArm {
    /// Which side this is: `engine`, `duckdb`, `duckdb-unprofiled` or
    /// `duckdb-filter-off`.
    pub side: String,
    /// The whole query, in milliseconds.
    pub total_ms: f64,
    /// The parts, when the side reports them.
    pub parts: Option<JoinParts>,
    /// The answer the side computed.
    pub answer: f64,
    /// The columns the side's scan read.
    pub columns_read: Vec<String>,
    /// The predicates the side pushed into its scan.
    pub filters: Vec<String>,
    /// Whether the probe ran **inside** the scan.
    ///
    /// True on the engine's cold path, and it matters: the scan's figure does
    /// **not** carry the probe's cost there, so the scan's figure and the
    /// join's figure are two different parts of the query.
    pub probe_in_scan: Option<bool>,
    /// How long the side spent planning, when the side reports it.
    pub planning_ms: Option<f64>,
    /// How much CPU the side's whole query took, when the side reports it.
    pub cpu_ms: Option<f64>,
}

/// One iteration: every arm, in the order they ran.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JoinIteration {
    /// The iteration's number, from zero.
    pub index: usize,
    /// Which side ran first: `engine` or `duckdb`.
    pub order: String,
    /// The host's load average when the iteration started.
    pub load_average: f64,
    /// The arms, in the order they ran.
    pub arms: Vec<JoinArm>,
}

/// The whole campaign's report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JoinPartsReport {
    /// The probe's name.
    pub probe: String,
    /// The machine the measurement was taken on.
    pub machine: MachineSpec,
    /// The `DuckDB` version that was measured.
    pub duckdb_version: String,
    /// The code the measurement was taken on.
    pub code: String,
    /// The answer both sides had to produce.
    pub expected_answer: f64,
    /// The iterations, in the order they ran.
    pub iterations: Vec<JoinIteration>,
}

/// Measures the join-orders query on both sides, in one session.
///
/// # Errors
///
/// Returns an error when the `DuckDB` binary is missing, when a run fails, when
/// a profile is not the shape this probe knows, when the engine's own query
/// fails, or when a side answers something other than the published answer.
pub fn run(options: &JoinPartsOptions) -> Result<JoinPartsReport, JoinPartsError> {
    let binary = find_binary(Comparator::DuckDb).ok_or(JoinPartsError::DuckDbMissing)?;
    let version = duckdb_version(&binary);
    let code = current_code();

    let definition = QUERIES
        .iter()
        .find(|definition| definition.query == BuiltinQuery::JoinOrders)
        .ok_or_else(|| JoinPartsError::ProfileShape {
            reason: "the join-orders query is not in the engine's table".to_owned(),
        })?;
    let sql =
        definition.sql_for(TableAddress::File(&options.data), TableAddress::File(&options.build));

    // The answer is checked before anything is timed: a probe that measured a
    // different query would be a measurement of nothing.
    let warm = engine_arm(options, definition.plan)?;
    check_answer("engine", warm.answer)?;
    let duck = duckdb_arm(&binary, options, &sql, DuckMode::Profiled)?;
    check_answer("duckdb", duck.answer)?;

    let iterations = options.iterations.max(2);
    let mut measured = Vec::with_capacity(iterations);
    for index in 0..iterations {
        let order = if index % 2 == 0 { "engine" } else { "duckdb" };
        let load_average = host_load_average();
        let arms = if order == "engine" {
            vec![
                engine_arm(options, definition.plan)?,
                duckdb_arm(&binary, options, &sql, DuckMode::Profiled)?,
                duckdb_arm(&binary, options, &sql, DuckMode::Unprofiled)?,
                duckdb_arm(&binary, options, &sql, DuckMode::FilterOff)?,
            ]
        } else {
            vec![
                duckdb_arm(&binary, options, &sql, DuckMode::Profiled)?,
                duckdb_arm(&binary, options, &sql, DuckMode::Unprofiled)?,
                duckdb_arm(&binary, options, &sql, DuckMode::FilterOff)?,
                engine_arm(options, definition.plan)?,
            ]
        };
        measured.push(JoinIteration { index, order: order.to_owned(), load_average, arms });
    }

    Ok(JoinPartsReport {
        probe: "join-parts".to_owned(),
        machine: MachineSpec::capture(options.threads),
        duckdb_version: version,
        code,
        expected_answer: PUBLISHED_ANSWER,
        iterations: measured,
    })
}

/// Writes the report to a file.
///
/// # Errors
///
/// Returns an error when the file cannot be written.
pub fn write_report(report: &JoinPartsReport, path: &Path) -> Result<(), JoinPartsError> {
    let text = serde_json::to_string_pretty(report)
        .map_err(|error| JoinPartsError::Write { reason: error.to_string() })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| JoinPartsError::Write { reason: error.to_string() })?;
    }
    std::fs::write(path, format!("{text}\n"))
        .map_err(|error| JoinPartsError::Write { reason: error.to_string() })
}

/// How `DuckDB` is run in one arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DuckMode {
    /// The profiler is on: the parts come from `DuckDB`'s own operator tree.
    Profiled,
    /// The profiler is off: the whole, the way the comparison harness takes it.
    Unprofiled,
    /// The profiler is on and the **join filter pushdown** is disabled.
    FilterOff,
}

/// Measures the engine's side.
fn engine_arm(
    options: &JoinPartsOptions,
    plan: fn(&Path) -> columnar_engine::QueryPlan,
) -> Result<JoinArm, JoinPartsError> {
    // The probe side's file is opened **outside** the clock, the way the
    // comparison harness opens it: the open is the reader's, and the query is
    // the query.
    let source = ParquetSource::open(&options.data).map_err(|error| JoinPartsError::Engine {
        reason: format!("could not open {}: {error}", options.data.display()),
    })?;
    let exec = ExecOptions { threads: Some(options.threads.max(1)), ..ExecOptions::default() };
    let started = Instant::now();
    let result = columnar_engine::execute_on(&plan(&options.build), &source, &exec)
        .map_err(|error| JoinPartsError::Engine { reason: error.to_string() })?;
    let total_ms = started.elapsed().as_secs_f64() * 1000.0;

    let output = &result.plan_output;
    let join = output.join.as_ref();
    let stage = |name: &str| {
        result
            .stages
            .iter()
            .find(|stage| stage.stage == name)
            .map_or(0.0, |stage| stage.elapsed.as_secs_f64() * 1000.0)
    };
    let rows_of = |name: &str| {
        result.stages.iter().find(|stage| stage.stage == name).map_or(0, |stage| stage.rows_in)
    };

    let parts = JoinParts {
        probe_side_read_ms: stage("scan"),
        build_side_read_ms: join.map_or(0.0, |join| ms(join.build_elapsed)),
        build_side_open_ms: join.map_or(0.0, |join| ms(join.open_elapsed)),
        join_ms: join.map_or(0.0, |join| ms(join.probe_elapsed)),
        projection_ms: stage("project"),
        aggregate_ms: stage("aggregate"),
        rows_to_join: rows_of("join"),
        rows_out: join.map_or(0, |join| join.output_rows as u64),
        bytes_read: None,
        outside_parts_ms: 0.0,
    };
    let mut parts = parts;
    parts.outside_parts_ms = total_ms
        - (parts.probe_side_read_ms
            + parts.build_side_read_ms
            + parts.build_side_open_ms
            + parts.join_ms
            + parts.projection_ms
            + parts.aggregate_ms);

    Ok(JoinArm {
        side: "engine".to_owned(),
        total_ms,
        parts: Some(parts),
        answer: answer_of(&result),
        columns_read: output.columns_read.clone(),
        filters: output.pushed_down.clone(),
        probe_in_scan: join.map(|join| join.probe_in_scan),
        planning_ms: None,
        cpu_ms: None,
    })
}

/// Measures `DuckDB`'s side.
fn duckdb_arm(
    binary: &Path,
    options: &JoinPartsOptions,
    sql: &str,
    mode: DuckMode,
) -> Result<JoinArm, JoinPartsError> {
    let profile_path = profile_path(mode);
    let mut args = vec![
        "-csv".to_owned(),
        "-c".to_owned(),
        format!("SET threads TO {};", options.threads.max(1)),
        "-c".to_owned(),
        "SET enable_object_cache=false;".to_owned(),
    ];
    if mode == DuckMode::Unprofiled {
        args.push("-c".to_owned());
        args.push(".timer on".to_owned());
    } else {
        args.push("-c".to_owned());
        args.push("PRAGMA enable_profiling='json';".to_owned());
        args.push("-c".to_owned());
        args.push(format!("PRAGMA profiling_output='{}';", profile_path.display()));
        if mode == DuckMode::FilterOff {
            // The one thing this arm changes: the build side's keys are **not**
            // pushed into the probe side's scan as a filter, so the probe
            // side's scan hands the join every row it read.
            args.push("-c".to_owned());
            args.push("SET disabled_optimizers='join_filter_pushdown';".to_owned());
        }
    }
    args.push("-c".to_owned());
    args.push(sql.to_owned());

    let output = Command::new(binary)
        .args(&args)
        .output()
        .map_err(|error| JoinPartsError::DuckDbFailed { stderr: error.to_string() })?;
    if !output.status.success() {
        return Err(JoinPartsError::DuckDbFailed {
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();

    let answer = duckdb_answer(&stdout)?;
    let side = match mode {
        DuckMode::Profiled => "duckdb",
        DuckMode::Unprofiled => "duckdb-unprofiled",
        DuckMode::FilterOff => "duckdb-filter-off",
    };

    if mode == DuckMode::Unprofiled {
        let _ = std::fs::remove_file(&profile_path);
        return Ok(JoinArm {
            side: side.to_owned(),
            total_ms: duckdb_timer_ms(&stdout),
            parts: None,
            answer,
            columns_read: Vec::new(),
            filters: Vec::new(),
            probe_in_scan: None,
            planning_ms: None,
            cpu_ms: None,
        });
    }

    let profile = read_profile(&profile_path)?;
    let _ = std::fs::remove_file(&profile_path);

    let mut parts = JoinParts {
        probe_side_read_ms: profile.probe_side_read_ms,
        build_side_read_ms: profile.build_side_read_ms,
        build_side_open_ms: 0.0,
        join_ms: profile.join_ms,
        projection_ms: profile.projection_ms,
        aggregate_ms: profile.aggregate_ms,
        rows_to_join: profile.rows_to_join,
        rows_out: profile.rows_out,
        bytes_read: profile.bytes_read,
        outside_parts_ms: 0.0,
    };
    parts.outside_parts_ms = profile.total_ms
        - (parts.probe_side_read_ms
            + parts.build_side_read_ms
            + parts.build_side_open_ms
            + parts.join_ms
            + parts.projection_ms
            + parts.aggregate_ms);

    Ok(JoinArm {
        side: side.to_owned(),
        total_ms: profile.total_ms,
        parts: Some(parts),
        answer,
        columns_read: profile.columns_read,
        filters: profile.filters,
        probe_in_scan: Some(false),
        planning_ms: Some(profile.planning_ms),
        cpu_ms: Some(profile.cpu_ms),
    })
}

/// What `DuckDB`'s own profile says.
#[derive(Debug, Clone, PartialEq)]
struct DuckProfile {
    /// The whole query, as the profile reports it.
    total_ms: f64,
    /// The probe side's table scan.
    probe_side_read_ms: f64,
    /// The build side's table scan.
    build_side_read_ms: f64,
    /// The hash join.
    join_ms: f64,
    /// The projection.
    projection_ms: f64,
    /// The ungrouped aggregate.
    aggregate_ms: f64,
    /// The rows that reached the join.
    rows_to_join: u64,
    /// The rows the join produced.
    rows_out: u64,
    /// The bytes read.
    bytes_read: Option<u64>,
    /// The columns the probe side's scan read.
    columns_read: Vec<String>,
    /// The predicates the scans carried.
    filters: Vec<String>,
    /// How long `DuckDB` spent planning: the parse, the bind, the optimiser
    /// and the physical planner.
    ///
    /// It is inside the whole and outside the operator tree, so it is the
    /// biggest named part of the whole less the parts.
    planning_ms: f64,
    /// The whole query's CPU time.
    cpu_ms: f64,
}

/// Reads `DuckDB`'s own profile.
fn read_profile(path: &Path) -> Result<DuckProfile, JoinPartsError> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        JoinPartsError::ProfileUnreadable { path: path.to_path_buf(), reason: error.to_string() }
    })?;
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
        JoinPartsError::ProfileUnreadable { path: path.to_path_buf(), reason: error.to_string() }
    })?;

    let query = value.get("query").ok_or_else(|| JoinPartsError::ProfileShape {
        reason: "the profile has no `query` section".to_owned(),
    })?;
    let total_ms = seconds(query.get("total_time"));
    let bytes_read = value
        .get("io")
        .and_then(|io| io.get("total_bytes_read"))
        .and_then(serde_json::Value::as_u64);

    let root = value
        .get("operator")
        .and_then(serde_json::Value::as_array)
        .and_then(|nodes| nodes.first())
        .ok_or_else(|| JoinPartsError::ProfileShape {
            reason: "the profile has no operator tree".to_owned(),
        })?;

    let mut nodes = Vec::new();
    collect_operators(root, &mut nodes);

    let probe = pick_scan(&nodes, "lineitem");
    let build = pick_scan(&nodes, "orders");
    let join = nodes
        .iter()
        .find(|node| node.get("type").and_then(serde_json::Value::as_str) == Some("HASH_JOIN"))
        .ok_or_else(|| JoinPartsError::ProfileShape {
            reason: "the profile has no hash join".to_owned(),
        })?;

    let projection = nodes
        .iter()
        .find(|node| node.get("type").and_then(serde_json::Value::as_str) == Some("PROJECTION"))
        .map_or(0.0, |node| seconds(node.get("timing")));
    let aggregate = nodes
        .iter()
        .find(|node| {
            node.get("type").and_then(serde_json::Value::as_str) == Some("UNGROUPED_AGGREGATE")
        })
        .map_or(0.0, |node| seconds(node.get("timing")));

    let mut columns_read = Vec::new();
    let mut filters = Vec::new();
    for scan in [probe, build].into_iter().flatten() {
        let extra = scan.get("extra_info");
        if let Some(extra) = extra {
            for column in strings(extra.get("Projections")) {
                columns_read.push(column);
            }
            for filter in strings(extra.get("Filters")) {
                filters.push(filter);
            }
            if let Some(dynamic) = extra.get("Dynamic Filters").and_then(serde_json::Value::as_str)
            {
                filters.push(dynamic.to_owned());
            }
        }
    }

    // The planning is inside the whole and outside the operator tree: the
    // parse, the bind, the optimiser and the physical planner.
    let planning_ms = ["parser", "planner", "optimizer", "physical_planner"]
        .iter()
        .map(|section| {
            value.get(*section).map_or(0.0, |section| seconds(section.get("total_time")))
        })
        .sum();

    Ok(DuckProfile {
        total_ms,
        probe_side_read_ms: probe.map_or(0.0, |scan| seconds(scan.get("timing"))),
        build_side_read_ms: build.map_or(0.0, |scan| seconds(scan.get("timing"))),
        join_ms: seconds(join.get("timing")),
        projection_ms: projection,
        aggregate_ms: aggregate,
        rows_to_join: probe.map_or(0, |scan| {
            scan.get("rows_scanned").and_then(serde_json::Value::as_u64).unwrap_or(0)
        }),
        rows_out: join.get("intermediate_rows").and_then(serde_json::Value::as_u64).unwrap_or(0),
        bytes_read,
        columns_read,
        filters,
        planning_ms,
        cpu_ms: seconds(query.get("cpu_time")),
    })
}

/// Collects the operator tree's nodes, in the order they appear.
fn collect_operators(node: &serde_json::Value, into: &mut Vec<serde_json::Value>) {
    if node.get("type").is_some() {
        into.push(node.clone());
    }
    if let Some(children) = node.get("children").and_then(serde_json::Value::as_array) {
        for child in children {
            collect_operators(child, into);
        }
    }
}

/// The table scan that read the file whose name carries the marker.
fn pick_scan<'a>(nodes: &'a [serde_json::Value], marker: &str) -> Option<&'a serde_json::Value> {
    nodes.iter().find(|node| {
        node.get("type").and_then(serde_json::Value::as_str) == Some("TABLE_SCAN")
            && node
                .get("extra_info")
                .and_then(|extra| extra.get("Filename(s)"))
                .and_then(serde_json::Value::as_str)
                .is_some_and(|name| name.contains(marker))
    })
}

/// A duration in milliseconds, from a number of seconds.
fn seconds(value: Option<&serde_json::Value>) -> f64 {
    value.and_then(serde_json::Value::as_f64).unwrap_or(0.0) * 1000.0
}

/// A duration in milliseconds.
fn ms(duration: std::time::Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// The strings a profile's field holds, whether it is one string or a list.
fn strings(value: Option<&serde_json::Value>) -> Vec<String> {
    match value {
        Some(serde_json::Value::String(text)) => vec![text.clone()],
        Some(serde_json::Value::Array(items)) => {
            items.iter().filter_map(serde_json::Value::as_str).map(ToOwned::to_owned).collect()
        }
        _ => Vec::new(),
    }
}

/// `DuckDB`'s answer, from its own output.
fn duckdb_answer(stdout: &str) -> Result<f64, JoinPartsError> {
    for line in stdout.lines().rev() {
        let line = line.trim();
        if let Ok(value) = line.parse::<f64>() {
            return Ok(value);
        }
    }
    Err(JoinPartsError::ProfileShape { reason: format!("duckdb printed no answer: {stdout}") })
}

/// `DuckDB`'s own timing of the query, from its `.timer` output.
fn duckdb_timer_ms(stdout: &str) -> f64 {
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("Run Time (s): real ") {
            if let Some(seconds) = rest.split_whitespace().next() {
                if let Ok(seconds) = seconds.parse::<f64>() {
                    return seconds * 1000.0;
                }
            }
        }
    }
    0.0
}

/// The engine's answer, from its own result.
///
/// The `Int` arm's cast loses the integer's last bits above two to the
/// fifty-third, and the answer is checked against the published one with a
/// **relative** tolerance, so those bits cannot change the verdict. The
/// `allow` is scoped to this one function, for that reason.
#[allow(clippy::cast_precision_loss)]
fn answer_of(result: &columnar_engine::AggregateResult) -> f64 {
    result.groups.first().and_then(|group| group.values.first()).map_or(0.0, |value| match value {
        AggregateValue::Int(value) => *value as f64,
        AggregateValue::Float(value) => *value,
        AggregateValue::Null => 0.0,
    })
}

/// Refuses an answer that is not the published one.
fn check_answer(side: &str, answer: f64) -> Result<(), JoinPartsError> {
    let difference = (answer - PUBLISHED_ANSWER).abs();
    if difference > PUBLISHED_ANSWER.abs() * ANSWER_TOLERANCE {
        return Err(JoinPartsError::Answer {
            side: side.to_owned(),
            answer,
            expected: PUBLISHED_ANSWER,
        });
    }
    Ok(())
}

/// Where a profile is written, for one arm.
fn profile_path(mode: DuckMode) -> PathBuf {
    let arm = match mode {
        DuckMode::Profiled => "profiled",
        DuckMode::Unprofiled => "unprofiled",
        DuckMode::FilterOff => "filter-off",
    };
    std::env::temp_dir().join(format!(
        "columnar-join-parts-{}-{}-{arm}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos())
    ))
}

/// `DuckDB`'s version.
fn duckdb_version(binary: &Path) -> String {
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

/// The code the measurement was taken on.
///
/// The commit alone does not distinguish two codes -- a campaign taken on a
/// dirty tree is a campaign of a code that is not in the repository -- so the
/// tree's state travels beside the commit.
fn current_code() -> String {
    let commit = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map_or_else(
            || "unknown".to_owned(),
            |output| String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        );
    let state = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map_or_else(
            || "unknown".to_owned(),
            |output| {
                if String::from_utf8_lossy(&output.stdout).trim().is_empty() {
                    "clean".to_owned()
                } else {
                    "dirty".to_owned()
                }
            },
        );
    format!("{commit}-{state}")
}

#[cfg(test)]
mod tests {
    use super::{
        check_answer, duckdb_answer, duckdb_timer_ms, ms, seconds, strings, ANSWER_TOLERANCE,
        PUBLISHED_ANSWER,
    };

    #[test]
    fn the_published_answer_is_the_answer() {
        assert!(check_answer("engine", PUBLISHED_ANSWER).is_ok());
    }

    #[test]
    fn a_wrong_answer_is_refused() {
        assert!(check_answer("engine", PUBLISHED_ANSWER * 2.0).is_err());
    }

    #[test]
    fn the_two_sides_digits_are_the_same_answer() {
        // DuckDB prints 11472226962.890097 and the engine 11472226962.8901.
        let difference = (11_472_226_962.890_097 - PUBLISHED_ANSWER).abs();
        assert!(difference <= PUBLISHED_ANSWER.abs() * ANSWER_TOLERANCE);
    }

    #[test]
    fn duckdb_answer_is_read_from_its_last_line() {
        let output = "sum(l.l_extendedprice)\n11472226962.890097\nRun Time (s): real 0.096\n";
        let answer = duckdb_answer(output).unwrap();
        assert!((answer - 11_472_226_962.890_097).abs() < 1e-6);
    }

    #[test]
    fn duckdb_timer_is_read_from_its_own_line() {
        let output = "sum(l.l_extendedprice)\n11472226962.890097\nRun Time (s): real 0.096 user 0.08 sys 0.01\n";
        assert!((duckdb_timer_ms(output) - 96.0).abs() < 1e-9);
    }

    #[test]
    fn a_profile_field_may_be_one_string_or_a_list() {
        let one = serde_json::json!("l_orderkey");
        let many = serde_json::json!(["l_orderkey", "l_extendedprice"]);
        assert_eq!(strings(Some(&one)), vec!["l_orderkey".to_owned()]);
        assert_eq!(strings(Some(&many)).len(), 2);
        assert!(strings(None).is_empty());
    }

    #[test]
    fn seconds_are_milliseconds() {
        assert!((seconds(Some(&serde_json::json!(1.5))) - 1500.0).abs() < 1e-9);
        assert!(seconds(None).abs() < 1e-9);
        assert!((ms(std::time::Duration::from_millis(3)) - 3.0).abs() < 1e-9);
    }
}
