# ADR-0024: The repository goes public

**Status**: accepted
**Date**: 2026-10-27
**Related**: [ADR-0009](0009-avx2-without-x86-hardware.md), [ADR-0007](0007-comparator-pinning.md), [spec 017](../../specs/017-the-repository-goes-public/spec.md)

## Context

Two facts about this repository, one of them about the code and one about the repository.

**The repository carries three files that are not part of it.** `PORTFOLIO_PROJECTS_ANALYSIS.md`,
`PORTFOLIO_PROJECTS_DEEP_REVIEW.md` and `Portfolio_Projects_Blueprint.pdf` are a career-planning
document about *other* projects: their goals, their risks, their estimated line counts, and which
of them would survive a job interview. They were added in the **first commit**, `19e061e`, before
the engine had a single measurement, and they have been tracked ever since.

**The repository is private, and every GitHub Actions job fails before a runner starts.** The
account's spending limit is exhausted, and a private repository's runner minutes come out of that
limit; a public repository's do not. The consequence is not theoretical: `ci.yml` and
`bench-avx2.yml` were written, and neither can run. The project has been carrying its own gate by
hand — `just check` — and the AVX2 half of the kernel has never been executed on native x86-64
hardware, only emulated under Rosetta 2 on the author's ARM64 Mac.

The two facts have one answer.

## The decision

1. **The repository becomes public.** The `repo` scope of the credential in the keychain is
   sufficient to change the visibility; the `workflow` scope is sufficient to dispatch the
   workflows afterwards.
2. **The three files leave the repository — from the tree *and* from the history.** Removing them
   from the tree would leave them in every clone's history, and a public repository hands its
   history to anyone who asks for it. The history is rewritten and force-pushed. **26 commits, one
   branch, no tags, no other remotes, no other clone** — so the rewrite costs nothing that is not
   already paid.
3. **A copy is kept outside the repository**, at `/Volumes/My Data/Work/MiniMax Code/Portfolio
   Projects/`. The document is the author's and it is not deleted; it simply is not an engine's
   file.
4. **`LICENSE` (Apache-2.0) and `CONTRIBUTING.md` are added.** Every dependency in the lockfile is
   permissive, the C++ baseline in `crates/columnar-kernels/src/` is this repository's own work,
   and there is no vendored third-party source in the tree.
5. **The reports that name a commit are re-measured after the rewrite.** A rewrite changes every
   hash, and 34 committed reports name the commit they were taken at. A reader who cannot check
   that commit out cannot reproduce the number — which is the failure Principle V exists to
   prevent. The re-measurement is the ordinary campaign cycle this repository already runs; the
   only unusual part is that it happens after a rewrite rather than after a change.
6. **The x86-64 ratios are published as their own set**, derived by a checked-in script from three
   committed reports, with the runner's machine and conditions attached, and **never** compared
   with an ARM64 figure.

## Why the x86-64 numbers are ratios and nothing else

`bench-avx2.yml` already encodes the answer, and it is the right one. On a GitHub-hosted runner
the absolute rows/sec and ns/row describe a shared machine: they are not reproducible, and they are
not comparable with the M4 Max's. The **ratios** — `scalar → cpp-simd` and `cpp-auto → cpp-simd`
— are taken from the same process, on the same machine, in the same run, so whatever the runner's
neighbours do to the absolute numbers they do to all four flavours in the same proportion.

So the published x86-64 set is two ratios and a machine's description, and the repository says in
the same breath that the absolute figures are not published. The `avx2-self-hosted` job is already
wired for the day a pinned x86-64 runner exists, and it is the only job whose absolute figures
would be worth publishing.

## What this costs

- **A force-push.** The rewritten history replaces the old one on `origin/main`. Anyone else who
  had cloned the repository would have to re-clone; there is nobody else.
- **A campaign cycle.** The probes that name a commit are re-measured, so the published figures
  move. They are derived by `scripts/medians.py` and guarded by
  `scripts/check-published-figures.py`, exactly as before — the figures move, the derivation does
  not.
- **The old commit hashes stop resolving.** That is the point of the rewrite, and it is why the
  reports are re-measured rather than merely re-pointed.

## What this does not do

- It does not change the engine. Not one line of behaviour, not one published figure's *meaning*,
  not one plan. The engine's tree is byte-identical before and after.
- It does not publish a number it cannot reproduce. The x86-64 absolute figures stay unpublished.
- It does not compare two machines. The ARM64 set and the x86-64 set are two sets, each with its
  own machine in its conditions.
- It does not turn the portfolio document into anything. It leaves the repository; it is not
  rewritten as a README section, a site or a set of slides.
