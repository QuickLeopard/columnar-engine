# Spec 004 — The performance gates

**Status**: accepted
**Date**: 2026-10-25
**Extends**: nothing. This closes a gap between the constitution and the code.
**Related**: ADR-0004 (the benchmark harness), ADR-0007 (the incumbents are pinned)

---

## 1. The problem

The constitution's quality-gate table lists two gates that did not exist:

| Gate | Scope | Rule |
|---|---|---|
| Perf regression | workspace | No statistically significant regression vs. the stored baseline (Mann-Whitney, shared runners). |
| Perf budget | `columnar-kernels` | Absolute budgets met. **Pinned runners only.** |

A constitution that lists a gate nobody implemented is a document that has
stopped describing the project. The rule was written; the code was not.

## 2. What a gate is for

A gate's job is to fail when something is wrong and to stay quiet when
something is right. A gate that fails at random gets ignored, and a gate that
never fails is decoration.

Both failure modes are available here, and the first one is the interesting
one: **this machine is shared**, and the noise in a measurement is larger than
most real changes. So the gate cannot be "is the new number lower than the
old number" — that is a coin flip, and it has to be loosened until it never
fires, at which point it is a coin flip that always comes up heads.

The gate asks the better question: **is the difference larger than the
machine's own spread?**

## 3. The statistics

The Mann-Whitney U test, because it makes no assumption about the shape of the
distribution — the throughput samples are not normal, they are a pile of timer
resolutions — and because it is exact for small samples, so a gate with eight
measurements in it is a real test and not an approximation.

- **Small samples** (both sides at most eight): the **exact** permutation
  distribution, by enumerating every way of splitting the pooled sample.
- **Larger samples**: the **normal approximation**, with two corrections that
  are not optional:
  - a **tie correction**, because the samples tie constantly — the timer has a
    resolution, and a benchmark that reports the same number twice has
    produced a tie. Tied observations carry less information than distinct
    ones: they compress the rank distribution, so the statistic's spread is
    narrower than the tie-free formula says and the variance is
    over-estimated if the ties are ignored. An over-estimated variance makes
    the test *conservative*, so a real regression goes unnoticed.
  - a **continuity correction**, because the statistic is discrete and the
    normal approximation is not.

The test is **one-sided**, in the direction "the current sample is the smaller
one", because that is the direction a regression lies in. A current sample
that is faster is never a regression, and the p-value says so without needing
a second test.

## 4. The finding that shaped the gate

The first implementation was statistically correct and **useless**. Five
consecutive gate runs against an unchanged baseline produced:

| flavor | runs reporting a regression | the difference |
|---|---|---|
| `cpp-auto` | 5 of 5 | 1.3% – 2.3% slower |
| `cpp-simd` | 4 of 5 | 1.7% – 3.9% slower |
| `scalar` | 4 of 5 | 0.9% – 1.2% slower |
| `rust-simd` | 0 of 5 | — |

**Thirteen false positives in twenty checks, on code that had not changed.**
The test was not wrong. The machine's throughput drifts by a percent or two
between one moment and the next, and a test with thirty-two samples on each
side detects that drift as significant every time — which is exactly what a
sensitive test should do.

The fix is not to weaken the test. It is to add a **second, independent
condition**: a regression must be statistically significant **and** practically
significant, and the practical threshold is set from the measurement above.

**The threshold is 5%**, and it is above the machine's observed drift of
0.9%–3.9%. With it, five consecutive runs on unchanged code produced **zero**
failures, and the differences that remained were reported as *within the
noise* rather than hidden.

## 5. The three answers, and then four

A gate that can only say "pass" or "fail" will eventually lie. This one says
one of four things:

| verdict | meaning | fails the gate |
|---|---|---|
| **No regression** | the current sample is not significantly slower | no |
| **Within the noise** | significantly slower, but by less than the practical threshold — the machine was different, not the code | no |
| **Regression** | significantly slower by more than the threshold | **yes** |
| **Inconclusive** | too few samples to decide, or the baseline is from another machine | no, and it is not a pass either |

The **inconclusive** answer is the one that matters most. It is not a pass,
and it must never be reported as one: a reader who sees "inconclusive" knows
the gate could not tell, which is more useful than a pass that was decided by
a threshold nobody examined.

## 6. The pinned-runner rule, made operational

The constitution says the budget gate runs on pinned runners only. The reason
is practical: a budget is an absolute number, and an absolute number measured
on a shared runner is not about the code.

The baseline records the machine it was measured on, and the gate **refuses**
to compare across machines unless the caller says so explicitly. The CPU, the
architecture, the SIMD path and the thread count all change what the numbers
mean; the operating system version and the compiler version do not change the
*machine*, and a reader is better served by a warning about those than by a
refusal. That turns "pinned runners only" from a sentence in a document into
something the code enforces.

## 7. Requirements

| | requirement |
|---|---|
| **R1** | The gate MUST compare the current measurement against a **stored baseline**, and the baseline MUST be committed. |
| **R2** | The comparison MUST be the Mann-Whitney U test, one-sided in the direction "the current sample is slower", at a declared significance level. |
| **R3** | The test MUST use the exact distribution for small samples and the normal approximation, with tie and continuity corrections, for large ones. |
| **R4** | A regression MUST be both statistically significant **and** slower by more than the baseline's practical threshold. The threshold MUST be a measured number, and the measurement MUST be published. |
| **R5** | The gate MUST distinguish *no regression*, *within the noise*, *regression* and *inconclusive*. An inconclusive result MUST NOT be reported as a pass. |
| **R6** | The gate MUST refuse to compare a baseline measured on a different CPU, architecture, SIMD path or thread count, unless the caller overrides it explicitly. |
| **R7** | The absolute budget MUST be checked separately from the statistical comparison, and a budget miss MUST fail the gate. |
| **R8** | The gate MUST be a **command**, not a test, and MUST NOT run in CI: it measures the machine, and a shared runner is noisy. |
| **R9** | The statistics, the baseline's JSON, and the verdict logic MUST be covered by deterministic tests, including the published reference values for the exact test. |
| **R10** | The harness MUST refuse to measure with the SIMD probe overridden, exactly as the A/B does. |

## 8. What this spec is not

| | why |
|---|---|
| **A CI job** | The constitution says "shared runners" for the regression gate and "pinned runners only" for the budget. This machine is not pinned, and its Actions minutes are currently blocked, so the gate is a local command. |
| **A replacement for the A/B** | The A/B is the project's published measurement; the gate is the thing that notices when the published measurement changed. |
| **A guard against machine drift** | It cannot be: the machine drifts, and the gate's answer to that is the practical threshold, not a refusal to measure. |
| **Automatically rewriting the baseline** | Writing a baseline is a deliberate act — the samples become the thing every later measurement is compared against. It is a separate command. |

## 9. Acceptance criteria

1. `cargo test -p columnar-bench --all-targets` is green, including the tests
   that pin the exact test against its published reference values.
2. Five consecutive gate runs against an unchanged baseline produce **zero**
   failures.
3. A baseline claiming an 11% faster flavor makes the gate report a regression
   and exit non-zero.
4. A baseline from a different CPU is refused unless overridden.
5. `cargo deny check` is still green, and no dependency was added.

## 10. References

| requirement | principle |
|---|---|
| R1, R2, R3 | **IV.** Evidence Over Claims — a gate is a claim about the code, and it has to be a statistical one. |
| R4, R5 | **IV.** Evidence Over Claims — and a gate that cries wolf is a claim that is not believed. |
| R6, R8 | **V.** The Comparison Is Reproducible by a Stranger — and by the same person on a quiet machine. |
| R7 | The performance standards: an absolute floor, met or not. |
| R9 | **IV.** Evidence Over Claims — the evidence is the test suite. |
| R10 | **VI.** The Comparison Is Honest — a timing taken under an override describes an emulator. |
