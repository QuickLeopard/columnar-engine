//! The engine's operators.
//!
//! The operator set is deliberately small: a filter, a projection (which is
//! also where late materialisation lives) and a hash aggregate.

pub mod aggregate;
pub mod filter;
pub mod project;

use columnar_kernels::Flavor;

pub use aggregate::{AggregateResult, AggregateValue, Group};

/// A selection over every row of a batch.
///
/// The core's [`columnar_core::SelectionVector::all`] leaves the padding bits
/// of its last word set, so walking it visits rows past the end of the
/// batch. This builds the same selection the way the filter does -- through
/// [`columnar_core::SelectionVector::from_filter`] -- with the padding bits
/// cleared.
#[must_use]
pub fn all_rows(total: usize) -> columnar_core::SelectionVector {
    let mut words = vec![u64::MAX; total.div_ceil(64)];
    let remainder = total % 64;
    if remainder != 0 {
        if let Some(last) = words.last_mut() {
            *last &= (1u64 << remainder) - 1;
        }
    }
    columnar_core::SelectionVector::from_filter(&words, total, total)
}

/// The name of a kernel flavor, as it appears in the plan output.
#[must_use]
pub const fn flavor_name(flavor: Flavor) -> &'static str {
    flavor.name()
}
