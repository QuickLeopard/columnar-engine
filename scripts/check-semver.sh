#!/usr/bin/env bash
#
# check-semver.sh -- the public surface has not broken.
#
# # The problem
#
# A published crate's surface is the only thing in this repository that other
# code is allowed to depend on. A surface that breaks without anyone noticing
# breaks the **embedder**, not the author: the embedder's build fails on a
# crate they did not change, against a version they did not ask for. So the
# break has to fail **this** build, before it reaches theirs.
#
# # The baseline
#
# The baseline is the last **published** version of the crate -- that is the
# version an embedder depends on, and it is the one that must not break.
#
# The three crates are not published yet, and there is no published version to
# compare against. The recorded baseline -- `scripts/semver-baseline.txt`, the
# revision that established the surface -- stands in for it, and the check
# says which baseline it used.
#
# # What this script does NOT produce
#
# A version bump decision. `cargo semver-checks check-release` reports
# whether the API has broken; what to do about it -- a major version, or a
# deprecation -- is a decision for a person.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

CRATES=(columnar-core columnar-kernels columnar-engine)

baseline="$(grep -v '^#' scripts/semver-baseline.txt | grep -v '^[[:space:]]*$' | head -1 || true)"

for crate in "${CRATES[@]}"; do
    manifest="crates/$crate/Cargo.toml"
    echo "-- $crate"

    # The published baseline, when there is one.
    if output="$(cargo semver-checks check-release --manifest-path "$manifest" 2>&1)"; then
        echo "  the surface has not broken since the last published version"
        continue
    fi

    # No published version yet: the recorded baseline stands in for it.
    if [ -z "$baseline" ]; then
        echo "  no published version and no recorded baseline -- the check starts when there is one"
        continue
    fi
    echo "  the baseline is $baseline"
    cargo semver-checks check-release --manifest-path "$manifest" --baseline-rev "$baseline" \
        || { echo "$output" >&2; exit 1; }
done
