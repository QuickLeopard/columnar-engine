//! The performance gates.
//!
//! The constitution requires two of them, and neither existed:
//!
//! | Gate | Scope | Rule |
//! |---|---|---|
//! | Perf regression | workspace | No statistically significant regression vs. the stored baseline (Mann-Whitney, shared runners). |
//! | Perf budget | `columnar-kernels` | Absolute budgets met. **Pinned runners only.** |
//!
//! This module is the measurement and statistics half of both. The half that
//! actually runs the benchmark is a command, not a test, because it measures
//! the machine and a shared machine is noisy: a gate that fails at random is
//! a gate nobody trusts.
//!
//! # Why a test and not a comparison of medians
//!
//! The obvious gate is "is the new number lower than the old number". That
//! is not a gate, it is a coin flip: on a shared machine the noise is larger
//! than most real changes, so the gate either fails at random or has to be
//! loosened until it never fires.
//!
//! A statistical test asks a better question: *is the difference larger than
//! the machine's own spread?* The Mann-Whitney U test is the right one here
//! because it makes no assumption about the shape of the distribution -- the
//! throughput samples are not normal, they are a pile of timer resolutions --
//! and because it is exact for small samples, so a gate with eight
//! measurements in it is still a real test and not an approximation.
//!
//! # The three answers
//!
//! A gate that can only say "pass" or "fail" is a gate that will eventually
//! lie. This one has three answers, and the third one matters most:
//!
//! - **No regression** -- the current sample is not significantly slower.
//! - **Regression** -- the current sample *is* significantly slower.
//! - **Inconclusive** -- there were not enough samples to tell, or the
//!   baseline was measured on a different machine. This is **not** a pass,
//!   and it must never be reported as one.
//!
//! # The pinned-runner rule
//!
//! The constitution says the budget gate runs on pinned runners only, and the
//! reason is practical: a budget is an absolute number, and an absolute
//! number measured on a shared runner is not about the code.
//!
//! The baseline records the machine it was measured on, and the gate refuses
//! to compare across machines unless the caller says so explicitly. That
//! turns "pinned runners only" from a sentence in a document into something
//! the code enforces.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::machine::MachineSpec;

/// The error type for the gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateError {
    /// The baseline could not be parsed.
    Parse(String),
    /// The baseline and the current measurement are not comparable.
    NotComparable(String),
}

impl std::fmt::Display for GateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(reason) => write!(f, "the baseline could not be read: {reason}"),
            Self::NotComparable(reason) => write!(f, "the baseline is not comparable: {reason}"),
        }
    }
}

impl std::error::Error for GateError {}

// ---------------------------------------------------------------------------
// The statistics
// ---------------------------------------------------------------------------

/// The result of a Mann-Whitney U test.
#[derive(Debug, Clone, PartialEq)]
pub struct MannWhitney {
    /// The U statistic, the number of times the first sample's values are
    /// followed by the second sample's.
    pub u: f64,
    /// The one-sided p-value: the probability of a result at least this far
    /// in the direction "the first sample is the smaller one".
    ///
    /// This is the direction a *regression* lies in, so it is the number the
    /// gate decides on.
    pub p_one_sided: f64,
    /// The two-sided p-value.
    pub p_two_sided: f64,
    /// The size of the first sample.
    pub n1: usize,
    /// The size of the second sample.
    pub n2: usize,
    /// Whether the exact distribution was used, rather than the normal
    /// approximation.
    pub exact: bool,
}

/// The largest sample size for which the exact distribution is used.
///
/// The exact test enumerates the `C(n1 + n2, n1)` ways of splitting the
/// pooled sample, so the cost grows fast. At eight and eight that is 12,870
/// splits, which is nothing; at twelve and twelve it is 2,704,156, which is
/// no longer a gate but a wait.
const EXACT_LIMIT: usize = 8;

/// Compares two samples with the Mann-Whitney U test.
///
/// The question it answers is whether the first sample is *smaller* than the
/// second -- which, for a performance baseline, is the direction a regression
/// lies in. The two-sided p-value is reported as well, because a reader
/// looking at a suspicious result should be able to see whether the
/// difference is significant in either direction.
///
/// # Method
///
/// For small samples the **exact** permutation distribution is used: the
/// pooled values are ranked, every way of splitting them into two groups of
/// the right sizes is enumerated, and the p-value is the fraction of splits
/// at least as extreme as the observed one. For larger samples the **normal
/// approximation** is used, with two corrections that are not optional:
///
/// - a **tie correction**, because the throughput samples tie a great deal --
///   the timer has a resolution, and a benchmark that reports the same number
///   twice has produced a tie. Without the correction the variance is
///   over-estimated and the test loses exactly the power it is there for;
/// - a **continuity correction**, because the statistic is discrete and the
///   normal approximation is not.
///
/// # Examples
///
/// ```
/// use columnar_bench::gate::mann_whitney_u;
///
/// // Two separated samples: the first is clearly smaller.
/// let result = mann_whitney_u(&[1.0, 2.0, 3.0], &[4.0, 5.0, 6.0]);
/// assert_eq!(result.u, 0.0);
/// assert!((result.p_two_sided - 0.1).abs() < 1e-12);
/// ```
#[must_use]
pub fn mann_whitney_u(first: &[f64], second: &[f64]) -> MannWhitney {
    assert!(!first.is_empty(), "the first sample is empty");
    assert!(!second.is_empty(), "the second sample is empty");

    let n1 = first.len();
    let n2 = second.len();
    let pooled = Pooled::new(first, second);
    let observed_u = pooled.u_of_first(n1);

    if n1 <= EXACT_LIMIT && n2 <= EXACT_LIMIT {
        let (one_sided, two_sided) = pooled.exact_distribution(n1, n2, observed_u);
        return MannWhitney {
            u: observed_u,
            p_one_sided: one_sided,
            p_two_sided: two_sided,
            n1,
            n2,
            exact: true,
        };
    }

    let (one_sided, two_sided) = pooled.normal_approximation(n1, n2, observed_u, true);
    MannWhitney {
        u: observed_u,
        p_one_sided: one_sided,
        p_two_sided: two_sided,
        n1,
        n2,
        exact: false,
    }
}

/// The pooled sample, ranked.
///
/// Two samples are compared by pooling them, ranking the pool, and asking
/// how the first sample's ranks distribute. The pool is kept in *value*
/// order, because that is the order the permutation test enumerates in, and
/// each observation remembers which sample it came from.
struct Pooled {
    /// The rank of each pooled observation, in value order.
    ranks: Vec<f64>,
    /// Whether each pooled observation belongs to the first sample.
    is_first: Vec<bool>,
}

impl Pooled {
    /// Pools and ranks two samples.
    ///
    /// The `float_cmp` suggestion is wrong here, and deliberately overridden:
    /// a tie means *the same measurement*, so the comparison has to be on the
    /// stored bits. An epsilon comparison would merge two distinct
    /// measurements into one tie group, and the tie correction is exactly
    /// what turns the sample's spread into a variance -- so merging distinct
    /// measurements would change the test's answer, not just its tidiness.
    #[allow(clippy::float_cmp)]
    fn new(first: &[f64], second: &[f64]) -> Self {
        let mut pooled: Vec<(f64, bool)> = Vec::with_capacity(first.len() + second.len());
        for &value in first {
            pooled.push((value, true));
        }
        for &value in second {
            pooled.push((value, false));
        }
        pooled.sort_by(|left, right| {
            left.0.partial_cmp(&right.0).unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut ranks = vec![0.0; pooled.len()];
        let mut is_first = vec![false; pooled.len()];
        let mut index = 0;
        while index < pooled.len() {
            // Tied values share the average of the ranks they span.
            let mut end = index;
            while end < pooled.len() && pooled[end].0 == pooled[index].0 {
                end += 1;
            }
            let average = exact(index + 1 + end) / 2.0;
            for position in index..end {
                ranks[position] = average;
                is_first[position] = pooled[position].1;
            }
            index = end;
        }

        Self { ranks, is_first }
    }

    /// The U statistic of the observed split.
    fn u_of_first(&self, n1: usize) -> f64 {
        let rank_sum: f64 = self
            .ranks
            .iter()
            .zip(self.is_first.iter())
            .filter(|(_, first)| **first)
            .map(|(rank, _)| *rank)
            .sum();
        rank_sum - triangular(n1)
    }

    /// The exact permutation distribution.
    ///
    /// Every way of choosing which `n1` of the pooled observations belong to
    /// the first sample is enumerated. With both samples at most eight long
    /// that is at most 12,870 splits.
    fn exact_distribution(&self, n1: usize, n2: usize, observed_u: f64) -> (f64, f64) {
        let mut combinations = 0_usize;
        let mut one_sided = 0_usize;
        let mut two_sided = 0_usize;
        let maximum = exact(n1) * exact(n2);

        let mut chosen = Vec::with_capacity(n1);
        enumerate(self.ranks.len(), n1, 0, &mut chosen, &mut |indices: &[usize]| {
            let rank_sum: f64 = indices.iter().map(|&index| self.ranks[index]).sum();
            let u = rank_sum - triangular(n1);
            combinations += 1;
            if u <= observed_u {
                one_sided += 1;
            }
            if u <= observed_u || u >= maximum - observed_u {
                two_sided += 1;
            }
        });

        let denominator = exact(combinations.max(1));
        (exact(one_sided) / denominator, exact(two_sided) / denominator)
    }

    /// The normal approximation, with tie and continuity corrections.
    ///
    /// `tie_correction` exists so that the correction can be *shown* to
    /// matter, rather than asserted to. It is always `true` on the path the
    /// gate takes.
    /// The normal approximation, with tie and continuity corrections.
    ///
    /// The `float_cmp` suggestion is wrong here for the same reason it is
    /// wrong in [`Pooled::new`]: a tie group is a group of equal
    /// measurements.
    #[allow(clippy::float_cmp)]
    fn normal_approximation(
        &self,
        n1: usize,
        n2: usize,
        observed_u: f64,
        tie_correction: bool,
    ) -> (f64, f64) {
        let total = n1 + n2;
        let mean = exact(n1) * exact(n2) / 2.0;

        // The tie correction: the pooled sample's ties reduce the variance.
        let mut tie_sum = 0.0;
        let mut index = 0;
        while index < self.ranks.len() {
            let mut end = index;
            while end < self.ranks.len() && self.ranks[end] == self.ranks[index] {
                end += 1;
            }
            let count = exact(end - index);
            tie_sum += count * count * count - count;
            index = end;
        }
        let denominator = exact(total * (total - 1));
        let variance = if denominator > 0.0 {
            let reduction = if tie_correction { tie_sum / denominator } else { 0.0 };
            exact(n1) * exact(n2) / 12.0 * (exact(total + 1) - reduction)
        } else {
            0.0
        };

        if variance <= 0.0 {
            // Every value is tied: the samples cannot be distinguished at all.
            return (1.0, 1.0);
        }
        let sigma = variance.sqrt();

        // The continuity correction, one-sided: the observed U is nudged
        // towards the mean by half a step.
        let z_one_sided = (observed_u - mean + 0.5) / sigma;
        let one_sided = normal_cdf(z_one_sided);

        // Two-sided: the observed U is nudged away from the mean, and both
        // tails are counted.
        let direction = if observed_u >= mean { 1.0 } else { -1.0 };
        let z_two_sided = (observed_u - mean + direction * 0.5) / sigma;
        let two_sided = (2.0 * (1.0 - normal_cdf(z_two_sided.abs()))).min(1.0);

        (one_sided, two_sided)
    }
}

/// A sample size or a rank as an exact `f64`.
///
/// Sample sizes and ranks are far below 2^32, so the conversion is exact.
/// Going through `u32` rather than casting keeps the compiler's precision
/// lint honest: a `usize` cast would be lossy in principle, and this is not.
fn exact(value: usize) -> f64 {
    f64::from(u32::try_from(value).unwrap_or(u32::MAX))
}

/// `n * (n + 1) / 2`, exactly.
fn triangular(n: usize) -> f64 {
    exact(n.saturating_mul(n.saturating_add(1))) / 2.0
}

/// Enumerates every `n`-element subset of `0..len`, in order.
///
/// The indices of a subset are strictly increasing, so a subset is visited
/// once and only once.
fn enumerate<F: FnMut(&[usize])>(
    len: usize,
    n: usize,
    start: usize,
    current: &mut Vec<usize>,
    visit: &mut F,
) {
    if current.len() == n {
        visit(current);
        return;
    }
    let remaining = n - current.len();
    for index in start..=len - remaining {
        current.push(index);
        enumerate(len, n, index + 1, current, visit);
        current.pop();
    }
}

/// The standard normal cumulative distribution function.
///
/// The Abramowitz and Stegun 7.1.26 approximation of the error function,
/// whose absolute error is below 1.5e-7 -- far below the precision any
/// p-value in this file needs, and small enough that a reader does not have
/// to take the approximation on trust.
fn normal_cdf(x: f64) -> f64 {
    0.5 * (1.0 + error_function(x / std::f64::consts::SQRT_2))
}

/// The error function, by Abramowitz and Stegun 7.1.26.
fn error_function(x: f64) -> f64 {
    const A1: f64 = 0.254_829_592;
    const A2: f64 = -0.284_496_736;
    const A3: f64 = 1.421_413_741;
    const A4: f64 = -1.453_152_027;
    const A5: f64 = 1.061_405_429;
    const P: f64 = 0.327_591_1;

    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();
    let t = 1.0 / (1.0 + P * x);
    let polynomial = (((((A5 * t + A4) * t) + A3) * t + A2) * t + A1) * t;
    sign * (1.0 - polynomial * (-x * x).exp())
}

// ---------------------------------------------------------------------------
// The baseline
// ---------------------------------------------------------------------------

/// A stored performance baseline.
///
/// The samples are the raw measurements, not a summary: a median would throw
/// away exactly the information the statistical test is about.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Baseline {
    /// The machine the baseline was measured on.
    pub machine: MachineSpec,
    /// The significance level the gate decides at.
    pub alpha: f64,
    /// The number of samples per flavor, for the record.
    pub samples_per_flavor: usize,
    /// The samples, per flavor: rows per second.
    pub flavors: BTreeMap<String, Vec<f64>>,
    /// The absolute budgets, per flavor: the minimum rows per second.
    pub budgets: BTreeMap<String, f64>,
    /// The smallest relative slowdown that counts as a regression.
    ///
    /// A statistical test on enough samples will find that *something*
    /// changed, and on a shared machine something always has: the machine's
    /// own throughput drifts by a percent or two between one moment and the
    /// next, and a test with thirty-two samples on each side detects that
    /// drift as significant every time.
    ///
    /// So a regression has to be **both** statistically significant **and**
    /// practically significant. This is the practical half, and it is a
    /// measured number rather than a taste: five consecutive gate runs
    /// against an unchanged baseline reported differences of 0.9% to 3.9%,
    /// and this threshold sits above that.
    pub min_relative_regression: f64,
}

impl Baseline {
    /// Reads a baseline from JSON.
    ///
    /// # Errors
    ///
    /// Returns [`GateError::Parse`] if the JSON is malformed or does not
    /// describe a baseline.
    pub fn from_json(text: &str) -> Result<Self, GateError> {
        serde_json::from_str(text).map_err(|error| GateError::Parse(error.to_string()))
    }

    /// Writes the baseline as JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_owned())
    }

    /// The samples for one flavor.
    #[must_use]
    pub fn samples(&self, flavor: &str) -> Option<&[f64]> {
        self.flavors.get(flavor).map(Vec::as_slice)
    }

    /// Whether a baseline measured on `baseline`'s machine can be compared
    /// against a measurement taken on `current`'s machine.
    ///
    /// The CPU, the architecture, the SIMD path and the thread count all
    /// change what the numbers mean. The operating system version and the
    /// compiler version do not change the *machine*, and a reader is better
    /// served by a warning about those than by a refusal.
    #[must_use]
    pub fn is_comparable_with(&self, current: &MachineSpec) -> bool {
        self.machine.cpu == current.cpu
            && self.machine.arch == current.arch
            && self.machine.simd == current.simd
            && self.machine.threads == current.threads
    }

    /// The ways in which the machine differs from the current one.
    #[must_use]
    pub fn differences_from(&self, current: &MachineSpec) -> Vec<String> {
        let mut differences = Vec::new();
        if self.machine.cpu != current.cpu {
            differences.push(format!("cpu: {} vs {}", self.machine.cpu, current.cpu));
        }
        if self.machine.arch != current.arch {
            differences.push(format!("arch: {} vs {}", self.machine.arch, current.arch));
        }
        if self.machine.simd != current.simd {
            differences.push(format!("simd: {} vs {}", self.machine.simd, current.simd));
        }
        if self.machine.threads != current.threads {
            differences.push(format!("threads: {} vs {}", self.machine.threads, current.threads));
        }
        differences
    }
}

// ---------------------------------------------------------------------------
// The verdicts
// ---------------------------------------------------------------------------

/// What the gate concluded about one flavor.
#[derive(Debug, Clone, PartialEq)]
pub enum FlavorVerdict {
    /// The current sample is not significantly slower than the baseline.
    NoRegression {
        /// The flavour's name.
        flavor: String,
        /// The current median.
        current_median: f64,
        /// The baseline median.
        baseline_median: f64,
        /// The one-sided p-value.
        p_value: f64,
    },
    /// The current sample is significantly slower than the baseline.
    Regression {
        /// The flavour's name.
        flavor: String,
        /// The current median.
        current_median: f64,
        /// The baseline median.
        baseline_median: f64,
        /// The one-sided p-value.
        p_value: f64,
        /// How much slower, as a fraction.
        slower_by: f64,
    },
    /// The gate could not tell.
    Inconclusive {
        /// The flavour's name.
        flavor: String,
        /// Why it could not tell.
        reason: String,
    },
    /// The current sample is significantly slower, but by less than the
    /// practical threshold.
    ///
    /// This is not a failure and it is not a pass either. It says the
    /// machine was different, not that the code got slower, and reporting it
    /// as a pass would hide the fact that the machine drifted.
    WithinNoise {
        /// The flavour's name.
        flavor: String,
        /// The current median.
        current_median: f64,
        /// The baseline median.
        baseline_median: f64,
        /// The one-sided p-value.
        p_value: f64,
        /// How much slower, as a fraction.
        slower_by: f64,
    },
    /// The flavour is below its absolute budget.
    BudgetMiss {
        /// The flavour's name.
        flavor: String,
        /// The measured median.
        current_median: f64,
        /// The budget it missed.
        budget: f64,
    },
}

impl FlavorVerdict {
    /// The flavour's name.
    #[must_use]
    pub fn flavor(&self) -> &str {
        match self {
            Self::NoRegression { flavor, .. }
            | Self::Regression { flavor, .. }
            | Self::WithinNoise { flavor, .. }
            | Self::Inconclusive { flavor, .. }
            | Self::BudgetMiss { flavor, .. } => flavor,
        }
    }

    /// Whether this verdict should fail the gate.
    #[must_use]
    pub fn is_failure(&self) -> bool {
        matches!(self, Self::Regression { .. } | Self::BudgetMiss { .. })
    }
}

/// The gate's conclusion about the whole measurement.
#[derive(Debug, Clone, PartialEq)]
pub struct GateReport {
    /// One verdict per flavour.
    pub verdicts: Vec<FlavorVerdict>,
    /// Whether the baseline and the current measurement are comparable at
    /// all.
    pub comparable: bool,
    /// How the machines differ, if they do.
    pub differences: Vec<String>,
}

impl GateReport {
    /// Whether the gate failed.
    ///
    /// A regression or a budget miss fails the gate. An inconclusive verdict
    /// does **not** fail it -- it is not a pass either, and the caller is
    /// expected to say so out loud -- but it is not a failure, and calling it
    /// one would train the reader to ignore the gate.
    #[must_use]
    pub fn failed(&self) -> bool {
        self.verdicts.iter().any(FlavorVerdict::is_failure)
    }

    /// The flavours whose difference is statistically significant but below
    /// the practical threshold.
    #[must_use]
    pub fn within_noise(&self) -> Vec<&str> {
        self.verdicts
            .iter()
            .filter(|verdict| matches!(verdict, FlavorVerdict::WithinNoise { .. }))
            .map(FlavorVerdict::flavor)
            .collect()
    }

    /// The flavours whose verdict is inconclusive.
    #[must_use]
    pub fn inconclusive(&self) -> Vec<&str> {
        self.verdicts
            .iter()
            .filter(|verdict| matches!(verdict, FlavorVerdict::Inconclusive { .. }))
            .map(FlavorVerdict::flavor)
            .collect()
    }
}

/// The smallest number of samples the gate will decide on.
///
/// Below this the test is noise, and the gate says so rather than guessing.
pub const MINIMUM_SAMPLES: usize = 5;

/// Checks a current measurement against a baseline.
///
/// The current samples are rows per second, per flavour, and so are the
/// baseline's. A flavour is **slower** when its median is lower, so the test
/// is one-sided in the "current is smaller" direction.
///
/// # Errors
///
/// Returns [`GateError::NotComparable`] when the baseline was measured on a
/// different machine and `allow_different_machine` is false.
pub fn check(
    current: &BTreeMap<String, Vec<f64>>,
    baseline: &Baseline,
    current_machine: &MachineSpec,
    allow_different_machine: bool,
) -> Result<GateReport, GateError> {
    let differences = baseline.differences_from(current_machine);
    if !differences.is_empty() && !allow_different_machine {
        return Err(GateError::NotComparable(differences.join("; ")));
    }

    let mut verdicts = Vec::new();
    for (flavor, current_samples) in current {
        let Some(baseline_samples) = baseline.samples(flavor) else {
            verdicts.push(FlavorVerdict::Inconclusive {
                flavor: flavor.clone(),
                reason: "the baseline has no samples for this flavor".to_owned(),
            });
            continue;
        };

        if current_samples.len() < MINIMUM_SAMPLES || baseline_samples.len() < MINIMUM_SAMPLES {
            verdicts.push(FlavorVerdict::Inconclusive {
                flavor: flavor.clone(),
                reason: format!(
                    "too few samples to decide ({} current, {} baseline, {} needed)",
                    current_samples.len(),
                    baseline_samples.len(),
                    MINIMUM_SAMPLES
                ),
            });
            continue;
        }

        let current_median = median(current_samples);
        let baseline_median = median(baseline_samples);

        // The test is one-sided: a current sample that is *faster* is never a
        // regression, and the p-value says so without needing a second test.
        let test = mann_whitney_u(current_samples, baseline_samples);
        if test.p_one_sided < baseline.alpha {
            let slower_by = if baseline_median > 0.0 {
                (baseline_median - current_median) / baseline_median
            } else {
                0.0
            };
            if slower_by >= baseline.min_relative_regression {
                verdicts.push(FlavorVerdict::Regression {
                    flavor: flavor.clone(),
                    current_median,
                    baseline_median,
                    p_value: test.p_one_sided,
                    slower_by,
                });
            } else {
                verdicts.push(FlavorVerdict::WithinNoise {
                    flavor: flavor.clone(),
                    current_median,
                    baseline_median,
                    p_value: test.p_one_sided,
                    slower_by,
                });
            }
        } else {
            verdicts.push(FlavorVerdict::NoRegression {
                flavor: flavor.clone(),
                current_median,
                baseline_median,
                p_value: test.p_one_sided,
            });
        }
    }

    // The budgets are a separate question: an absolute floor, not a
    // comparison against the past.
    for (flavor, budget) in &baseline.budgets {
        let Some(current_samples) = current.get(flavor) else {
            continue;
        };
        if current_samples.is_empty() {
            continue;
        }
        let current_median = median(current_samples);
        if current_median < *budget {
            verdicts.push(FlavorVerdict::BudgetMiss {
                flavor: flavor.clone(),
                current_median,
                budget: *budget,
            });
        }
    }

    Ok(GateReport { verdicts, comparable: differences.is_empty(), differences })
}

/// The median of a sample.
///
/// For an even count it is the mean of the two middle values, which is the
/// usual convention and the one the baseline was written with.
#[must_use]
pub fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    } else {
        sorted[middle]
    }
}

#[cfg(test)]
mod tests {
    use super::Pooled;

    /// The tie correction is not decorative.
    ///
    /// The benchmark's samples tie constantly -- the timer has a resolution,
    /// and a benchmark that reports the same number twice has produced a tie.
    /// Tied observations carry less information than distinct ones: they
    /// compress the rank distribution, so the statistic's spread is narrower
    /// than the tie-free formula says, and the variance is over-estimated if
    /// the ties are ignored.
    ///
    /// An over-estimated variance makes the test *conservative*: the p-value
    /// comes out too large, and a real regression goes unnoticed. So the
    /// corrected p-value here is the **smaller** one, and this test requires
    /// the two paths to differ.
    #[test]
    fn the_tie_correction_changes_the_answer() {
        let mut current = vec![1.0; 30];
        current.extend(vec![2.0; 30]);
        let mut baseline = vec![1.0; 20];
        baseline.extend(vec![2.0; 40]);

        let pooled = Pooled::new(&current, &baseline);
        let n1 = current.len();
        let n2 = baseline.len();
        let observed = pooled.u_of_first(n1);

        let (corrected, _) = pooled.normal_approximation(n1, n2, observed, true);
        let (uncorrected, _) = pooled.normal_approximation(n1, n2, observed, false);

        assert!(
            corrected < uncorrected,
            "the tie correction did not change the p-value: {corrected} vs {uncorrected}"
        );
    }
}
