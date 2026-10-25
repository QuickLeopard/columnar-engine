//! The performance gates: the statistics, the baseline, and the verdicts.
//!
//! Everything here is deterministic and measures nothing. The half of the
//! gate that actually runs the benchmark is a command, not a test, because it
//! measures the machine and a shared machine is noisy.

use std::collections::BTreeMap;

use columnar_bench::gate::{
    check, mann_whitney_u, median, Baseline, FlavorVerdict, GateReport, MINIMUM_SAMPLES,
};
use columnar_bench::machine::MachineSpec;

/// A machine, for the comparability checks.
fn machine() -> MachineSpec {
    MachineSpec {
        cpu: "Apple M4 Max".to_owned(),
        arch: "aarch64".to_owned(),
        cores: 14,
        os: "macOS 27.0.1".to_owned(),
        simd: "neon".to_owned(),
        forced_simd: "auto".to_owned(),
        rustc: "rustc 1.97.1".to_owned(),
        threads: 1,
    }
}

/// A baseline over the given samples.
fn baseline_with(flavors: BTreeMap<String, Vec<f64>>) -> Baseline {
    Baseline {
        machine: machine(),
        alpha: 0.05,
        samples_per_flavor: 8,
        flavors,
        budgets: BTreeMap::new(),
        min_relative_regression: 0.05,
    }
}

/// Eight samples of a given value.
fn samples(value: f64) -> Vec<f64> {
    vec![value; 8]
}

#[test]
fn the_exact_test_agrees_with_the_published_small_sample_values() {
    // Two separated samples of three: the first is the smaller one in every
    // one of the C(6,3) = 20 ways to split the pooled sample, so the
    // one-sided p-value is 1/20 and the two-sided is 2/20.
    let result = mann_whitney_u(&[1.0, 2.0, 3.0], &[4.0, 5.0, 6.0]);

    assert!(result.u.abs() < 1e-12, "the U statistic is {}", result.u);
    assert!(result.exact, "a small sample must use the exact distribution");
    assert!(
        (result.p_one_sided - 0.05).abs() < 1e-12,
        "the one-sided p-value is {}",
        result.p_one_sided
    );
    assert!(
        (result.p_two_sided - 0.1).abs() < 1e-12,
        "the two-sided p-value is {}",
        result.p_two_sided
    );
}

#[test]
fn the_exact_test_agrees_with_the_published_five_by_five_values() {
    // C(10,5) = 252 splits; one puts the five smallest in the first sample
    // and one puts the five largest, so the two-sided p-value is 2/252.
    let result = mann_whitney_u(&[1.0, 2.0, 3.0, 4.0, 5.0], &[6.0, 7.0, 8.0, 9.0, 10.0]);

    assert!(result.u.abs() < 1e-12, "the U statistic is {}", result.u);
    assert!(result.exact, "a small sample must use the exact distribution");
    assert!(
        (result.p_two_sided - 2.0 / 252.0).abs() < 1e-12,
        "the two-sided p-value is {}",
        result.p_two_sided
    );
}

#[test]
fn a_sample_of_only_ties_cannot_be_told_from_another() {
    let result = mann_whitney_u(&[1.0; 5], &[1.0; 5]);

    // Every value is tied, so every one of the 252 splits gives the same U:
    // the middle of the distribution, n1 * n2 / 2.
    assert!((result.u - 12.5).abs() < 1e-12, "the U statistic is {}", result.u);
    assert!(
        (result.p_one_sided - 1.0).abs() < 1e-12,
        "the one-sided p-value is {}",
        result.p_one_sided
    );
    assert!(
        (result.p_two_sided - 1.0).abs() < 1e-12,
        "the two-sided p-value is {}",
        result.p_two_sided
    );
}

#[test]
fn the_normal_approximation_is_used_for_large_samples() {
    let first: Vec<f64> = (1..=40).map(f64::from).collect();
    let second: Vec<f64> = (60..=99).map(f64::from).collect();
    let result = mann_whitney_u(&first, &second);

    assert!(!result.exact, "a large sample must not use the exact distribution");
    assert!(result.u.abs() < 1e-12, "the U statistic is {}", result.u);
    // The two samples do not overlap at all, so the p-value is far below any
    // significance level -- and, computed independently, it is about
    // 1.3e-14. The assertion is loose because the normal approximation is
    // an approximation; what matters is that the answer is decisive.
    assert!(result.p_one_sided < 1e-10, "the one-sided p-value is {}", result.p_one_sided);
    assert!(result.p_two_sided < 1e-10, "the two-sided p-value is {}", result.p_two_sided);
}

#[test]
fn a_slower_sample_is_significantly_slower() {
    let current: Vec<f64> = (1..=20).map(f64::from).collect();
    let baseline: Vec<f64> = (100..=119).map(f64::from).collect();
    let result = mann_whitney_u(&current, &baseline);

    assert!(
        result.p_one_sided < 0.05,
        "a separated sample must be significant: {}",
        result.p_one_sided
    );
}

#[test]
fn a_faster_sample_is_not_slower() {
    // The other direction: the first sample is the larger one.
    let current: Vec<f64> = (100..=119).map(f64::from).collect();
    let baseline: Vec<f64> = (1..=20).map(f64::from).collect();
    let result = mann_whitney_u(&current, &baseline);

    assert!(
        result.p_one_sided > 0.95,
        "the other direction must not be significant: {}",
        result.p_one_sided
    );
}

#[test]
fn the_median_is_the_middle_value() {
    assert!((median(&[3.0, 1.0, 2.0]) - 2.0).abs() < 1e-12);
    assert!((median(&[4.0, 1.0, 3.0, 2.0]) - 2.5).abs() < 1e-12);
    assert!((median(&[]) - 0.0).abs() < 1e-12);
}

#[test]
fn a_baseline_survives_a_json_round_trip() {
    let mut flavors = BTreeMap::new();
    flavors.insert("scalar".to_owned(), samples(3.5e9));
    flavors.insert("cpp-simd".to_owned(), samples(8.8e9));
    let mut budgets = BTreeMap::new();
    budgets.insert("scalar".to_owned(), 2.0e9);

    let baseline = Baseline {
        machine: machine(),
        alpha: 0.05,
        samples_per_flavor: 8,
        flavors,
        budgets,
        min_relative_regression: 0.05,
    };

    let parsed = Baseline::from_json(&baseline.to_json()).expect("the baseline round-trips");
    assert_eq!(parsed, baseline);
}

#[test]
fn a_malformed_baseline_is_an_error_and_not_a_panic() {
    let result = Baseline::from_json("{not json");
    assert!(result.is_err(), "a malformed baseline must be an error");
}

#[test]
fn a_sample_that_is_not_slower_is_not_a_regression() {
    let mut flavors = BTreeMap::new();
    flavors.insert("scalar".to_owned(), samples(3.5e9));
    let baseline = baseline_with(flavors);

    let mut current = BTreeMap::new();
    current.insert("scalar".to_owned(), samples(3.6e9));

    let report = check(&current, &baseline, &machine(), false).expect("the check runs");
    assert!(!report.failed(), "a faster sample must not fail the gate: {report:?}");
    assert!(matches!(report.verdicts.first(), Some(FlavorVerdict::NoRegression { .. })));
}

#[test]
fn a_sample_that_is_significantly_slower_is_a_regression() {
    let mut flavors = BTreeMap::new();
    flavors.insert("scalar".to_owned(), samples(3.5e9));
    let baseline = baseline_with(flavors);

    // A clear regression: every current sample is below every baseline one.
    let mut current = BTreeMap::new();
    current.insert("scalar".to_owned(), samples(2.0e9));

    let report = check(&current, &baseline, &machine(), false).expect("the check runs");
    assert!(report.failed(), "a slower sample must fail the gate: {report:?}");
    match report.verdicts.first() {
        Some(FlavorVerdict::Regression { slower_by, .. }) => {
            assert!(
                (*slower_by - (3.5e9 - 2.0e9) / 3.5e9).abs() < 1e-9,
                "the slowdown is reported as {slower_by}"
            );
        }
        other => panic!("expected a regression, got {other:?}"),
    }
}

/// A difference that is significant but smaller than the practical threshold
/// is neither a regression nor a pass.
///
/// This is the verdict that keeps the gate honest on a shared machine. Five
/// consecutive gate runs against an unchanged baseline reported differences
/// of 0.9% to 3.9% -- statistically significant every time, because the test
/// has thirty-two samples on each side -- and none of them was a change to
/// the code. A gate that fired on those would be a gate nobody trusts, so the
/// practical threshold is a second, independent condition.
#[test]
fn a_small_significant_difference_is_within_the_noise() {
    let mut flavors = BTreeMap::new();
    flavors.insert("scalar".to_owned(), samples(3.5e9));
    let baseline = baseline_with(flavors);

    // One per cent slower: significant at this sample size, and below the
    // five per cent threshold.
    let mut current = BTreeMap::new();
    current.insert("scalar".to_owned(), samples(3.465e9));

    let report = check(&current, &baseline, &machine(), false).expect("the check runs");
    assert!(!report.failed(), "a one per cent difference is not a failure: {report:?}");
    assert_eq!(
        report.within_noise(),
        vec!["scalar"],
        "the difference should be within the noise: {report:?}"
    );
    assert!(
        matches!(report.verdicts.first(), Some(FlavorVerdict::WithinNoise { .. })),
        "the verdict is wrong: {report:?}"
    );
}

/// A difference above the practical threshold is a regression even if the
/// machine's noise could have produced it.
#[test]
fn a_large_difference_is_a_regression() {
    let mut flavors = BTreeMap::new();
    flavors.insert("scalar".to_owned(), samples(3.5e9));
    let baseline = baseline_with(flavors);

    // Ten per cent slower: above the five per cent threshold.
    let mut current = BTreeMap::new();
    current.insert("scalar".to_owned(), samples(3.15e9));

    let report = check(&current, &baseline, &machine(), false).expect("the check runs");
    assert!(report.failed(), "a ten per cent slowdown must fail the gate: {report:?}");
    assert!(
        report.within_noise().is_empty(),
        "a ten per cent slowdown is not within the noise: {report:?}"
    );
}

#[test]
fn too_few_samples_are_inconclusive_and_not_a_pass() {
    let mut flavors = BTreeMap::new();
    flavors.insert("scalar".to_owned(), samples(3.5e9));
    let baseline = baseline_with(flavors);

    // Three samples: below the five the gate needs to decide.
    let mut current = BTreeMap::new();
    current.insert("scalar".to_owned(), vec![1.0e9, 1.1e9, 0.9e9]);

    let report = check(&current, &baseline, &machine(), false).expect("the check runs");
    assert!(!report.failed(), "too few samples is not a failure: {report:?}");
    assert_eq!(
        report.inconclusive(),
        vec!["scalar"],
        "too few samples must be inconclusive, not a pass: {report:?}"
    );
    assert_eq!(MINIMUM_SAMPLES, 5);
}

#[test]
fn a_flavor_below_its_budget_misses_the_budget() {
    let mut flavors = BTreeMap::new();
    flavors.insert("scalar".to_owned(), samples(3.5e9));
    let mut baseline = baseline_with(flavors);
    baseline.budgets.insert("scalar".to_owned(), 3.0e9);

    let mut current = BTreeMap::new();
    current.insert("scalar".to_owned(), samples(2.5e9));

    let report = check(&current, &baseline, &machine(), false).expect("the check runs");
    assert!(report.failed(), "a budget miss must fail the gate: {report:?}");
    assert!(
        report.verdicts.iter().any(|verdict| matches!(verdict, FlavorVerdict::BudgetMiss { .. })),
        "the budget miss was not reported: {report:?}"
    );
}

#[test]
fn a_flavor_above_its_budget_meets_it() {
    let mut flavors = BTreeMap::new();
    flavors.insert("scalar".to_owned(), samples(3.5e9));
    let mut baseline = baseline_with(flavors);
    baseline.budgets.insert("scalar".to_owned(), 3.0e9);

    let mut current = BTreeMap::new();
    current.insert("scalar".to_owned(), samples(3.5e9));

    let report = check(&current, &baseline, &machine(), false).expect("the check runs");
    assert!(!report.failed(), "the budget was met: {report:?}");
}

#[test]
fn a_baseline_from_another_machine_is_refused() {
    let mut flavors = BTreeMap::new();
    flavors.insert("scalar".to_owned(), samples(3.5e9));
    let mut baseline = baseline_with(flavors);
    baseline.machine.cpu = "Intel Xeon Platinum".to_owned();

    let mut current = BTreeMap::new();
    current.insert("scalar".to_owned(), samples(3.5e9));

    let result = check(&current, &baseline, &machine(), false);
    assert!(result.is_err(), "a baseline from another machine must be refused");

    // And it is refused for the right reason.
    let report = check(&current, &baseline, &machine(), true).expect("the override works");
    assert!(!report.comparable, "the machines differ: {report:?}");
    assert!(
        report.differences.iter().any(|difference| difference.contains("cpu")),
        "the difference was not named: {:?}",
        report.differences
    );
}

#[test]
fn a_flavor_the_baseline_does_not_have_is_inconclusive() {
    let mut flavors = BTreeMap::new();
    flavors.insert("scalar".to_owned(), samples(3.5e9));
    let baseline = baseline_with(flavors);

    let mut current = BTreeMap::new();
    current.insert("a-new-flavor".to_owned(), samples(1.0e9));

    let report: GateReport = check(&current, &baseline, &machine(), false).expect("the check runs");
    assert_eq!(report.inconclusive(), vec!["a-new-flavor"]);
}
