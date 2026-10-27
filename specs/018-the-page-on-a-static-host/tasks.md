# Spec 018 — The page on a static host — tasks

**Published**: <https://oib98vz3b8zwj.space.minimax.io>

## R1 — The page runs, and measures, with no server at all

- [x] Serve `target/web` with a plain file server and open the page in the in-app Browser: the
      module loads, the kernel runs, the edge check and the mask check pass, and the numbers are
      printed. **12.57×, 12,726,115 matches, masks identical bit for bit.**
- [x] With no server, the page's only failure mode is a missing or broken module — not a missing
      endpoint. On the hosted page it answered the report POST with a **405** and said so.

## R2 — The page names the commit it was built from

- [x] The build writes the commit into the **built page** — `just web-build` substitutes it into
      the built copy of `app.js`, with `-dirty` when the tree has tracked modifications, the same
      convention `scripts/web-server.py` uses.
- [x] The page shows that commit. A page served from a static host names its commit; a page
      served by `scripts/web-server.py` keeps showing the **live** commit.
- [x] The page **checks** the server's answer rather than trusting it. The hosted page answers
      every unknown path with its own index page, with a `200`, so the first deployment's page
      displayed the host's document as its commit. The check is a short token, not a document.

## R3 — The page never claims a report was written when it was not

- [x] `app.js` checks the POST's `response.ok` and reports the status when it is not ok.
- [x] The message says what is true on a static host: *"there is no report server at this address
      (405) — the numbers above are this browser's"*.
- [x] On the local server the message is unchanged, and the report really is written to
      `target/web/browser-report.json`.
- [x] The POST is given a ten-second timeout, because a host that ignores POST would leave it
      pending for ever.

## R4 — The published page is the page the repository builds

- [x] The deployed directory is `target/web` as `just web-build` leaves it.
- [x] The page's link back to the repository is in the built page.

## The deployment itself

- [x] The user confirmed: the site is **public**, and `source_path` is uploaded to **private**
      cloud storage. The source uploaded is a 3.0 MB snapshot of the repository's **tracked**
      tree — no `data/`, no `target/`, no `.git` — and it was checked for keys and tokens first.
- [x] Deployed, and the first deployment's flaw — the host answering an unknown path with its
      index page — was found by opening the URL in a browser, fixed, and the site updated **in
      place** at the same URL, which the user chose over a second copy.
- [x] The deployed page was opened in the browser after the update: the kernel runs, the masks
      are identical bit for bit, the edge check agrees, the match count is 12,726,115, the page
      names commit `e5ab865`, and it says there is no report server.
- [x] The URL is published in `README.md` and in `PERF.md` §1a.

## The gate

- [x] `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, the
      cross-compile check for `wasm32-unknown-unknown`, and the full test suite are green.
- [x] `python3 scripts/check-published-figures.py` is green: the browser figures still agree with
      the committed reports.
- [x] The page's own text is re-read against the deployed page — every claim it makes is true
      there.
