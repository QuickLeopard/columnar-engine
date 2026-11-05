# Spec 037 — The write side

**Status**: open
**Started**: 2026-11-05

## The problem

Every published figure in this repository is a figure about **reading**. The scan's decode, the join's
probe, the sort's heap, the drift's own parts — thirty-four specs of them, all of them measured on the
read side of the engine. **Nothing in the tree writes a table.** The one writer is `columnar-testdata`'s
dataset generator: a benchmark fixture, `publish = false`, with a fixed TPC-H schema and a scale factor
as its only parameter. It is not a write path, and it was never measured as one.

**And an embedded store is made of three things the engine does not have.**

1. **A write path** — rows in, row groups out.
2. **A commit protocol** — a table is many files, and the set of them has to become visible atomically,
   or a reader sees half a table.
3. **A catalog that survives the process** — `Catalog` is the embedder's, in memory, and its own
   documentation says so: *"the engine never reads one from disk, never mutates one, and never outlives
   the borrow the embedder gave it"* (`crates/columnar-engine/src/facade.rs:485`).

**None of the three has a measurement**, and this repository's method is to measure before it builds. So
the design question is not "should there be a store" — it is answered elsewhere — it is **what the write
side costs**, because three numbers decide the store's shape:

- **How long does a row group take to write**, against the **15.4 ms** the read of a scan takes? The
  store's whole premise is *write once, read many*, and until the write is priced that premise is a
  slogan.
- **What does a commit cost**, and above all **what does an `fsync` cost on this host**? A protocol that
  needs three `fsync`s per commit is a different product from one that needs one, and the difference is
  the difference between a bulk loader and a database.
- **What does the write do to the file's bytes**, and therefore to the read that follows? §3h measured
  that the encoding is a **measurement, not a rule**: the dictionary pays **4.8–5.7×** on the two
  predicate columns and *loses* by **8%** on `l_extendedprice`. That was measured on the **read** side.
  The **write** side chooses the encoding and has never been measured at all.

**And the write side is the half the whole track has been walking past.** Spec 036 answered the last
named resolution of the measurement track — the file-state floor — and left the project with a query
engine whose reading is priced to a tenth of a millisecond and whose writing is not priced at all.

## The requirements

### R1 — The write is measured against the read it replaces

The probe writes the published file's **own rows** back out, through the Parquet writer, in row groups
of the file's **own size** — a million rows. The write's milliseconds per row group are published beside
the read's **15.4 ms** per scan, so that *write once, read many* becomes a **ratio** and not an
assertion.

### R2 — The commit is measured, step by step

The manifest commit has four steps — `fsync` the data file, write the manifest and `fsync` it, `rename`
it over the old one, `fsync` the directory — and each is measured **on its own**, with the whole
sequence measured as one. The `fsync` is the number that decides the protocol's shape, so it is a
figure and not a caveat.

### R3 — The bytes are published, and the file's identity is checked

The written file's size is published against the original's, and the probe compares the two files **byte
for byte**. So *"the store's writer reproduces the file"* is a claim the report either carries or does
not — it is not an expectation.

### R4 — The encoding policy is measured where it is chosen

The same rows are written **twice** — the file's own policy (the dictionary on, the high-cardinality
comment column plain) and the dictionary off — and the two are compared on **bytes written**, **ms
written**, and **ms read back**. §3h priced the dictionary's trade on the read; this prices it on both
sides of the write, which is where the choice is actually made.

### R5 — The read-back is measured through the engine's own reader

The file the probe wrote is read by `ParquetSource` — the engine's own reader, with the engine's own
batch rule — against the original file read by the same call in the same process. The premise's read
half is checked **on the file the write half produced**, not on the file the generator produced.

### R6 — The engine's own work is unchanged

No engine path is touched. The probe is an **instrument**, and the write path it measures **is not
built here**: building it before it is measured is exactly what this increment refuses.

### R7 — The answer is published either way

A cheap write side makes the store cheap. An expensive `fsync` changes the protocol — fewer commits per
load, or a write-ahead log, which is a different design with a different amount of work. **Both are
published**, with the numbers that produced them, and the design decision is the ADR's.

### R8 — The verdict is a design, not a feeling

The numbers decide four things, and each is written down: the **row-group size** the store writes, the
**commit's shape**, the **encoding policy**, and **whether the store is worth building at all**. A
number that answers "no" is published like any other number — §3f's refusal is the precedent.

## The plan's shape

Five phases, and the edges between them:

1. **The instrument** — the probe: the write shapes, the commit shapes, the read-back shapes, and the
   byte-for-byte comparison, with the report carrying every series.
2. **The campaigns** — three campaigns of the probe on this machine, the raw reports committed under
   their own names.
3. **The publication** — `PERF.md` publishes the tables, the guard checks every figure **inside the
   section that publishes it**, and `scripts/medians.py` prints them.
4. **The ADR and the questions** — the ADR records the design the numbers decide, and the next question
   is written down where the last one was: the README's Next row and `STATUS.md`'s last section.
5. **The closure** — the spec's status is `done` with the date it finished, and CI is green on the
   publish commit.

The edges: 1 → 2 → 3 → 4 → 5, and 1 → 3 (the publication's figures come from the reports), 2 → 3.

## The scope

| | |
|---|---|
| The engine's own work | **This increment changes none of it.** No engine path is touched: the probe is an instrument, and the write path it measures is not built here. |
| The write path | **Not built.** This increment measures it. The store's code comes after, against the numbers — or does not come, if the numbers say no. |
| The read path | **Unchanged.** The probe reads through the engine's own reader, exactly as the engine reads. |
| The published set | **The standing set does not slide.** The probe is a new instrument with a new family of reports, and nothing in the standing set moves. |
| The library's publish | **Not this increment's.** It is a separate act, with its own account, and it is written down as an open question. |
| The store's crate | **Not this increment's.** It is the next spec, written against what this increment measures. |
| The `parquet` bump | **Not this increment's.** It is written down as an open question, and it moves the published figures. |

## The measurement

1. **Three campaigns of the probe**, on this machine, the raw reports committed under their own names.
   The medians are of three campaigns, and the spread is published.
2. **The host's state is published** with every campaign — the load average, as every report carries it —
   because the answer depends on the host's state on the day, and spec 036 is the reason that is not
   optional.
3. **The write is measured on the published file's own rows and row groups**, so the write's figures and
   the read's figures describe the same data and the same shape.
4. **The byte comparison is exact.** The two files are compared byte for byte, and the verdict is what
   the comparison says.

## The references

| | |
|---|---|
| [`constitution.md`](../../constitution.md) IV | evidence over claims: every published number in a checked-in benchmark, and the machine it was measured on |
| [`constitution.md`](../../constitution.md) V | the method's evidence rules: the medians of at least three campaigns, and every quoted figure agreeing with the committed report |
| [`constitution.md`](../../constitution.md) VII | explicit scope, no silent deferral |
| [`constitution.md`](../../constitution.md) — Performance Standards | repeated runs and published medians; a published number must be checkable where it was taken |
| [`PERF.md`](../../PERF.md) §2d | where the cold time goes: the read the write is measured against |
| [`PERF.md`](../../PERF.md) §3f | owning the Parquet decode is not worth it — the format is not the lever |
| [`PERF.md`](../../PERF.md) §3h | the materialised column, and the dictionary as a measurement rather than a rule |
| [`PERF.md`](../../PERF.md) §3j | the row-group count is a knob, and the per-row-group cost |
| [`PERF.md`](../../PERF.md) §3r | the file-state floor: why the host's state travels with every figure |
| [`ADR-0018`](../../docs/adr/0018-the-scan-reads-the-parquet-decode.md) | the scan reads the Parquet decode, and the rewrite that was refused |
| [`spec 036`](../036-the-file-state-floor/spec.md) | the measurement track's last named resolution |
| [`STATUS.md`](../../STATUS.md) | where the project's open questions live |
