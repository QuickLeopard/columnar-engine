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

# Build and run everything: format check, lint, tests.
check: fmt lint test

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
query data="data/lineitem.parquet":
    cargo run --release -p columnar-cli -- query --data {{data}} --query q6

# Run the kernel A/B. Override the row count with `just bench rows=20000000`.
bench data="data/lineitem.parquet" rows="20000000" iterations="5" out="bench-results/kernel.json":
    cargo run --release -p columnar-cli -- bench --data {{data}} --rows {{rows}} --iterations {{iterations}} --out {{out}}

# Run the external comparison against DuckDB and Polars.
#
# Both incumbents are pinned to one thread, because the engine is
# single-threaded in this increment. See ADR-0007.
compare data="data/lineitem.parquet" iterations="3" threads="1" out="bench-results/compare.json":
    cargo run --release -p columnar-cli -- compare --data {{data}} --iterations {{iterations}} --threads {{threads}} --out {{out}}

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
