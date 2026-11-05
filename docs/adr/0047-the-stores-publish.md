# ADR-0047 — The store is publishable, and the order was never a tie

**Status**: accepted
**Date**: 2026-11-05
**Spec**: [040-the-source-that-reads-a-set-of-files](../../specs/040-the-source-that-reads-a-set-of-files/spec.md)

## Context

**ADR-0044 said the store could not be published yet, and named two reasons. Both of them turned out to be
wrong, and the increment that found them is the one that made the store's half whole.**

**The first reason was the API.** The store's crate was held back with `publish = false`, and the reason in
its manifest was: *"the crate's API is new, and the next increment changes it -- the engine has to read a
table that is many files, and the store's own surface will follow that."* **That increment is spec 040**, and
it is done: the engine has `FileSetSource`, and it reads a table that is many files. **And the store's own
surface did not have to move for it** -- `Table::files` and `Table::file_path` were already there, and the
embedder could build the list of paths from them. **So the reason was spent**, and the crate was publishable
the whole time after spec 040.

**The second reason was the order.** ADR-0044 said: *"The store depends on the core, and so does the
kernels, so two crates are ready at once and the publish script's total-order check has to learn a tie
before the store can join it."* **The store does not depend on the core.** It depends on `parquet`, `arrow`,
`serde`, `serde_json` and `thiserror`, and on **nothing in this workspace** -- the store is below the engine
and below the core, and it writes files rather than reading them through the engine's vocabulary. **The tie
is real and the reason is not**: the store is ready **beside the core**, because both depend on nothing.

**And two things the record did not know were found by trying to publish the crate.** The store **could not
be packaged at all**: `cargo package -p columnar-store` failed with *"no matching package named
`columnar-testdata` found"*, because its **dev-dependency** on the generator carried a version and the
generator is not on crates.io -- and a versioned dependency has to resolve on crates.io, while a **path-only**
one is dropped from the published manifest. And the store's package **carried no licence file**, which the
publish script checks for every crate.

## Decision

1. **The store is published with the others.** `publish = true`, and the reason in its manifest is now the
   truth: the API is settled, the crate packages, and the publish itself is a separate act that needs an
   account and a token.

2. **The store's dev-dependency on the generator is path-only**, so cargo drops it from the published
   manifest and the crate does not need `columnar-testdata` on crates.io to be published. And the store
   carries its **own `LICENSE`**, the copy every other published crate carries.

3. **The publish script breaks the tie the graph cannot.** The order is still **computed** from the
   manifests -- the graph decides everything it can -- and where the graph does not decide, **the list
   does**, and the list is written in the publish order. **The order is `columnar-core`, `columnar-kernels`,
   `columnar-store`, `columnar-engine`.** The old rule -- "the graph must be a total order" -- was wrong for
   a crate that depends on nothing: such a crate has a **place** in the order, not a problem in it.

4. **The glue is one call.** `Table::paths` hands the engine the list of paths it takes, in the table's
   order -- which is the order the rows come back in. The two halves of the store fit: the store writes the
   files and names them, and the engine reads them as one table.

5. **The publish is the user's act.** Nothing here uploads anything, and nothing here needs a token. The
   commands and the order are in [`docs/embedding.md`](../../docs/embedding.md).

## Consequences

- **The store's crate is publishable, and the publish is one command away.** `cargo publish --dry-run -p
  columnar-store` passes, and the script computes the order from the manifests.
- **The store's dev-dependency is not a publish blocker any more**, and the reason it was one is written
  down where it will not be repeated: a **path-only** dependency is dropped from the published manifest and a
  versioned one is not.
- **The publish order is `columnar-core`, `columnar-kernels`, `columnar-store`, `columnar-engine`.** The
  store is independent of the engine's stack, so it lands beside the engine, and the engine -- which reads
  what the store writes -- is last.
- **The two wrong sentences are corrected where they were published.** ADR-0044's "the store depends on the
  core" and its "the store's crate is not published yet" were wrong, and they are corrected there, with a
  pointer here.
- **The store's crate is still not on crates.io**, and it will not be until the user runs the publish: it
  needs an account and a token, and that is not an act this repository can take.

## References

- [`constitution.md`](../../constitution.md) II — the method: measure first, and the figures are derived from
  the committed reports rather than typed.
- [`constitution.md`](../../constitution.md) VII — explicit scope, no silent deferral.
- [`ADR-0044`](0044-the-stores-half.md) — the store's shape, and the two sentences this ADR corrects.
- [`ADR-0046`](0046-the-source-over-a-set-of-files.md) — the source over a set of files, and the increment
  that settled the store's surface.
- [`spec 038`](../038-the-stores-half/spec.md) — the store's half.
- [`spec 040`](../040-the-source-that-reads-a-set-of-files/spec.md) — the source over a set of files, and the
  glue.
- [`docs/embedding.md`](../../docs/embedding.md) — the publish guide: the order and the commands.
- [`STATUS.md`](../../STATUS.md) — where the project's open questions live.
