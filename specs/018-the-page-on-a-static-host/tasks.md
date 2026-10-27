# Spec 018 — The page on a static host — tasks

## R1 — The page runs, and measures, with no server at all

- [ ] Serve `target/web` with a plain file server and open the page in the in-app Browser: the
      module loads, the kernel runs, the edge check and the mask check pass, and the numbers are
      printed.
- [ ] With no server, the page's only failure mode must be a missing or broken module — not a
      missing endpoint.

## R2 — The page names the commit it was built from

- [ ] `just web-build` writes `target/web/commit`, holding `git rev-parse --short HEAD` and
      `-dirty` when the tree has tracked modifications — the same convention
      `scripts/web-server.py` uses.
- [ ] The page shows the commit it was given: the static file on a static host, the live commit
      from the server locally.
- [ ] The local server still answers `/commit` dynamically: it intercepts the path before the file
      handler, so the static file never shadows it.

## R3 — The page never claims a report was written when it was not

- [ ] `app.js` checks the POST's `response.ok` and reports the status when it is not ok.
- [ ] The message says what is true on a static host: the numbers above are this browser's, and
      there is no report server here to record them.
- [ ] On the local server the message is unchanged, and the report really is written to
      `target/web/browser-report.json`.

## R4 — The published page is the page the repository builds

- [ ] The deployed directory is `target/web` as `just web-build` leaves it.
- [ ] The page's link back to the repository is in the built page.

## The deployment itself

- [ ] Ask the user to confirm: the site is **public**, and `source_path` is uploaded to **private**
      cloud storage.
- [ ] Deploy, open the public URL in the in-app Browser, and confirm the kernel runs there — the
      same proof the local page gives, at the URL a reader can reach.
- [ ] Publish the URL in `README.md` and in `PERF.md` §1a, commit, and push.

## The gate

- [ ] `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, the
      cross-compile check for `wasm32-unknown-unknown`, and the full test suite are green.
- [ ] `python3 scripts/check-published-figures.py` is green: the browser figures still agree with
      the committed reports.
- [ ] The page's own text is re-read against the deployed page — every claim it makes is true
      there.
