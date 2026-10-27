// The browser build of the engine's comparison kernel.
//
// The page loads the WebAssembly module the repository builds, rebuilds the
// column from the histogram the module carries, runs the oracle and the
// `simd128` path over it, and compares their masks bit for bit.
//
// Three rules the page keeps:
//
//   1. **No number without a check.** The throughput is printed only after
//      the two implementations' masks are identical. An implementation that
//      computed a different predicate would otherwise publish a faster,
//      wrong number.
//
//   2. **Medians and spreads, not a best run.** The measurement is taken
//      over several rounds and the median is published with the range, the
//      way the native benchmark does it. This machine is not isolated.
//
//   3. **The clock is the browser's.** `std::time::Instant` does not exist
//      on `wasm32-unknown-unknown` -- there is no clock in that platform --
//      so the timing is taken here, around the call.

"use strict";

const ORACLE = 0;
const SIMD = 1;

const ROWS = 20_000_000;
const ITERATIONS = 5;
const ROUNDS = 5;
const THRESHOLD = 0.04;

const el = (id) => document.getElementById(id);

function say(text) {
  el("out").textContent = text;
}

// Whether a run is in flight, and whether a press arrived while it was.
//
// The button is **never disabled**: a browser drops a click on a disabled
// button entirely, so a reader who pressed it while the page was measuring
// would get nothing at all -- no run, no status change, no feedback. A
// visible control has to answer. A press that arrives during a run is kept
// and run again when the run in flight ends.
let running = false;
let queued = false;

function label(text) {
  el("run").textContent = text;
}

function setField(id, value, className) {
  const node = el(id);
  node.textContent = value;
  if (className) {
    node.className = className;
  }
  return node;
}

function formatRows(rows) {
  return rows.toLocaleString("en-US");
}

// Loads and instantiates the module.
async function loadModule() {
  const response = await fetch("columnar_web.wasm");
  if (!response.ok) {
    throw new Error(
      `could not fetch columnar_web.wasm (${response.status}). Run \`just web\` and open the URL it prints.`
    );
  }
  const bytes = await response.arrayBuffer();
  const { instance } = await WebAssembly.instantiate(bytes, {});
  return instance.exports;
}

// The commit the page was built from.
//
// It is baked into the **built** copy of this file. A page served from a
// static host has no server to ask, and a static host answers an unknown
// path with its index page -- so "ask the server for the commit" is a
// question whose answer has to be checked, not trusted.
//
// The local server answers `GET /commit` from git at the moment the page
// asks, so the live commit wins there; this is the fallback for the hosts
// with no server at all.
const BUILD_COMMIT = "__BUILD_COMMIT__";

// The commit the module was built from.
//
// It comes from the **server**, not from the module: cargo caches a build
// script, so a commit baked into the module can be a commit the module was
// not built from. The server reads git when the page asks, so it cannot be
// stale. A page served from somewhere with no server says which commit it
// was built from instead.
async function fetchCommit() {
  try {
    const response = await fetch("commit");
    if (!response.ok) {
      return BUILD_COMMIT;
    }
    const text = (await response.text()).trim();
    // A commit is a short token. A document is not, and a host that answers
    // an unknown path with its index page would otherwise pass a whole
    // document off as a commit.
    if (!text || text.length > 40 || text.includes("<")) {
      return BUILD_COMMIT;
    }
    return text;
  } catch (error) {
    return BUILD_COMMIT;
  }
}

// Times one implementation over the prepared column.
//
// The warm-up pass is not timed: the first call pays for the module's first
// touch of its buffers, and a benchmark that includes that measures the
// loader.
function timeRun(exports, flavor, threshold) {
  exports.columnar_web_run(flavor, ROWS, 1, threshold);
  const start = performance.now();
  // WebAssembly's `i64` arrives in JavaScript as a BigInt, which
  // `JSON.stringify` refuses to serialise. The counts fit in a double.
  const matches = Number(exports.columnar_web_run(flavor, ROWS, ITERATIONS, threshold));
  const elapsedMs = performance.now() - start;
  return { matches, elapsedMs, nsPerRow: (elapsedMs * 1e6) / (ROWS * ITERATIONS) };
}

function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
}

function range(values) {
  return [Math.min(...values), Math.max(...values)];
}

function fmtRatio(value) {
  return `${value.toFixed(2)}x`;
}

function fmtRate(nsPerRow) {
  return `${(1e9 / nsPerRow / 1e9).toFixed(3)} G`;
}

async function run() {
  running = true;
  label(queued ? "Running… — it will run again" : "Running…");
  el("status").textContent = "loading the module…";

  let exports;
  try {
    exports = await loadModule();
  } catch (error) {
    el("status").textContent = "failed";
    say(
      `The module did not load.\n\n${error.message}\n\nIf the browser does not support WebAssembly's \`simd128\`, the module cannot be instantiated at all -- that is the browser saying no, not this page failing.`
    );
    finish();
    return;
  }

  const simdLive = exports.columnar_web_simd_kind() === 1;
  const commit = await fetchCommit();
  const expected = Number(exports.columnar_web_expected_matches(ROWS));
  const threshold = exports.columnar_web_threshold();

  setField("c-browser", navigator.userAgent);
  setField("c-engine", `wasm32-unknown-unknown · simd128 · commit ${commit}`);
  setField("c-machine", `${navigator.hardwareConcurrency || "?"} logical cores`);
  setField("c-simd", simdLive ? "simd128 (live)" : "none — the oracle and the page are the same code");
  setField("c-rows", "building…");
  setField("c-iterations", `${ITERATIONS} iterations · ${ROUNDS} rounds`);
  el("status").textContent = "rebuilding the column…";

  const buildStart = performance.now();
  exports.columnar_web_prepare(ROWS);
  const buildMs = performance.now() - buildStart;
  setField("c-rows", `${formatRows(ROWS)} (rebuilt in ${buildMs.toFixed(0)} ms)`);

  const oracleRounds = [];
  const simdRounds = [];
  for (let round = 0; round < ROUNDS; round += 1) {
    el("status").textContent = `running… round ${round + 1} of ${ROUNDS}`;
    oracleRounds.push(timeRun(exports, ORACLE, threshold));
    simdRounds.push(timeRun(exports, SIMD, threshold));
  }

  // The edge check: every operator, over the values that are easy to get
  // wrong -- a NaN, the two zeros, the two infinities, a NULL row -- and a
  // NaN threshold, which must never match for any operator.
  //
  // This is the browser's differential suite. CI has no browser, and the
  // path it checks only exists in a browser, so it runs here, in the
  // reader's browser, every time the page is opened.
  const edgesOk = exports.columnar_web_check_edges() === 1;
  setField(
    "c-edges",
    edgesOk ? "identical, on every operator and every edge" : "THE TWO PATHS DISAGREE ON AN EDGE",
    edgesOk ? "ok" : "bad"
  );

  // The mask check comes after the runs, and it is the gate: no throughput is
  // published for two implementations that did not produce the same mask.
  const agree = exports.columnar_web_masks_agree() === 1;
  setField(
    "c-mask",
    agree ? "identical, bit for bit" : "THE TWO MASKS DIFFER",
    agree ? "ok" : "bad"
  );

  const oracleMatches = oracleRounds[oracleRounds.length - 1].matches;
  const simdMatches = simdRounds[simdRounds.length - 1].matches;
  const countOk = expected !== 0 && oracleMatches === expected && simdMatches === expected;
  setField(
    "c-matches",
    countOk
      ? `${formatRows(oracleMatches)} — the native benchmark's ${formatRows(expected)}, exactly`
      : `${formatRows(oracleMatches)} — expected ${formatRows(expected)}`,
    countOk ? "ok" : "bad"
  );

  if (!agree || !edgesOk) {
    el("status").textContent = "the two paths disagree";
    say(
      "The two implementations did NOT produce the same answer.\n\n" +
        "No throughput is printed: an implementation that computes a different predicate would\n" +
        "otherwise publish a faster, wrong number."
    );
    finish();
    return;
  }

  const oracleNs = oracleRounds.map((r) => r.nsPerRow);
  const simdNs = simdRounds.map((r) => r.nsPerRow);
  const oracleMedian = median(oracleNs);
  const simdMedian = median(simdNs);
  const [oracleLow, oracleHigh] = range(oracleNs);
  const [simdLow, simdHigh] = range(simdNs);
  const ratio = oracleMedian / simdMedian;

  say(
    [
      "flavor      rows/sec      ns/row   vs oracle      matches",
      "---------------------------------------------------------------",
      `oracle     ${fmtRate(oracleMedian).padStart(7)}   ${oracleMedian.toFixed(3).padStart(8)}     1.00x   ${formatRows(oracleMatches)}`,
      `simd128    ${fmtRate(simdMedian).padStart(7)}   ${simdMedian.toFixed(3).padStart(8)}   ${fmtRatio(ratio).padStart(6)}   ${formatRows(simdMatches)}`,
      "",
      `spread     oracle ${oracleLow.toFixed(3)}–${oracleHigh.toFixed(3)} ns/row · simd128 ${simdLow.toFixed(3)}–${simdHigh.toFixed(3)} ns/row`,
      "",
      "the two masks are identical, bit for bit.",
      `the match count is the native benchmark's: ${formatRows(oracleMatches)}.`,
      "",
      `rows        ${formatRows(ROWS)}`,
      `iterations  ${ITERATIONS} per round, ${ROUNDS} rounds, median published`,
      `threshold   l_discount >= ${threshold}`,
    ].join("\n")
  );

  // The report is posted back so that the number is written by a machine and
  // not by hand. If the server is not there -- the page was opened from the
  // file system, the server was started without it, or the page is on a
  // static host -- the page still works; it just does not leave a record.
  //
  // A static host answers a POST it has nowhere to put with 404 or 405, and
  // `fetch` does **not** throw on that, so the status is what says so. The
  // page checks it, because a page that claims a report was written when it
  // was not is worse than a page that says nothing.
  const report = {
    browser: navigator.userAgent,
    engine: "wasm32-unknown-unknown",
    simd: simdLive ? "simd128" : "none",
    commit,
    machine: `${navigator.hardwareConcurrency || "?"} logical cores`,
    rows: ROWS,
    iterations: ITERATIONS,
    rounds: ROUNDS,
    threshold,
    column_build_ms: buildMs,
    oracle_ns_per_row: oracleNs,
    simd_ns_per_row: simdNs,
    oracle_median_ns_per_row: oracleMedian,
    simd_median_ns_per_row: simdMedian,
    ratio_median: ratio,
    matches: oracleMatches,
    expected_matches: expected,
    masks_agree: agree,
    edges_agree: edgesOk,
  };
  try {
    // A host that ignores POST entirely would leave the fetch pending for
    // ever, and the status line is the last thing a reader reads. The
    // numbers are already printed by this point; this only says what
    // happened to the report.
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 10000);
    const response = await fetch("report", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(report, null, 2),
      signal: controller.signal,
    });
    clearTimeout(timeout);
    el("status").textContent = response.ok
      ? "done · the report was written to target/web/browser-report.json"
      : `done · there is no report server at this address (${response.status}) — the numbers above are this browser's`;
  } catch (error) {
    el("status").textContent = `done · the report was not written: ${error.message}`;
  }

  finish();
}

// Ends a run.
//
// The button is never disabled, so the only thing this has to do is say
// what is true now: the run is over, and the reader can ask for another
// one. A press that arrived while the run was in flight is not lost -- it
// runs now.
function finish() {
  running = false;
  if (queued) {
    queued = false;
    run();
    return;
  }
  // The page runs on load, so by the time the reader can press anything,
  // the kernel has already run once. The button says so.
  label("Run the kernel again");
}

// The button answers a press, always.
//
// While a run is in flight the press is kept rather than dropped, and the
// button says that it was heard.
el("run").addEventListener("click", () => {
  if (running) {
    queued = true;
    label("Running… — it will run again");
    return;
  }
  run();
});

// The page runs on load. A reader who opens the link should see the
// measurement, not a button they have to find.
run();
