//! The benchmark harness and the external comparison driver.
//!
//! Two things live here, and they are deliberately in the same crate:
//!
//! 1. [`kernel_ab`] — the A/B of the comparison kernel across the three
//!    flavors. This is the project's headline measurement.
//! 2. [`external`] — the `DuckDB` and `Polars` comparison, driven as CLI
//!    subprocesses against the *same* Parquet file the engine reads.
//!
//! They are in one crate because the comparison is only meaningful if both
//! sides are measured by the same harness, on the same machine, at the same
//! time. A kernel number produced by one harness and a `DuckDB` number produced
//! by another is a comparison of two harnesses.
//!
//! ## The framing rule
//!
//! Every result this crate produces reads **"kernel X on query Y"**, never
//! "this database versus `DuckDB`". `DuckDB` is a mature system and it wins
//! whole queries outright; the queries where it wins are published in the
//! same table as the ones where this engine's kernel wins. A reader who
//! cannot trust the loss will not trust the win either.
//!
//! ## Machine-readable output
//!
//! Every measurement is emitted as JSON ([`result::BenchReport`]). Prose
//! numbers are hand-copied and drift; a JSON artifact is diffable, and the
//! machine spec travels with the number.

#![deny(missing_docs)]
#![deny(unsafe_code)]
// The harness indexes small, locally-built vectors by construction -- the
// median of a sample, the nth element of a result. The lint stays enabled
// for the crates that build the engine itself.
#![allow(clippy::indexing_slicing)]

pub mod external;
pub mod gate;
pub mod kernel_ab;
pub mod machine;
pub mod result;
pub mod scaling;
pub mod size;

pub use external::{Comparator, ExternalComparison, ExternalQuery, Mode};
pub use kernel_ab::{KernelAb, KernelFlavorResult};
pub use machine::MachineSpec;
pub use result::BenchReport;
pub use scaling::{ScalingPoint, ScalingReport};
