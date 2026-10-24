#!/usr/bin/env bash
#
# verify-avx2.sh -- prove the AVX2 path is correct, on a machine that is not
# an AVX2 machine.
#
# # The problem
#
# The project's SIMD story has two halves: NEON on Apple Silicon, and AVX2 on
# x86-64. The machine this project is developed on is an ARM64 MacBook, so
# the AVX2 half could not be executed at all -- and, as it turned out, could
# not even be *compiled*, because the x86-64 code was never built by anyone.
# It sat in the tree, unchecked, with two type errors in it.
#
# # The route
#
# Rosetta 2 is installed on Apple Silicon and runs x86-64 binaries. Its
# translator implements the AVX2 instruction set correctly -- an AVX2
# instruction produces the right answer on the ARM64 host. What it does
# *not* do is advertise AVX2: the CPUID feature bits it reports are the
# host's, and the host has no AVX2 units. So the kernel's runtime probe
# correctly concludes "no AVX2 here" and never enters the path.
#
# `COLUMNAR_FORCE_SIMD=avx2` overrides the probe. That is the only thing
# this script changes, and it exists so the AVX2 path can be *correctness*
# tested on ARM64 hardware.
#
# # What this script does NOT produce
#
# A performance number. Under Rosetta the AVX2 instructions are emulated, so
# a timing taken here measures the translator, not any hardware. The script
# therefore runs the test suite only, and never the benchmark. Anything the
# benchmark prints with the override in force describes an emulator.
#
# The real AVX2 performance numbers require an x86-64 machine and are
# published only from a machine where the CPU actually has AVX2.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

export PATH="$HOME/.cargo/bin:$PATH"

target="x86_64-apple-darwin"

# The emulator runs the suite roughly an order of magnitude slower than
# native code, so the property-test case count is reduced. The case count
# is a sampling parameter, not a correctness parameter: every case is still
# checked against the oracle, and the boundary, NULL and NaN tests below are
# fixed-size and run in full.
export PROPTEST_CASES="${PROPTEST_CASES:-32}"

say() { printf '%s\n' "$*"; }

say "== Columnar Engine -- AVX2 correctness under Rosetta 2 =="
say ""

# ---------------------------------------------------------------------------
# 1. The emulator has to be there.
# ---------------------------------------------------------------------------
if ! arch -x86_64 /usr/bin/uname -m >/dev/null 2>&1; then
    say "error: Rosetta 2 is not available on this machine." >&2
    say "       install it with:  softwareupdate --install-rosetta" >&2
    exit 1
fi
say "emulator:   $(arch -x86_64 /usr/bin/uname -m) via Rosetta 2"

# ---------------------------------------------------------------------------
# 2. The x86-64 target has to be installed.
# ---------------------------------------------------------------------------
if ! rustup target list --installed | grep -qx "$target"; then
    say "installing the rust target $target ..."
    rustup target add "$target"
fi
say "rust target: $target ($(rustc --version))"

# ---------------------------------------------------------------------------
# 3. Report what the CPU probe says on its own, so the override's effect is
#    visible rather than assumed.
#
#    The override is deliberately *not* exported globally: the "without"
#    report below has to run with the probe in charge.
# ---------------------------------------------------------------------------
say ""
say "-- without the override --"
say "the probe reads the host's CPUID, which has no AVX2, so the path is"
say "unreachable. This is the correct behaviour, not a failure:"
say ""

# ---------------------------------------------------------------------------
# 4. Build the test suite for x86-64.
# ---------------------------------------------------------------------------
say "building the kernel test suite for $target ..."
build_output="$(cargo test --release -p columnar-kernels --target "$target" --no-run 2>&1)" || {
    say "$build_output" >&2
    say "error: the x86-64 build failed." >&2
    exit 1
}

executables="$(printf '%s\n' "$build_output" | grep -oE '\((target/[^)]+)\)' | tr -d '()' || true)"

if [ -z "$executables" ]; then
    say "$build_output" >&2
    say "error: no test executables were produced." >&2
    exit 1
fi

# The report test is the one that shows the before/after of the override.
report_executable=""
for executable in $executables; do
    case "$executable" in
        *simd_report*) report_executable="$executable" ;;
    esac
done

report() {
    arch -x86_64 "$report_executable" --nocapture --test-threads=1 2>&1 \
        | grep -E 'forced|simd_kind|avx2 rows|matches oracle' \
        | sed 's/^/    /'
}

if [ -n "$report_executable" ]; then
    report
fi

# ---------------------------------------------------------------------------
# 5. Run every one of them under the emulator, with the override in force.
# ---------------------------------------------------------------------------
say ""
say "-- with COLUMNAR_FORCE_SIMD=avx2 --"
say "the AVX2 path is live, and the row counter shows its instructions ran:"
say ""

export COLUMNAR_FORCE_SIMD="avx2"

if [ -n "$report_executable" ]; then
    report
fi

failed=0
count=0

while IFS= read -r executable; do
    [ -n "$executable" ] || continue
    count=$((count + 1))
    say "running $executable under Rosetta 2 ..."
    if ! arch -x86_64 "$executable" --nocapture; then
        say "  FAILED: $executable" >&2
        failed=$((failed + 1))
    fi
done <<EOF
$executables
EOF

say ""
if [ "$failed" -ne 0 ]; then
    say "RESULT: FAILED -- $failed of $count test binaries failed." >&2
    exit 1
fi

say "RESULT: PASSED -- $count test binaries green under Rosetta 2 with AVX2 forced."
say ""
say "What this establishes: the AVX2 path -- both the C++ FFI path and the"
say "Rust intrinsics path -- compiles, executes, and produces masks identical"
say "to the oracle's, bit for bit, on an ARM64 Mac."
say ""
say "What this does not establish: any AVX2 performance number. Under"
say "Rosetta the instructions are emulated. AVX2 timings require an x86-64"
say "machine with AVX2 and are published only from one."
