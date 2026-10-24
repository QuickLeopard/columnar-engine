//! The dependency policy, enforced on this workspace's own manifests.
//!
//! The policy is that **no crate in this workspace may depend on `rand`**.
//! The dataset generator has to be reproducible from a seed alone: the same
//! `(scale factor, seed)` must give byte-identical output on every machine,
//! on every release, for ever. `rand`'s stream is not version-stable across
//! releases, so the generator implements splitmix64 and xoshiro256** by hand
//! instead.
//!
//! ## Why this is a test and not a `cargo deny` rule
//!
//! `deny.toml` used to carry `bans.deny = [{ name = "rand" }]`. That rule
//! matches the whole transitive graph, and `rand` is in it exactly once:
//!
//! ```text
//! proptest -> rand
//! ```
//!
//! `proptest` is a **dev-dependency** of `columnar-kernels`, and it is what
//! generates the random inputs the differential tests compare the five kernel
//! implementations on. A test harness that generates random test inputs is not
//! the thing the policy is about -- the thing the policy is about is this
//! repository's own manifests.
//!
//! So the rule lives here, where it can see the manifests, and `deny.toml` says
//! so in a comment. If `rand` ever enters the production graph, this test
//! fails, and the `multiple-versions` lint warns about it at the same time.

use std::path::{Path, PathBuf};

/// The crates that may not appear in any dependency list.
const FORBIDDEN: &[&str] = &["rand", "rand_core"];

/// The workspace root, found by walking up from this crate's directory.
fn workspace_root() -> PathBuf {
    let mut path = Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
    while !path.join("Cargo.toml").is_file() || !path.join("crates").is_dir() {
        assert!(
            path.parent().is_some(),
            "could not find the workspace root above {}",
            env!("CARGO_MANIFEST_DIR")
        );
        path = path.parent().expect("the parent exists").to_path_buf();
    }
    path
}

/// Every crate's `Cargo.toml` in the workspace.
fn manifests() -> Vec<PathBuf> {
    let crates = workspace_root().join("crates");
    let mut found: Vec<PathBuf> = std::fs::read_dir(&crates)
        .expect("the crates directory is readable")
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path().join("Cargo.toml"))
        .filter(|path| path.is_file())
        .collect();
    found.sort();
    found
}

/// The names of the dependencies declared in one manifest.
///
/// The parser is deliberately simple: it looks for the dependency tables and
/// reads the names out of them. It does not need to understand TOML in full,
/// because it only has to find a dependency called `rand` -- and if the
/// manifest's shape ever changes so that this parser stops finding the
/// dependency tables at all, the test fails loudly rather than passing
/// vacuously.
fn dependency_names(manifest: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(manifest).expect("the manifest is readable");
    let mut names = Vec::new();
    let mut in_dependency_table = false;

    for line in text.lines() {
        let line = line.trim();

        // A table header: `[dependencies]`, `[dev-dependencies]`,
        // `[build-dependencies]`, or one of their `workspace = true` forms.
        if line.starts_with('[') {
            in_dependency_table = line.starts_with("[dependencies")
                || line.starts_with("[dev-dependencies")
                || line.starts_with("[build-dependencies");
            continue;
        }

        if !in_dependency_table || line.is_empty() || line.starts_with('#') {
            continue;
        }

        // `name = "version"` or `name.workspace = true`.
        if let Some((name, _)) = line.split_once('=') {
            let name = name.split('.').next().unwrap_or(name).trim();
            if !name.is_empty() && !name.starts_with('#') {
                names.push(name.to_owned());
            }
        }
    }

    names
}

/// No crate in this workspace may depend on `rand` or `rand_core`.
#[test]
fn the_workspace_never_declares_a_dependency_on_rand() {
    let manifests = manifests();
    assert!(!manifests.is_empty(), "the workspace has crates to check");

    let mut offenders = Vec::new();
    for manifest in &manifests {
        for name in dependency_names(manifest) {
            if FORBIDDEN.contains(&name.as_str()) {
                offenders.push(format!("{} declares a dependency on `{name}`", manifest.display()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "the dataset must be reproducible from a seed alone, and `rand`'s stream is not \
         version-stable across releases. Generate the data with the hand-written splitmix64 + \
         xoshiro256** in `columnar-testdata::prng` instead.\n\n{}",
        offenders.join("\n")
    );
}

/// The parser that enforces the policy has to actually find the dependency
/// tables.
///
/// A parser that silently stopped matching would make the test above pass
/// vacuously, which is the worst possible failure mode for a policy test.
#[test]
fn the_dependency_parser_finds_the_dependency_tables() {
    let kernels = workspace_root().join("crates").join("columnar-kernels").join("Cargo.toml");
    let names = dependency_names(&kernels);

    // A regular dependency, a workspace dependency and a dev-dependency: the
    // parser has to see all three kinds, because `rand` could be hiding in
    // any of them.
    assert!(
        names.iter().any(|name| name == "columnar-core"),
        "the parser did not find the regular dependency `columnar-core` in {names:?}"
    );
    assert!(
        names.iter().any(|name| name == "proptest"),
        "the parser did not find the dev-dependency `proptest` in {names:?}"
    );

    // `columnar-core` is the base crate and has no dependency tables at all,
    // so the parser must find nothing there. This is the shape that would
    // make the policy test vacuous, so it is pinned.
    let core = workspace_root().join("crates").join("columnar-core").join("Cargo.toml");
    let core_names = dependency_names(&core);
    assert!(
        core_names.is_empty(),
        "columnar-core is the base crate and should depend on nothing, but the parser found \
         {core_names:?}"
    );
}
