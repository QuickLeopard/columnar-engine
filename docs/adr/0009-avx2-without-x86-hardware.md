# ADR-0009: AVX2 without x86-64 hardware — verify correctness under Rosetta, publish no timing

**Status**: accepted
**Date**: 2026-10-24
**Reviewers**: TBD

## Context

ADR-0002 committed this project to two SIMD paths: NEON on Apple Silicon, AVX2 on x86-64, with
runtime dispatch and a scalar fallback, and CI on two classes of runner.

The premise of that decision was that the x86-64 side would be *measured* somewhere. The
development hardware is an ARM64 MacBook — an Apple M4 Max — and there is no x86-64 machine, no
self-hosted runner, and no intention of buying one. So the x86-64 half was, from the start, code
that would be written and not run.

That is a bad way to leave a SIMD kernel, and the way it actually turned out was worse than
"untested". **The AVX2 Rust intrinsics path had never been compiled by anyone.** It sat in the
tree, unchecked, for the entire life of the project with two type errors in it:

```
error[E0308]: mismatched types
   --> crates/columnar-kernels/src/rust_simd.rs:396:21
    |
396 |             mask |= bits << k;
    |                     ^^^^^^^^^ expected `u64`, found `u32`
```

`_mm256_movemask_pd` returns 32 bits — one per 64-bit lane, four lanes — and the code accumulated
it into a `u64` mask with a shift that was never applied to the widened value. It had never
compiled, so it had never been wrong in a *runnable* way; it had simply never existed as far as
any compiler was concerned.

The discovery was incidental: somebody cross-built the crate for `x86_64-apple-darwin` to see
whether it was even possible. It was not.

The question this ADR answers is: **what is the honest thing to do about a code path that the
project's only machine cannot run?**

There were three candidate answers, and one of them is wrong:

1. **Delete the AVX2 path.** Simplest, but it gives up on a real implementation and shrinks the
   project.
2. **Keep it, untested, and call it "code-complete".** This is what was happening. It is what
   ADR-0002's "not yet measured" note quietly permitted.
3. **Find a way to execute it here, publish that as *correctness* only, and refuse to publish
   any timing taken that way.**

Option 3 is correct, and it turned out to be available.

## The route: Rosetta 2

Rosetta 2 is installed on Apple Silicon and runs x86-64 binaries. Two facts about it matter:

- **It executes AVX2 instructions correctly.** A probe compiled with `-mavx2` and run under
  `arch -x86_64` returns the right answers: `_mm256_add_epi64` of 7 and 5 gives 12, and
  `_mm256_movemask_pd` of an equality mask gives the expected bits.
- **It reports no AVX2 in CPUID.** The feature bits it exposes are the *host's*, and the host is an
  ARM64 chip with no AVX2 units. `__builtin_cpu_supports("avx2")` returns 0.

The second fact is why the AVX2 path was unreachable and why it could never have been reached by
simply running the tests under the emulator: the kernel's runtime probe correctly concludes
"there is no AVX2 here" and falls back to scalar. The probe is right; the machine simply is not
the one the code needs.

So the probe needs a way to be overridden — for correctness testing only.

## Decision

**1. Add `COLUMNAR_FORCE_SIMD`, a correctness-testing override of the runtime probe.**

| value | meaning |
|---|---|
| unset (default) | use whatever the CPU probe finds |
| `none` | use no SIMD path |
| `neon` | use the ARM NEON path |
| `avx2` | use the x86 AVX2 path |

The value is read once and cached. An override naming an ISA the binary was not compiled for is
ignored rather than misapplied.

**2. Make the harness refuse to measure while an override is in force.**

`just bench` and `just compare` exit with an explanation instead of producing a number. Under
Rosetta the AVX2 instructions are emulated, so a timing taken there measures the translator, not
any hardware. Publishing it would be publishing a number that does not mean what it appears to
mean, which is the one thing this repository exists to prevent. `MachineSpec` records the
override's state so a reader can check that the published artifacts were taken with it unset.

**3. Prove the path ran, rather than proving it was reported.**

A dispatch that says "AVX2" is not proof that the AVX2 instructions executed. Both the C++ and
the Rust side therefore carry a **row counter**, incremented when the SIMD path is entered. The
verification asserts that the counter moved *and* that the resulting mask matches the oracle's
bit for bit. Together those say what is actually being claimed: the instructions ran, and they
ran correctly.

**4. Gate compilation, so the path cannot rot again.**

`just check-x86` cross-compiles the workspace for `x86_64-apple-darwin` on the ARM64 machine, and
CI runs the same check on `macos-latest`. The `ubuntu-latest` CI jobs already compiled the x86-64
path natively — but only once CI was wired to a remote, and until then the local gate is the
cross-check.

**5. Publish AVX2 *performance* from a machine that has AVX2, and only ratios from one that might
not.**

The CI workflow detects AVX2 on the runner. If it is present, the kernel A/B runs and the step
summary publishes **ratios only** — `scalar → cpp-simd` and `cpp-auto → cpp-simd` — because both
are taken from the same process, on the same machine, in the same run, and are therefore robust to
whatever the runner's neighbours do to the absolute figures. Absolute rows/sec and ns/row from a
shared runner are not comparable with the ARM numbers in `PERF.md`, and the workflow says so
rather than publishing them as if they were.

A self-hosted x86-64 job remains wired for the day a machine exists. It is the only job whose
absolute figures would be worth publishing.

## Consequences

**Good.**

- The AVX2 path is no longer unverified code. It compiles, it runs, and it agrees with the oracle
  — on the author's own machine, today.
- The class of bug that was found (never-compiled, never-run code) now has a gate. `just check-x86`
  is fast and catches it locally, which CI alone did not.
- The "not yet measured" claim in `PERF.md` is now specific: *performance* is not measured,
  *correctness* is. Previously it covered both, and the second was a gap in the truth.
- The C++ probe was rewritten to use `CPUID`/`XGETBV` directly instead of
  `__builtin_cpu_supports("avx2")`. The builtin expands to a call into `libcpu_indicator` on
  Darwin, which is not on every toolchain's default link line — it is what made the first x86-64
  link fail. The hand-written probe performs the same checks with no link-time dependency.

**Bad, and accepted.**

- The override is a footgun by construction: forcing `avx2` on a real x86-64 machine without AVX2
  raises `SIGILL`. That is what "force" means, the script only uses it where it has been verified
  to work, and the benchmark refuses to run with it set.
- The emulation is slow, so the verification run reduces the property-test case count. The
  fixed-size boundary, NULL and NaN tests run in full regardless.
- Correctness on an emulator is not correctness on silicon. It is strong evidence, and it is not
  proof; the native x86-64 CI job is what closes the loop when one is available.

**Neutral.**

- The AVX2 *performance* numbers still do not exist. This ADR does not change that, and no
  estimate is offered. The blueprint's "AVX2 6.7× scalar" was measured on different hardware and
  remains not reproduced and not quoted as this project's own.

## Alternatives considered

**Run the emulator and publish the timings.** Rejected. The instructions are emulated; the number
measures the translator. Publishing it would be exactly the kind of number this repository's
framing rule exists to prevent.

**Claim the AVX2 path is "code-complete" and leave it.** Rejected. That is the status quo that let
a file with two compile errors sit in the tree indefinitely, and "code-complete" was doing work
that a compiler should have been doing.

**Drop AVX2 and keep only NEON.** Rejected for now, and worth revisiting. The dual-ISA kernel is
a real design goal, and the override makes it verifiable. If the project never gets x86-64
hardware, the AVX2 path becomes code that is correct but never fast, and deleting it would be
honest — but the cost of keeping it is now one cross-check in CI.
