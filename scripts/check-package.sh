#!/usr/bin/env bash
#
# check-package.sh -- the four crates package, in the order their
# dependencies decide, and what they carry is what an embedder needs.
#
# # The problem
#
# A crate that cannot be packaged is a crate nobody can depend on. The four
# published crates here depend on each other -- `columnar-engine` depends on
# `columnar-core` and `columnar-kernels` -- and cargo resolves a packaged
# crate's dependencies from crates.io, not from the workspace. So the crates
# package in the order the dependency graph decides: `columnar-core`, then
# `columnar-kernels`, then `columnar-store`, then `columnar-engine`. The
# order is not a convention here; it is what the dependency graph decides.
#
# And the store is the one the graph does not place: it depends on **nothing
# in the workspace**, so it is ready beside the core. **Where the graph does
# not decide, the list does** -- and the list is written in the publish
# order.
#
# The second thing a package gets wrong is what it *carries*. A research
# harness that ships as a library is a library with fourteen measurement
# probes in it, a dataset in it, and a portfolio's front page where the
# crate's documentation should be.
#
# # What this script checks
#
# 1. **The order.** The publish order is *computed* from the manifests, not
#    written down here: every internal dependency is read out of the
#    manifests, and the order is the topological sort of them. **Where the
#    graph does not decide -- a crate that depends on nothing in the
#    published set is ready beside the crate that depends on nothing -- the
#    list decides**, and the list is the publish order for those ties.
# 2. **The tree.** For each crate, `cargo package --list` prints the files
#    that would ship. The tree must hold the crate's **own** README, its
#    `src/`, and a licence file -- and it must hold **no probe, no dataset
#    and no report**.
# 3. **The package.** Each crate is packaged. A crate whose sibling is not on
#    crates.io yet cannot be packaged, and cargo says so; that refusal is
#    checked for exactly, and the crate is reported as *ready, waiting for
#    its sibling*. Any other failure is a failure.
# 4. **The dry run.** `cargo publish --dry-run` for each crate, with the
#    same treatment.
#
# # What this script does NOT produce
#
# A crates.io publish. Nothing here uploads anything, and nothing here needs
# a token. The publish itself is a separate act, with its own account, and
# the guide (`docs/embedding.md`) says in what order it happens and with
# what commands.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# The crates that are published. The publish order is computed below, from the
# manifests -- and where the manifests do not decide, **this list does**: it
# is written in the publish order, so the crate that depends on nothing in
# the workspace lands between the kernels and the engine.
PUBLISHED=(columnar-core columnar-kernels columnar-store columnar-engine)

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

note() {
    echo "  $*"
}

# --- the publish order, computed ------------------------------------------
#
# Every internal dependency is read out of the manifests, and the order is
# the topological sort. The graph is acyclic -- core depends on nothing,
# kernels depends on core, engine depends on both, and the store depends on
# nothing in the workspace -- and where it does not decide, the list does. A
# cycle, or a crate that depends on something outside the published set, fails
# here.

echo "== the publish order, computed from the manifests"

order="$(python3 - "${PUBLISHED[@]}" <<'PY'
import re
import sys

crates = sys.argv[1:]
dependencies = {}
for crate in crates:
    text = open(f"crates/{crate}/Cargo.toml").read()
    found = []
    for section in re.split(r"^\[", text, flags=re.M):
        if section.startswith("dependencies]"):
            for line in section.splitlines():
                match = re.match(r"^(columnar-[a-z]+)\s*=", line)
                if match:
                    found.append(match.group(1))
    dependencies[crate] = found

# A dependency outside the published set cannot be published after it.
for crate, deps in dependencies.items():
    for dep in deps:
        if dep not in crates:
            raise SystemExit(f"{crate} depends on {dep}, which is not published")

order = []
remaining = list(crates)
while remaining:
    ready = [c for c in remaining if all(d in order for d in dependencies[c])]
    if not ready:
        raise SystemExit(f"the dependency graph has a cycle among {remaining}")
    # Where the graph does not decide, the list does. The list is written in
    # the publish order, so the tie is broken the way the publish is meant to
    # happen -- and the graph still decides everything it can.
    ready.sort(key=crates.index)
    order.append(ready[0])
    remaining.remove(ready[0])

for crate in order:
    after = " ".join(dependencies[crate])
    print(f"{crate}\t{after}")
PY
)" || fail "the publish order could not be computed"

publish_order=()
while IFS=$'\t' read -r crate after; do
    publish_order+=("$crate")
    if [ -n "$after" ]; then
        note "$crate (after: $after)"
    else
        note "$crate (after: nothing)"
    fi
done <<< "$order"

# --- the trees -------------------------------------------------------------

echo
echo "== the tree each crate ships"

for crate in "${publish_order[@]}"; do
    echo "-- $crate"
    tree="$(cargo package -p "$crate" --list --allow-dirty 2>/dev/null)" \
        || fail "$crate: the manifest does not list"

    # The crate's own README, and not the repository's.
    echo "$tree" | grep -qx "README.md" \
        || fail "$crate: the package does not carry a README.md"
    if cmp -s "crates/$crate/README.md" "README.md"; then
        fail "$crate: the packaged README is the repository's README"
    fi
    note "the README is the crate's own"

    # The source, and the licence.
    echo "$tree" | grep -q "^src/" || fail "$crate: the package does not carry its src/"
    echo "$tree" | grep -qi "^LICENSE" || fail "$crate: the package does not carry a licence file"
    note "it carries its src/ and its licence"

    # And nothing it should not.
    if echo "$tree" | grep -q "^examples/"; then
        fail "$crate: the package carries the measurement probes"
    fi
    if echo "$tree" | grep -q "\.parquet$"; then
        fail "$crate: the package carries a dataset"
    fi
    if echo "$tree" | grep -q "^bench-results/"; then
        fail "$crate: the package carries a benchmark report"
    fi
    if echo "$tree" | grep -q "^benches/"; then
        fail "$crate: the package carries the benchmark harness"
    fi
    note "it carries no probe, no dataset and no report"
done

# --- the packages ----------------------------------------------------------

echo
echo "== the packages"

for crate in "${publish_order[@]}"; do
    echo "-- $crate"
    if output="$(cargo package -p "$crate" --allow-dirty --no-verify 2>&1)"; then
        note "packaged"
        continue
    fi
    # The one failure this script expects: the crate's sibling is not on
    # crates.io yet. That refusal is the proof of the publish order -- the
    # crate is ready and it is waiting for its sibling.
    if echo "$output" | grep -q "no matching package named \`columnar-[a-z-]*\` found"; then
        waiting="$(echo "$output" | sed -n 's/.*no matching package named `\([a-z-]*\)` found.*/\1/p' | head -1)"
        note "ready, and waiting for $waiting on crates.io"
        continue
    fi
    echo "$output" >&2
    fail "$crate: the package did not build"
done

# --- the dry runs ----------------------------------------------------------

echo
echo "== the dry runs"

for crate in "${publish_order[@]}"; do
    echo "-- $crate"
    if output="$(cargo publish -p "$crate" --dry-run --allow-dirty 2>&1)"; then
        note "the dry run passed"
        continue
    fi
    if echo "$output" | grep -q "no matching package named \`columnar-[a-z-]*\` found"; then
        waiting="$(echo "$output" | sed -n 's/.*no matching package named `\([a-z-]*\)` found.*/\1/p' | head -1)"
        note "ready, and waiting for $waiting on crates.io"
        continue
    fi
    echo "$output" >&2
    fail "$crate: the dry run failed"
done

echo
echo "The four crates are ready to publish, in the order above."
echo "The publish itself is a separate act: see docs/embedding.md."
