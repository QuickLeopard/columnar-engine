# Spec 041 — The reader's four gigabytes

**Status**: open
**Date**: 2026-11-05
**Predecessor**: [spec 040 — the source that reads a set of files](../040-the-source-that-reads-a-set-of-files/spec.md)

## The problem

**A library is read by people who did not write it, and it reads files its users provide.** The
engine's own record says what a hostile file can ask of it: **"a hostile Parquet file can ask for
four gigabytes before a byte of payload is read"**, and it names the way — the compact protocol
allocates the declared length eagerly — and the fix: **`thrift` ≥ 0.23**, which `parquet` 54 does
not allow. That sentence is in three places: [spec 034's R10](../034-the-engine-as-a-library/spec.md),
[`STATUS.md`](../../STATUS.md)'s open questions, and [`deny.toml`](../../deny.toml)'s comment on the
advisory it exempts.

**The record is wrong in two ways, and the instrument is what says so.**

**The first wrongness is the location.** The hole is **not** in the footer's metadata. `parquet`
54.3.1 reads the footer through its **own slice protocol**, whose `read_bytes` asks the slice for
the declared range and refuses it when the bytes are not there — `self.buf.get(..len)` — **before**
it allocates anything. So a file whose footer declares a four-gigabyte string is refused by a
four-gigabyte *claim*, and the largest allocation the reader makes while opening it is **8 KiB**.
The metadata path is bounded, and it was bounded in the version the tree already ships.

**The second wrongness is where the hole really is.** It is in the **page headers**: the ones that
precede every page of data, read one after another as the reader walks a row group. Those go
through thrift's **streaming** compact protocol, over a `Read` rather than over a slice — and
thrift 0.17's `read_bytes` there does `vec![0u8; len]`: it **allocates the declared length before
it reads a byte of it**. The advisory's own sink — the *binary* protocol — is never constructed by
`parquet` at all, which `deny.toml` already says; the reachable one is the compact protocol, in
the page headers, and the record named the wrong file.

**And the third wrongness is the fix.** `thrift` ≥ 0.23 does refuse an over-long declared length —
**but only if the caller asks it to**. The fix is a **configurable limit**: `max_string_size`, and
the default configuration sets it to **none**. `parquet` 58.4.1 sets no limit either. So the bump
the record names would **not** have closed the hole.

**What does close it is a different thing entirely, and it is in the same crate.** `parquet`
**58.4.1** reads the metadata through its own slice-bounded reader — as 54.3.1 does — and its
reader **does not read the page statistics by default**: `DEFAULT_READ_PAGE_STATS` is `false`, and
the page header is then parsed *without* the statistics, so the hostile declared length is never
parsed at all. **54.3.1 has no such option**: its page-header path always reads them.

**And the bump is not free.** Arrow's decode is measured, so every published figure in this
repository moves when Arrow moves. The bump wants its own measurement, and it gets one: the hole
measured before and after, and every figure that moved published old and new.

## The requirements

### R1 — The hole is measured, on the tree as it stands

**The claim becomes a figure, and the figure is the largest single allocation the reader asks for.**

The probe writes a real Parquet file — one column, one row group, one page — and then replaces that
page's header with a **hostile one**: a hand-written Thrift compact header whose statistics field
declares a length of **four gigabytes**. The file is then opened and read through the **engine's
own reader**, and the probe records:

- the **largest single allocation** the reader asked for (a counting global allocator, so the
  number is the reader's and not the probe's);
- the **elapsed time** of the open and the read;
- **what the reader said** — the error, or the fact that it did not fail.

The declared length is not one number but a **curve**: 1 MiB, 64 MiB, 1 GiB, 4 GiB. **The cost
before the bump is the allocation**, and the curve is what shows it — the reader asks for what the
file says, so the largest allocation *is* the declared length. **Three campaigns, and the median**,
with the host's load recorded.

### R2 — The published claim is corrected where it was published

**Three places say the metadata is the hole and that `thrift` ≥ 0.23 is the fix.** Each is
corrected **in the place that published it** — spec 034's R10, `STATUS.md`'s open questions, and
`deny.toml`'s comment — to what the measurement says: the **page headers** are the hole, the
metadata path is bounded, and the fix is `parquet`'s own reader rather than thrift's opt-in limit.

**And a new ADR records the correction**, with the reason and the evidence, because a sentence that
was published as a finding and was wrong is exactly the kind of thing a reader must be able to
trace.

### R3 — The bump, and nothing but the bump

`parquet` **54.3.1 → 58.4.1** and `arrow` **54 → 58.4.1**, in the workspace's shared dependency
table — one line, so the version cannot drift between crates. **The churn the compiler finds is
fixed, and the engine's behaviour for an honest file does not change**: the same rows, the same
answers, the same plans.

### R4 — The hole is closed, and the closing is measured

**The same probe, the same hostile file, the same campaigns, on the bumped tree** — and the largest
allocation the reader asks for, which is the figure the requirement is judged by. **The hole is
closed when the reader asks for nothing like what the file declared**, and the figure says by how
much.

### R5 — Every figure that moves is published old and new

**Arrow's decode is measured, so the bump moves figures.** Every published figure that moves is
published **beside** the figure it replaces, with the reason, and **the code's share of every
difference is separated from the host's** — the machine's own floor is measured in the same
campaign, as every other comparison in this repository does it.

### R6 — The surface does not break

`cargo semver-checks` passes against the last published version. **The surface is still 46 names**,
the crate still packages in the order its dependencies decide, and **the publish still does not
wait** on anything this increment did.

### R7 — The dependency policy says what is true

`deny.toml`'s exemption of the `thrift` advisory is **kept or dropped for the reason that is true
after the bump**, and the comment beside it says which. **An exemption that no longer names a
crate in the graph is removed**, because an exemption nobody checks is a silence.

### R8 — The scope is named

**This increment is the dependency and the hole.** It adds **no name to the surface**, no query
capability, no operator, and no behaviour change for an honest file. **The store, the source over
a set of files, and the write side are untouched** except where the dependency bump reaches them —
and where it reaches them, the figures move and are published.

## The plan's shape

1. **The probe, first, and the before-figure on the tree as it stands.** The instrument is written
   and run **before** the bump, because a "before" measured after the bump is not a before.
2. **The correction**, in the three places that published the claim, and the ADR.
3. **The bump**, and the churn the compiler finds.
4. **The after-figure**, the same probe, the same campaigns.
5. **The figures that moved**, published old and new, in the sections that publish them, with the
   machine's own floor beside them.
6. **The closure**: the spec's status, and the record.

## The scope

| | |
|---|---|
| **In** | The hostile-file probe and its campaigns; the correction of the published claim; the `parquet`/`arrow` bump to 58.4.1; the churn it causes; the re-measurement of every published figure the bump moves; the dependency policy's exemption; the ADR. |
| **Out** | A new query capability, an operator, a name on the surface. A **writer** change: the store's bytes are the published file's, and the bump does not change what the store writes — or it does, and that is measured. A **new** protection of our own: the fix is upstream's, not a wrapper of ours. **The `parquet` 59/60 line**, which drops `thrift` outright: it is not needed, because 58.4.1 closes the hole, and it is a bigger jump than the evidence asks for. **Concurrency, compaction, MVCC** — out of scope of the store's half and untouched here. |

## The measurement

**The acceptance is a list, and every item on it is checked by a command.**

1. `python3 scripts/check-links.py` is green — the new documents resolve.
2. `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings` (both
   architectures), `cargo test --workspace --all-targets`, `cargo test --workspace --doc`,
   `cargo deny check`, `just check-wasm` — green.
3. `cargo semver-checks` passes: the surface did not break.
4. `./scripts/check-package.sh` — the four crates still package in the order their dependencies
   decide, and the store's dev-dependency is still path-only.
5. `python3 scripts/medians.py` and `python3 scripts/check-published-figures.py` — every published
   figure agrees with the committed reports, **including the figures this increment moved**.
6. **The probe's own campaigns**: three campaigns on each tree, the medians published beside the
   reports, and the hostile file committed as a **fixture the probe writes**, not as a binary.

## The references

- [`constitution.md`](../../constitution.md) I — one predicate, six implementations, one answer:
  the hostile file is answered the same way by every path the reader takes.
- [`constitution.md`](../../constitution.md) IV — evidence over claims: the finding is measured,
  not asserted.
- [`constitution.md`](../../constitution.md) V — the comparison is reproducible by a stranger: the
  probe writes the hostile file, so the stranger runs it.
- [`constitution.md`](../../constitution.md) VII — explicit scope, no silent deferral: what is out
  is named, with the reason.
- [`spec 034`](../034-the-engine-as-a-library/spec.md) R10 — the published finding, and the
  exclusion this increment is the answer to.
- [`STATUS.md`](../../STATUS.md) — the open question, and where the project stands.
- [`deny.toml`](../../deny.toml) — the advisory, the exemption, and the reason.
- [ADR-0046](../../docs/adr/0046-the-source-over-a-set-of-files.md) — the source over a set of
  files, the reader whose figures this bump moves.
- [ADR-0043](../../docs/adr/0043-the-write-side.md) — the write side, the other half of the
  figures.
