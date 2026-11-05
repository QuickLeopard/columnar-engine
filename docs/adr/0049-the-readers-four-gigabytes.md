# ADR-0049 — The four gigabytes are in the page headers, and the fix is not thrift's

**Status**: accepted
**Date**: 2026-11-05
**Spec**: [041-the-readers-four-gigabytes](../../specs/041-the-readers-four-gigabytes/spec.md)

## Context

**The repository published a finding about hostile files, and the finding was wrong in three ways.**
It is in three places — [spec 034's R10](../../specs/034-the-engine-as-a-library/spec.md),
[`STATUS.md`](../../STATUS.md)'s open questions and [`deny.toml`](../../deny.toml)'s comment on the
advisory it exempts — and it says: *a hostile Parquet file's **metadata** can ask for four gigabytes
before a byte of payload is read; the fix is upstream (`thrift` ≥ 0.23), which `parquet` 54 does not
allow.*

**The measurement says the hole is real, and it is somewhere else.**

**The metadata path is bounded.** `parquet` reads the footer through its **own slice protocol**, whose
`read_bytes` asks the slice for the declared range and refuses it when the bytes are not there —
`self.buf.get(..len).ok_or(eof_error)?` — **before it allocates anything**. So a footer that declares
a four-gigabyte string is refused by a four-gigabyte *claim*, and the largest allocation the reader
makes while opening it is **8 KiB**: the same as an honest file's.

**The page headers are the reachable path.** They are read one after another as the reader walks a
row group, through thrift's **streaming** compact protocol — over a `Read`, not over a slice — and
there `read_bytes` does `vec![0u8; len]`: it **allocates the declared length before it reads a byte
of it**. The probe writes a real file with one hand-written page header and reads it through the
**engine's own reader**, and the largest single allocation the reader asks for is **the declared
length, exactly**: 1,048,576 at 1 MiB, 67,108,864 at 64 MiB, 1,073,741,824 at 1 GiB and
**4,294,967,295 at 4 GiB** — a share of **1.0** at every length, from a **25-byte** header. **And the
cost is address space, not time**: the read takes **0.089 ms**, less than the honest file's own
**0.300 ms**, because four gigabytes of untouched pages cost the reader almost nothing to ask for
and everything to carry.

**And the fix the record named would not have closed it.** `thrift` ≥ 0.23 refuses an over-long
declared length **only if the caller asks it to**: the limit is `max_string_size`, the default
configuration sets it to **none**, and `parquet` sets none either — in 54.3.1 or in 58.4.1. **A bump
that takes `thrift` ≥ 0.23 and nothing else leaves the hole open.**

**What closes it is `parquet`'s own reader, from 58.4.1.** It reads the metadata through a
slice-bounded reader of its own, and — the part that matters — **its reader does not read the page
statistics by default**: `DEFAULT_READ_PAGE_STATS` is `false`, and the page header is then parsed
*without* the statistics, so the hostile declared length is **never parsed at all**. `54.3.1` has no
such option: its page-header path always reads them.

**And the target the record named is stale.** The newest `parquet` is **60.1.0**, and 59.x and 60.x
**drop `thrift` altogether** — so they have their own protocol implementation, which is a bigger
jump than the evidence asks for. **58.4.1 is the smallest jump that closes the hole by default**, and
the hole is what this increment is about.

## Decision

1. **The published claim is corrected where it was published.** Spec 034's R10, `STATUS.md`'s open
   questions and `deny.toml`'s comment each say what the measurement says: the **page headers** are
   the hole, the metadata path is **bounded**, and the fix is `parquet`'s own reader rather than
   thrift's opt-in limit.

2. **The bump is to `parquet` 58.4.1 and `arrow` 58.4.1**, in the workspace's shared dependency table,
   so the version cannot drift between crates. **The evidence asks for the smallest jump that closes
   the hole, and 59.x and 60.x are a bigger jump than that** — a jump taken for tidiness rather than
   for the hole, and this repository does not take those.

3. **The fix is upstream's, and the engine does not wrap it.** The engine does not parse page
   headers, does not bound the file, and does not add a check of its own: **the reader is the place
   the hole is, and the reader is the place the fix is.**

4. **The figure is the largest single allocation the reader asked for**, measured with a counting
   global allocator around the engine's own reader, and the declared length is a **curve** — because
   one number cannot tell "it asks for what it is told" from "it asked for a lot once".

5. **The probe writes the hostile file**, so the finding is reproducible by a stranger: a real file
   with one hand-written page header, and the footer untouched so the reader walks it as it walks an
   honest one.

## Consequences

- **The hole is closed, and the closing is measured.** The same probe, the same hostile file and the
  same campaigns on the bumped tree: the largest allocation the reader asks for, beside the one it
  asked for before.

- **The `thrift` exemption in `deny.toml` is re-examined against the graph that is actually there.**
  The advisory names `thrift` 0.17.0, and the bump takes the graph's `thrift` to **0.23.0** — a
  version the advisory's criteria do not cover, so `cargo deny` reports the advisory as **not
  encountered**. An exemption that names nothing is a **silence**, so it is **removed**, with the
  reason.
  **And the reason this ADR first wrote was wrong**, and is corrected here: it said the bump takes
  `thrift` **out of the graph**, and it does not — `parquet` 58.4.1 depends on `thrift` 0.23.0.
  [ADR-0050](0050-the-bump-and-the-exemption-that-named-nothing.md) records the correction.

- **Arrow's decode is measured, so the bump moves the published figures.** Every figure that moves is
  published **beside** the figure it replaces, with the reason, and the machine's own floor is
  measured in the same campaign.

- **The engine's surface does not move**: `execute_on` still takes a `ParquetSource`, the surface is
  still 46 names, and the read path's semantics for an honest file are unchanged — which the
  repository's own differential suite says, row by row.

## References

- [`constitution.md`](../../constitution.md) IV — evidence over claims: the finding is measured, and
  the measurement corrected two published sentences.
- [`constitution.md`](../../constitution.md) VII — explicit scope, no silent deferral: what the bump
  does not buy is named.
- [`spec 034`](../../specs/034-the-engine-as-a-library/spec.md) R10 — the published finding, and the two
  sentences this ADR corrects.
- [`spec 041`](../../specs/041-the-readers-four-gigabytes/spec.md) — the reader's four gigabytes.
- [`PERF.md`](../../PERF.md) §3w — the measurement, and the figure it publishes.
- [`deny.toml`](../../deny.toml) — the advisory, the exemption, and the reason.
- [`STATUS.md`](../../STATUS.md) — the open question, and where the project stands.
- [ADR-0043](0043-the-write-side.md) — the write side, the other half of the figures.
- [ADR-0046](0046-the-source-over-a-set-of-files.md) — the source over a set of files,
  the reader whose figures this bump moves.
