# Spec 037 — The write side — tasks

**Status**: open
**Spec**: [spec.md](spec.md)

## Phase 1 — the instrument

- [x] The **write shapes**: the published file's own rows, written back out through the Parquet writer
      in the file's own row groups — the whole file, and one row group alone.
- [x] The **commit shapes**: the `fsync` of the data file, the manifest's write and `fsync`, the
      `rename`, the directory's `fsync`, and the whole commit as one sequence.
- [x] The **encoding shapes**: the file's own policy and the dictionary off, on the same rows.
- [x] The **read-back shapes**: the engine's own reader on the file the probe wrote, against the
      original in the same process.
- [x] The **byte comparison**: the two files compared byte for byte, and the verdict in the report.
- [x] The report carries every series, the file's own facts, the host's load average, and the commit.

## Phase 2 — the campaigns

- [x] Three campaigns of the probe, on this machine, the raw reports committed under their own names.
- [x] The host's load average published with each, and the spread beside every median.

## Phase 3 — the publication

- [x] `PERF.md` publishes the write table, the commit table, the encoding table and the read-back
      table, and the guard checks every figure **inside the section that publishes it**.
- [x] `scripts/medians.py` prints the new set beside the others.
- [x] The **design the numbers decide** is published in the section: the row-group size, the commit's
      shape, the encoding policy, and the answer to whether the store is worth building.

## Phase 4 — the ADR and the questions

- [x] The **ADR** records the decision the numbers make, and the refusal if the numbers say no.
- [x] The **next question** is written down where this one was: the README's Next row and
      `STATUS.md`'s last section — **either way**.

## Phase 5 — the closure

- [ ] The spec's status is `done` with the date it finished, and CI is green on the publish commit.

## The gate

- [x] `cargo fmt`, clippy on arm64 and x86-64, the whole test suite, `cargo deny`, the wasm32 check,
      the published-figures guard and the page checks all green.
- [ ] CI green, and the spec's status is `done` with the date it finished.
