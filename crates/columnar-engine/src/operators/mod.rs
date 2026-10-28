//! The engine's operators.
//!
//! The operator set is deliberately small: a filter, a projection (which is
//! also where late materialisation lives), a hash aggregate, a hash join and
//! a sort.

pub mod aggregate;
pub mod expr;
pub mod filter;
pub mod join;
pub mod key;
pub mod project;
pub mod sort;

use columnar_kernels::Flavor;

pub use aggregate::{AggregateResult, AggregateValue, Group};

/// The name of a kernel flavor, as it appears in the plan output.
#[must_use]
pub const fn flavor_name(flavor: Flavor) -> &'static str {
    flavor.name()
}
