//! Forcing a SIMD path that the CPU feature probe would otherwise hide.
//!
//! # Why this exists
//!
//! The kernel reaches its SIMD path through a runtime CPU probe:
//! `__builtin_cpu_supports("avx2")` on the C++ side and
//! `is_x86_feature_detected!("avx2")` on the Rust side. That probe is the
//! right thing to do in production -- it is what lets one binary run on a
//! machine with AVX2 and on a machine without.
//!
//! But the probe reports the *host* CPU's feature bits, and that is exactly
//! what makes the AVX2 path unreachable on a machine this project has:
//!
//! - **Rosetta 2 on Apple Silicon executes AVX2 instructions correctly**, but
//!   it reports `avx2=0`, because the host is an ARM64 chip that has no AVX2
//!   units. The probe therefore hides a path that would in fact run.
//!
//! Without an override, the AVX2 path can only ever be exercised on real
//! x86-64 hardware. This module exists so that the path can be *correctness*
//! tested on an ARM64 Mac, using the same differential suite that tests it
//! everywhere else.
//!
//! # What it is not
//!
//! This is a **correctness-testing** switch. It is not a performance switch.
//!
//! - Under Rosetta 2, AVX2 instructions are *emulated*, so any timing taken
//!   with this override measures the emulator, not the hardware. Numbers
//!   produced that way must never be published.
//! - Forcing `avx2` on a real x86-64 machine that has no AVX2 raises
//!   `SIGILL`. That is the intended meaning of "force".
//!
//! The benchmark harness therefore refuses to report timings when an override
//! is in force, and `PERF.md` publishes the override's existence rather than
//! anything measured with it.
//!
//! # The variable
//!
//! `COLUMNAR_FORCE_SIMD` is read **once**, at first use.
//!
//! | value | meaning |
//! |---|---|
//! | unset (default) | use whatever the CPU probe finds |
//! | `none` | use no SIMD path; fall back to the scalar reference |
//! | `neon` | use the ARM NEON path |
//! | `avx2` | use the x86 AVX2 path |
//!
//! An override naming an ISA this binary was not compiled for is ignored by
//! the Rust side, and the C++ side keeps whatever its own architecture
//! provides. On an ARM64 build, setting `COLUMNAR_FORCE_SIMD` to `avx2` therefore leaves
//! the C++ path on NEON and takes the Rust intrinsics path off the table --
//! it never produces a wrong answer.

use std::sync::OnceLock;

/// The SIMD path the environment asks the kernel to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ForcedSimd {
    /// Use whatever the CPU feature probe finds. The default.
    Auto,
    /// Use no SIMD path; the scalar reference handles everything.
    None,
    /// Use the ARM NEON path.
    Neon,
    /// Use the x86 AVX2 path.
    Avx2,
}

impl ForcedSimd {
    /// A short, stable name for plan output and benchmark labels.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::None => "none",
            Self::Neon => "neon",
            Self::Avx2 => "avx2",
        }
    }
}

/// The environment variable that carries the override.
pub const ENV_VAR: &str = "COLUMNAR_FORCE_SIMD";

/// Parses the value of the override variable.
///
/// Anything unrecognised is [`ForcedSimd::Auto`]: a typo must not silently
/// disable the SIMD path on a machine that has one.
#[must_use]
pub fn parse(value: Option<&str>) -> ForcedSimd {
    match value {
        Some("none") => ForcedSimd::None,
        Some("neon") => ForcedSimd::Neon,
        Some("avx2") => ForcedSimd::Avx2,
        _ => ForcedSimd::Auto,
    }
}

/// The override currently in force.
///
/// The environment is read once and cached, because this is consulted from
/// the kernel's hot path.
#[must_use]
pub fn forced() -> ForcedSimd {
    static FORCED: OnceLock<ForcedSimd> = OnceLock::new();
    *FORCED.get_or_init(|| parse(std::env::var(ENV_VAR).ok().as_deref()))
}

/// Whether an override other than [`ForcedSimd::Auto`] is in force.
///
/// The benchmark harness uses this to refuse to publish a timing taken with
/// the probe overridden.
#[must_use]
pub fn is_forced() -> bool {
    forced() != ForcedSimd::Auto
}

#[cfg(test)]
mod tests {
    use super::{parse, ForcedSimd};

    #[test]
    fn an_unset_variable_means_auto() {
        assert_eq!(parse(None), ForcedSimd::Auto);
    }

    #[test]
    fn the_three_documented_values_parse() {
        assert_eq!(parse(Some("none")), ForcedSimd::None);
        assert_eq!(parse(Some("neon")), ForcedSimd::Neon);
        assert_eq!(parse(Some("avx2")), ForcedSimd::Avx2);
    }

    #[test]
    fn a_typo_does_not_silently_disable_simd() {
        // The dangerous failure mode is a misspelled variable quietly turning
        // the SIMD path off on a machine that has one.
        for typo in ["", "avx", "AVX2", "neon ", "yes", "0"] {
            assert_eq!(parse(Some(typo)), ForcedSimd::Auto, "{typo:?}");
        }
    }

    #[test]
    fn the_names_are_stable() {
        assert_eq!(ForcedSimd::Auto.name(), "auto");
        assert_eq!(ForcedSimd::None.name(), "none");
        assert_eq!(ForcedSimd::Neon.name(), "neon");
        assert_eq!(ForcedSimd::Avx2.name(), "avx2");
    }
}
