//! The machine spec, recorded alongside every number.
//!
//! A performance number without the machine it was measured on is a claim.
//! This module captures the machine -- CPU model, core count, architecture,
//! the SIMD path in use, the toolchain and the OS -- so that the number and
//! its conditions travel together in the JSON artifact.

use std::process::Command;

/// The conditions a measurement was taken under.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MachineSpec {
    /// The CPU model, as the OS reports it.
    pub cpu: String,
    /// The machine architecture, e.g. `arm64` or `x86_64`.
    pub arch: String,
    /// The number of logical cores.
    pub cores: usize,
    /// The operating system version string.
    pub os: String,
    /// The SIMD path that the kernel selected at runtime.
    pub simd: String,
    /// The Rust compiler version.
    pub rustc: String,
    /// The thread count the measurement used.
    pub threads: usize,
}

impl MachineSpec {
    /// Captures the current machine.
    ///
    /// `threads` is the thread count the measurement will use; it is recorded
    /// here because a number measured with one thread and a number measured
    /// with eight are not comparable, and a reader should not have to guess.
    #[must_use]
    pub fn capture(threads: usize) -> Self {
        Self {
            cpu: cpu_model(),
            arch: std::env::consts::ARCH.to_owned(),
            cores: core_count(),
            os: os_version(),
            simd: columnar_kernels::simd_kind().name().to_owned(),
            rustc: rustc_version(),
            threads,
        }
    }
}

/// The CPU model, as the OS reports it.
fn cpu_model() -> String {
    #[cfg(target_os = "macos")]
    {
        if let Some(value) = run("sysctl", &["-n", "machdep.cpu.brand_string"]) {
            return value;
        }
    }
    if let Ok(contents) = std::fs::read_to_string("/proc/cpuinfo") {
        for line in contents.lines() {
            if let Some(rest) = line.strip_prefix("model name") {
                if let Some(value) = rest.split(':').nth(1) {
                    return value.trim().to_owned();
                }
            }
        }
    }
    "unknown".to_owned()
}

/// The number of logical cores.
fn core_count() -> usize {
    #[cfg(target_os = "macos")]
    {
        if let Some(value) = run("sysctl", &["-n", "hw.ncpu"]) {
            if let Ok(count) = value.trim().parse::<usize>() {
                return count;
            }
        }
    }
    std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
}

/// The operating system version.
fn os_version() -> String {
    #[cfg(target_os = "macos")]
    {
        if let Some(value) = run("sw_vers", &["-productVersion"]) {
            return format!("macOS {value}");
        }
    }
    if let Some(value) = run("uname", &["-sr"]) {
        return value;
    }
    "unknown".to_owned()
}

/// The Rust compiler version.
fn rustc_version() -> String {
    run("rustc", &["--version"]).unwrap_or_else(|| "unknown".to_owned())
}

/// Runs a command and returns its trimmed stdout.
fn run(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
