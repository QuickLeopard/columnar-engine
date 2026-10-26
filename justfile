# Just recipes for the columnar engine.
#
# `just` is the single entry point. A reviewer who clones this repository
# should be able to read this file and know every command they need.

# List the recipes.
default:
    @just --list

# ---------------------------------------------------------------------------
# Build and check
# ---------------------------------------------------------------------------

# Build the workspace in release mode.
build:
    cargo build --workspace --release

# Build and run everything: format check, lint, tests, and the check that
# the published figures still agree with the committed reports.
#
# The last one is also a CI job, but the local gate must not be weaker than
# CI -- a private repository's Actions minutes are billed, and this account's
# billing is currently blocked, so the local gate is the one that runs.
check: fmt lint test check-published

# Compile the x86-64 half of the kernel from this ARM64 machine.
#
# The AVX2 path -- in C++ and in the Rust intrinsics -- only compiles on
# x86-64. It sat in the tree, uncompiled and wrong, because the development
# machine is an ARM64 MacBook and nothing else checked it. This recipe is
# the local gate that catches that class of rot.
check-x86:
    @rustup target list --installed | grep -qx x86_64-apple-darwin || rustup target add x86_64-apple-darwin
    cargo check --workspace --all-targets --target x86_64-apple-darwin

# Compile the browser's half of the kernel from this machine.
#
# The `simd128` path only exists in a `wasm32` build, so without this it rots
# silently too -- and it is the path the browser runs, so a reader who opens
# the page deserves a kernel that was compiled by somebody.
#
# Only the two crates that reach the browser are checked: the engine's Parquet
# reader has no reason to exist in a browser and does not build for one.
#
# Only the **libraries** are checked, not the test harnesses: the differential
# suite's dev-dependencies are Unix-only, and the browser's check is the page
# itself -- it compares the two masks before it prints anything.
check-wasm:
    @rustup target list --installed | grep -qx wasm32-unknown-unknown || rustup target add wasm32-unknown-unknown
    RUSTFLAGS="-C target-feature=+simd128" cargo check --lib --target wasm32-unknown-unknown -p columnar-kernels -p columnar-web

# Check formatting.
fmt:
    cargo fmt --all -- --check

# Lint with clippy, warnings are errors.
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Run every test, including the differential kernel suite.
test:
    cargo test --workspace --all-targets

# Run only the differential kernel suite -- the gate that protects the
# project's headline claim.
differential:
    cargo test -p columnar-kernels --test differential -- --nocapture

# Verify the AVX2 path's correctness on this machine, via Rosetta 2.
#
# This is the answer to "this machine has no AVX2". Rosetta 2 executes AVX2
# instructions correctly but reports no AVX2 in CPUID, so the probe would
# otherwise hide the path. The override reaches it, and the row counter
# proves the instructions ran.
#
# This checks correctness, never performance: under Rosetta the instructions
# are emulated, and `bench` refuses to run with the override in force.
verify-avx2:
    ./scripts/verify-avx2.sh

# Check the dependency policy.
deny:
    cargo deny check

# ---------------------------------------------------------------------------
# Data
# ---------------------------------------------------------------------------

# Generate the benchmark dataset. Override with `just dataset scale=1 seed=42`.
dataset scale="1" seed="24001" out="data/lineitem.parquet":
    cargo run --release --quiet -p columnar-cli -- dataset --scale-factor {{scale}} --seed {{seed}} --output {{out}}

# Generate the dataset with a **row-group size** of the given number of rows.
#
# The row-group count is the cap on the engine's read parallelism, so this is
# the knob that opens it: a million rows a group is six row groups over six
# million rows, and a quarter of a million is twenty-four of them.
#
# The file is named after the row-group count it has, because the count is
# what the measurement is about. Override with
# `just dataset-row-groups 500000 seed=24001 scale=1`.
dataset-row-groups rows="250000" seed="24001" scale="1":
    #!/usr/bin/env bash
    set -euo pipefail
    export PATH="$HOME/.cargo/bin:$PATH"
    total=$((6000000 * {{scale}}))
    groups=$(( (total + {{rows}} - 1) / {{rows}} ))
    out="data/lineitem-rg${groups}.parquet"
    cargo run --release --quiet -p columnar-cli -- dataset \
        --scale-factor {{scale}} --seed {{seed}} \
        --row-group-rows {{rows}} --output "$out"
    echo "wrote $out with $groups row groups"

# Remove the generated dataset.
#
# Only `data/`. The reports in `bench-results/` are **committed** -- they are
# the evidence every published number is derived from -- so they are not
# build artefacts and they are not removed here.
clean-data:
    rm -rf data

# ---------------------------------------------------------------------------
# Running
# ---------------------------------------------------------------------------

# The browser build: the engine's kernel as a WebAssembly module, and the
# page that runs it.
#
# The module is `columnar-kernels` compiled for `wasm32-unknown-unknown` with
# the `simd128` feature -- the browser's third SIMD path, beside NEON and
# AVX2. The page rebuilds the benchmark's column from a histogram, runs the
# oracle and the `simd128` path over it, and compares their masks bit for
# bit before it prints a number.
#
# The build needs no toolchain beyond Rust: no `wasm-bindgen`, no `npm`, no
# emscripten. `rust-lld` ships with the toolchain.
web port="8080":
    #!/usr/bin/env bash
    set -euo pipefail
    just web-build
    echo
    echo "== serving target/web on http://localhost:{{port}} =="
    echo "   open that URL; the page runs the kernel and posts its result back."
    echo
    python3 scripts/web-server.py "{{port}}"

# Build the browser module and the page into target/web.
web-build:
    #!/usr/bin/env bash
    set -euo pipefail
    rustup target list --installed | grep -qx wasm32-unknown-unknown || rustup target add wasm32-unknown-unknown
    echo "== building the wasm32 module (simd128) =="
    RUSTFLAGS="-C target-feature=+simd128" \
        cargo build --release -p columnar-web --target wasm32-unknown-unknown
    mkdir -p target/web
    cp target/wasm32-unknown-unknown/release/columnar_web.wasm target/web/columnar_web.wasm
    cp crates/columnar-web/web/index.html crates/columnar-web/web/app.js target/web/
    echo "== target/web =="
    ls -l target/web

# Publish the browser's measurement into bench-results/.
#
# The page writes its report to target/web/browser-report.json. Copying it
# into bench-results/ is the deliberate act -- that is the difference between
# running the page and publishing what it measured.
web-report:
    #!/usr/bin/env bash
    set -euo pipefail
    test -f target/web/browser-report.json || { echo "no report: run the page first"; exit 1; }
    cp target/web/browser-report.json bench-results/browser.json
    echo "== bench-results/browser.json =="
    cat bench-results/browser.json

# Build the Rust peer: DataFusion, as a separate process.
#
# The peer is a **separate cargo project** under `tools/`, with its own lock
# file, and it is built separately on purpose: it is an instrument, not a
# dependency. Nothing here touches the engine's dependency graph, and
# `cargo deny` and `cargo test --workspace` never see it.
#
# The binary lands in `tools/bin/`, which is where the comparison harness
# looks for a comparator -- the same place it finds DuckDB.
peer-build:
    #!/usr/bin/env bash
    set -euo pipefail
    export PATH="$HOME/.cargo/bin:$PATH"
    cd tools/datafusion-peer
    cargo build --release --locked
    mkdir -p ../bin
    cp target/release/datafusion-peer ../bin/datafusion-peer
    echo "== tools/bin/datafusion-peer =="
    ../bin/datafusion-peer --version

# Take the materialised column apart, and run the real Q6.
#
# The probe is spec 014's instrument: it measures the decode of the column
# the query materialises -- and of the two it filters on -- and then runs the
# real Q6 through the engine's own lazy path.
probe data="data/lineitem.parquet" out="bench-results/materialised.json":
    cargo run --release -p columnar-engine --example materialised_column_probe -- {{data}} --out {{out}}

# Take the **per-row-group cost** apart.
#
# This is spec 016's instrument: it measures the crate's own read of a single
# column, the engine's scan, the page shape of every row group, and the cost
# of the reader's batch size -- the measurement that found the partial batch.
probe-cost data="data/lineitem.parquet" out="bench-results/row-group-cost.json":
    cargo run --release -p columnar-engine --example row_group_cost_probe -- {{data}} --out {{out}}

# Sweep the lazy path's **reader batch limit**.
#
# The limit is a tuning constant, and a constant has to be measured for the
# shape of the work it is used on. This walks the limits, prints the batch
# each one derives for the file, and then measures the two candidates against
# each other with the order alternating -- over both shapes the lazy path
# serves.
probe-batch-size data="data/lineitem.parquet" out="bench-results/batch-size.json":
    cargo run --release -p columnar-engine --example lazy_batch_size -- {{data}} --out {{out}}

# Sweep the **size** of the data: this engine and every incumbent, at every
# size, on the same file for each size.
#
# The datasets are generated by this repository's own generator, so they are a
# function of the scale factor and the seed. The generator is byte-deterministic:
# scale factor one at seed 24001 is the file `data/lineitem.parquet`.
curve scales="1,2,4" seed="24001" threads="1" iterations="5" budget="1024" out="bench-results/curve.json":
    #!/usr/bin/env bash
    set -euo pipefail
    data=()
    for sf in {{scales}}; do
        file="data/lineitem-sf$sf.parquet"
        if [ ! -f "$file" ]; then
            echo "== generating the dataset at scale factor $sf =="
            cargo run --release --quiet -p columnar-cli -- dataset \
                --scale-factor "$sf" --seed {{seed}} --output "$file"
        fi
        data+=("$file")
    done
    cargo run --release -p columnar-cli -- curve \
        --data "$(IFS=,; echo "${data[*]}")" \
        --query q6-revenue --threads {{threads}} --iterations {{iterations}} \
        --memory-budget-mib {{budget}} --out {{out}}

# The demo: the whole story, in one command, in about a minute.
#
# A stranger who clones this repository should be able to see what it is in
# one step, without reading a table of numbers first:
#
#   1. **the kernel A/B** -- the project's headline claim, with the four
#      implementations' match counts side by side, which is the bit-identical
#      agreement made visible rather than asserted;
#   2. **the engine's own plan output** -- two columns of sixteen, with the
#      predicates pushed into the reader;
#   3. **the comparison** against DuckDB and Polars.
#
# One iteration and one thread: this is a demonstration, not a published
# number. The published numbers come from `just compare` run three times over,
# and `just medians` derives the table from the committed reports. See
# PERF.md.
demo data="data/lineitem.parquet":
    #!/usr/bin/env bash
    set -euo pipefail
    data="{{data}}"
    echo "== building =="
    cargo build --release --workspace
    if [ ! -f "$data" ]; then
      echo
      echo "== generating the dataset =="
      cargo run --release --quiet -p columnar-cli -- dataset --scale-factor 1 --seed 24001 --output "$data"
    fi
    echo
    echo "== the kernel: four implementations, one answer =="
    cargo run --release --quiet -p columnar-cli -- bench --data "$data" --rows 20000000 --iterations 5 --out "$(mktemp)"
    echo
    echo "== the engine's own plan =="
    cargo run --release --quiet -p columnar-cli -- query --data "$data" --query q6 --lazy
    echo
    echo "== against DuckDB and Polars, one thread =="
    cargo run --release --quiet -p columnar-cli -- compare --data "$data" --iterations 1 --threads 1 --out "$(mktemp)"

# Run the TPC-H Q6 query over the dataset and print the plan output.
query data="data/lineitem.parquet" lazy="":
    cargo run --release -p columnar-cli -- query --data {{data}} --query q6 {{lazy}}

# Run the query the lazy way: read only the columns the plan needs, with
# the plan's predicates applied during the scan.
query-lazy data="data/lineitem.parquet":
    cargo run --release -p columnar-cli -- query --data {{data}} --query q6 --lazy

# Run the kernel A/B. Override the row count with `just bench rows=20000000`.
bench data="data/lineitem.parquet" rows="20000000" iterations="5" out="bench-results/kernel.json":
    cargo run --release -p columnar-cli -- bench --data {{data}} --rows {{rows}} --iterations {{iterations}} --out {{out}}

# Run the external comparison against DuckDB and Polars.
#
# Both incumbents are pinned to one thread, because this is the like-for-like
# *single-threaded* comparison. See ADR-0007 and its amendment in ADR-0011:
# the thread-count sweep is the second table, and `just sweep` produces it.
#
# Publish a table from this: run it at least three times, keep the reports,
# and derive the medians with `just medians`.
compare data="data/lineitem.parquet" iterations="3" threads="1" out="bench-results/compare.json":
    cargo run --release -p columnar-cli -- compare --data {{data}} --iterations {{iterations}} --threads {{threads}} --out {{out}}

# Sweep the thread count: the engine and both incumbents, at the same thread
# counts, on the same file.
#
# This is the second table, not a replacement for the first. The headline
# comparison stays pinned to one thread, because that is the like-for-like
# single-threaded claim; this one shows how each system's number moves with
# the thread count, so the reader sees the shape of the curve and not only its
# endpoint.
#
# The engine's parallelism is capped by the file's row groups, so the report
# records both the thread count asked for and the workers that ran.
#
# Three modes, because they answer different questions: `cold` reads the file
# inside the measurement, which is what a user waiting on a command line
# pays; `resident` measures the query against data that is already in memory,
# which is the pure query-engine measurement; and `load` measures the load
# itself -- the time to get the file into memory -- which is the one
# measurement in this repository that is about the file rather than the query.
#
# Publish a table from this too: run it at least three times, keep the
# reports, and derive the medians with `just medians`.
sweep data="data/lineitem.parquet" query="q6" threads="1,2,4,6,8,14" iterations="3" mode="cold" out="bench-results/scaling.json":
    cargo run --release -p columnar-cli -- sweep --data {{data}} --query {{query}} --threads {{threads}} --iterations {{iterations}} --mode {{mode}} --out {{out}}

# The performance gate: measure the kernel and compare it against the stored
# baseline.
#
# This measures the machine, so it is not a `cargo test` and not a CI job: a
# shared runner is noisy, and a gate that fails at random is a gate nobody
# trusts. Run it on a machine that is not doing anything else.
#
# A regression has to be BOTH statistically significant AND more than the
# baseline's practical threshold. The threshold is 5%, which is above the
# machine's own run-to-run drift -- measured, not chosen; see spec 004.
perf-gate data="data/lineitem.parquet" blocks="32":
    cargo run --release -p columnar-cli -- gate --data {{data}} --blocks {{blocks}} --baseline bench-results/kernel-baseline.json

# Rewrite the performance baseline from a fresh measurement.
#
# This is a deliberate act: the samples become the thing every later
# measurement is compared against. Do it on a quiet machine.
perf-baseline data="data/lineitem.parquet" blocks="32" out="bench-results/kernel-baseline.json":
    cargo run --release -p columnar-cli -- gate --data {{data}} --blocks {{blocks}} --write-baseline {{out}}

# Derive the published comparison table from the committed raw reports.
#
# The table in PERF.md is the median of three full `just compare` runs. This
# is the derivation, and it prints each figure as `median (min-max)`.
medians:
    python3 scripts/medians.py

# Check that the published figures still agree with the committed reports.
#
# A hand-edited number in PERF.md or README.md that drifts away from the
# evidence fails here.
check-published:
    python3 scripts/check-published-figures.py

# Run everything that produces a published number.
perf: bench compare

# ---------------------------------------------------------------------------
# Housekeeping
# ---------------------------------------------------------------------------

# Remove build artefacts.
clean:
    cargo clean

# Print the machine spec the next benchmark would record.
machine:
    @echo "cpu:    $(sysctl -n machdep.cpu.brand_string 2>/dev/null || lscpu | head -1)"
    @echo "cores:  $(sysctl -n hw.ncpu 2>/dev/null || nproc)"
    @echo "arch:   $(uname -m)"
    @echo "os:     $(sw_vers -productVersion 2>/dev/null || uname -sr)"
    @echo "rustc:  $(rustc --version 2>/dev/null || echo 'not on PATH')"
