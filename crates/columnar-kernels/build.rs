//! Builds the C++ comparison kernels.
//!
//! There are two translation units, and the difference between them is the
//! whole point of the kernel A/B:
//!
//! | Translation unit | Flags | Exposed as | What it is |
//! |---|---|---|---|
//! | `comparison.cpp` | ordinary | `col_compare_*` (SIMD dispatch), `col_compare_*_auto` | the SIMD kernels, plus the scalar reference *as the compiler vectorises it* |
//! | `scalar.cpp` | `-fno-vectorize` | `col_compare_*_scalar` | the same scalar reference with vectorisation taken away |
//!
//! Both compile the same source (`scalar_impl.h`), so they compute the same
//! predicate and differ only in width. That makes the two ratios meaningful:
//!
//! - `scalar -> cpp-simd` answers "how much is vectorisation worth?"
//! - `cpp-auto -> cpp-simd` answers "does the hand-written kernel beat what
//!   the compiler does with the same loop?"
//!
//! The second question has no predetermined answer, and on this machine it
//! came out the other way. See ADR-0006.
//!
//! ## Why the AVX2 path is not a compile flag
//!
//! The C++ translation unit is compiled for the *baseline* ISA of the host,
//! never with `-mavx2`. The AVX2 path is reached through functions carrying
//! `__attribute__((target("avx2")))` and only after a runtime CPU probe.
//!
//! This matters more than it looks: a translation unit compiled with
//! `-mavx2` may emit AVX2 instructions anywhere, including in the scalar
//! fallback, so on a machine without AVX2 the process would fault before it
//! had a chance to detect the absence and fall back. The kernel would then
//! not be portable at all -- it would be an x86-64-with-AVX2-only kernel
//! wearing a runtime-dispatch hat.

fn main() {
    // Keep the C++ objects' deployment target in step with the Rust target's.
    //
    // Without this, `cc` builds the objects for the SDK's own version (27.0 on
    // this machine) while rustc links against the target's default minimum
    // (10.12 for x86_64, 11.0 for aarch64). The linker then warns that the
    // object "was built for newer 'macOS' version ... than being linked", and
    // the binary claims a deployment target it does not honour.
    //
    // Setting the environment variable is how `cc` is told what to do; it is
    // read when the compiler is invoked, which is below.
    if let Ok(target) = std::env::var("TARGET") {
        if target.ends_with("-apple-darwin") {
            let minimum = if target.starts_with("aarch64") { "11.0" } else { "10.12" };
            std::env::set_var("MACOSX_DEPLOYMENT_TARGET", minimum);
        }
    }

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .file("src/comparison.cpp")
        .flag_if_supported("-std=c++17")
        // -O3 is requested explicitly rather than left to the profile, because
        // the published kernel numbers are taken from release builds and the
        // C++ side must be optimised to the same degree as the Rust side.
        .flag_if_supported("-O3")
        .warnings(false);

    // Deliberately NOT set: `-mavx2`. See the module comment.
    build.compile("columnar_comparison");

    // The genuinely scalar baseline. Same source, vectoriser switched off.
    //
    // `-fno-vectorize` is the important flag; `-fno-slp-vectorize` is passed
    // as well so the baseline is scalar even on a compiler that only honours
    // the older spelling.
    let mut scalar = cc::Build::new();
    scalar
        .cpp(true)
        .file("src/scalar.cpp")
        .flag_if_supported("-std=c++17")
        .flag_if_supported("-O3")
        .flag_if_supported("-fno-vectorize")
        .flag_if_supported("-fno-slp-vectorize")
        .warnings(false);
    scalar.compile("columnar_scalar_novec");

    println!("cargo:rerun-if-changed=src/comparison.cpp");
    println!("cargo:rerun-if-changed=src/scalar.cpp");
    println!("cargo:rerun-if-changed=src/scalar_impl.h");
    println!("cargo:rerun-if-changed=src/comparison.h");
}
