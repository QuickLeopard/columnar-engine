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
    cargo run --release -p columnar-cli -- dataset --scale-factor {{scale}} --seed {{seed}} --output {{out}}

# Remove the generated dataset.
clean-data:
    rm -rf data bench-results

# ---------------------------------------------------------------------------
# Running
# ---------------------------------------------------------------------------

# Run the TPC-H Q6 query over the dataset and print the plan output.
query data="data/lineitem.parquet" lazy="":
    cargo run --release -p columnar-cli -- query --data {{data}} --query q6 {{lazy}}

# Run the query the lazy way: read only the columns the plan needs, with
# the plan's predicates pushed into the Parquet reader.
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
# Publish a table from this too: run it at least three times, keep the
# reports, and derive the medians with `just medians`.
sweep data="data/lineitem.parquet" query="q6" threads="1,2,4,6,8,14" iterations="3" out="bench-results/scaling.json":
    cargo run --release -p columnar-cli -- sweep --data {{data}} --query {{query}} --threads {{threads}} --iterations {{iterations}} --out {{out}}

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
