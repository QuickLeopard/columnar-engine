# Spec 004 — Tasks

**Spec**: [`spec.md`](spec.md)
**Status**: implemented. Every requirement below is done, and the gate is green.

Legend: `[x]` done · `[ ]` open · `[~]` partially done, and the gap is written down.

---

## T1 — The statistics

- [x] `mann_whitney_u(first, second)` — one-sided in the direction "the first
      sample is the smaller one", with both p-values reported.
- [x] The **exact** permutation distribution for samples of at most eight: every
      way of splitting the pooled sample is enumerated.
- [x] The **normal approximation** for larger samples, with the **tie
      correction** and the **continuity correction**.
- [x] The tie correction is *shown* to matter, by a test that compares the
      corrected and uncorrected paths on the same sample and requires them to
      differ. It is not asserted to.
- [x] The normal CDF is the Abramowitz and Stegun 7.1.26 approximation of the
      error function, whose absolute error is below 1.5e-7. **No dependency was
      added**: the workspace's dependency graph stays as it was.

## T2 — The baseline

- [x] `Baseline` serialises to and from JSON, and carries the machine it was
      measured on, the significance level, the samples, the budgets, and the
      practical threshold.
- [x] The samples are the **raw measurements**, not a summary: a median would
      throw away exactly the information the test is about.
- [x] The baseline is committed: `bench-results/kernel-baseline.json`.
- [x] `KernelAb::measure_samples` collects them — one independent block per
      sample, each block the same shape as one A/B measurement.

## T3 — The verdicts

- [x] Four verdicts: no regression, within the noise, regression, inconclusive.
- [x] A regression is **both** statistically significant **and** slower by more
      than the practical threshold.
- [x] The budget is checked separately, and a budget miss fails the gate.
- [x] An inconclusive verdict is its own outcome, and is not reported as a pass.

## T4 — The pinned-runner rule

- [x] The gate refuses to compare a baseline measured on a different CPU,
      architecture, SIMD path or thread count, unless the caller overrides it.
- [x] The refusal names the differences.

## T5 — The threshold, measured

- [x] The first implementation was statistically correct and useless: **13 false
      positives in 20 checks** on unchanged code, with differences of 0.9% to
      3.9%.
- [x] The threshold is **5%**, above the observed drift.
- [x] With it, **five consecutive runs on unchanged code produce zero
      failures**, and the remaining differences are reported as *within the
      noise*.
- [x] An end-to-end check: a baseline claiming an 11% faster flavor makes the
      gate report a regression and exit non-zero.

## T6 — The command

- [x] `columnar-cli gate` measures the kernel and compares it against the
      baseline.
- [x] `columnar-cli gate --write-baseline` writes a new one. That is a
      deliberate act — the samples become the thing every later measurement is
      compared against — so it is a separate command.
- [x] `just perf-gate` and `just perf-baseline`.
- [x] The harness refuses to measure with the SIMD probe overridden, exactly as
      the A/B does.
- [x] The gate is **not** a `cargo test` and **not** a CI job. It measures the
      machine, and a shared runner is noisy.

## T7 — The tests

- [x] The exact test is pinned against published reference values:
      `[1,2,3]` against `[4,5,6]` gives U = 0 and a two-sided p of 0.1;
      `[1..5]` against `[6..10]` gives U = 0 and a two-sided p of 2/252.
- [x] A sample of only ties cannot be told from another.
- [x] The normal approximation is used for large samples, and a separated
      sample is decisive.
- [x] The JSON round-trips, and a malformed baseline is an error rather than a
      panic.
- [x] All four verdicts are covered, plus the budget miss and the
      different-machine refusal.

## T8 — Follow-ons, not done

- [ ] **The gate covers the kernel only.** The engine's query paths — the lazy
      scan, the filter, the aggregate — have no regression gate, because the
      baseline format is a per-flavor set of samples and the engine's paths are
      per-query. The same machinery would work; the shape of the baseline does
      not.
- [ ] **The baseline is from a shared machine.** The constitution says the
      budget gate runs on pinned runners only, and this machine is not one. The
      gate is honest about it — it records the machine and refuses across
      machines — but a pinned runner would make the gate meaningful in the way
      the constitution intends.
- [ ] **The gate is not in CI.** It cannot be, while the account's Actions
      minutes are blocked and the machine is shared. `just perf-gate` is the
      gate today.
