# Spec 018 — The page on a static host

**Status**: in progress
**Started**: 2026-10-27
**ADR**: [ADR-0025](../../docs/adr/0025-the-page-on-a-static-host.md)
**Predecessor**: [017-the-repository-goes-public](../017-the-repository-goes-public/spec.md)

## The problem

The browser page is the hook for a reader who has a link rather than a terminal — spec 013's
whole point. It works, it checks itself, and it refuses to print a throughput if the two
implementations disagree. But it needs a **server**: `just web` starts `scripts/web-server.py`,
which serves `target/web` and takes the page's report.

A server is what a reader does not have. A CDN, a static host, GitHub Pages — none of them run
Python, and none of them will ever accept a `POST /report`.

So the page is about to be published to the one place a reader can reach, and it carries two
behaviours that only work because a server happens to be there:

- **The commit.** The page asks for `commit` and the server answers from `git`, at the moment the
  page asks. On a static host there is nothing to ask, and the page says `unavailable` — honest,
  and useless, in a repository whose every published figure names the commit it was taken at.
- **The report.** The page posts its measurement to `report` and, on success, says *"the report was
  written to target/web/browser-report.json"*. A static host answers that POST with **404** — and
  `fetch` does **not** throw on a 404. The page would print a success message for a report nobody
  received, on a path that does not exist on the reader's machine.

The second is the kind of thing this repository does not ship. A page that lies about itself is
worse than a page that says nothing, and this one is the project's proof.

## The requirements

### R1 — The page runs, and measures, with no server at all

- [ ] The page loads the module, runs the kernel, runs the edge check and the mask check, and
      prints its numbers from a static host, with nothing but a file server.
- [ ] The two checks still gate the output: no throughput is printed unless the masks are
      identical bit for bit and every edge agrees. A host with no server must not weaken that.

### R2 — The page names the commit it was built from

- [ ] The build writes the commit into the built directory, as a file named `commit` — the same
      name the local server answers dynamically, so the page asks the same question either way.
- [ ] The page shows that commit. A page served from a static host names its commit; a page
      served by `scripts/web-server.py` keeps showing the **live** commit, with `-dirty` when the
      tree is not clean. The server's behaviour is unchanged.
- [ ] The commit the file names is the commit the tree was at when the page was **built**. If the
      kernel's source changed, cargo rebuilt the module from that tree; if it did not, the module
      is the same bytes either way. The file cannot be stale in the way a build-script-baked
      commit can be, because it is written on every build.

### R3 — The page never claims a report was written when it was not

- [ ] The page checks the POST's **status**, not only that it did not throw.
- [ ] On a host that did not accept the report, the page says so, in words that are true on that
      host.
- [ ] On the local server, the page still says the report was written — and it is.

### R4 — The published page is the page the repository builds

- [ ] The deployed site is `target/web`, built by `just web-build`, with no hand-edited copy of
      anything in it.
- [ ] The page carries a link back to the repository, so a reader who wants the source has it.

## Scope

**Out of scope, with the reasons:**

- **Collecting reports from the deployed page.** A static host has no endpoint to post to and no
  database to keep them in. The published evidence stays in `bench-results/`, measured on the
  development machine, where the host's load average is a condition of the measurement. The
  deployed page is the **proof**, not a new source of published numbers.
- **A service worker, cache-busting, a custom domain.** The page is a measurement and a proof; it
  does not need to be a service.
- **Publishing the ARM64 or x86-64 measurements through the browser.** The browser's `simd128` path
  is a third implementation of one predicate; §1 and §1b are measured elsewhere, on their own
  hosts. The page never becomes a source of published figures.
- **A URL per commit.** The page names its commit in the text. A URL per commit would need a
  hosting scheme this repository does not have.

## References

| principle | how this spec serves it |
|---|---|
| I. One Predicate, Six Implementations, One Answer | The browser's path is held to the oracle bit for bit, in the reader's own browser, every time the page is opened. |
| IV. Evidence over claims | The page's checks gate its output, and the page stops claiming a report was written when it was not. |
| V. The Comparison Is Reproducible by a Stranger | A reader with a link can open the page and watch the check run, with no toolchain and no build. |
| VI. The Dataset Is Named Honestly | The page states the column, the threshold, the row count and the commit it was built from. |
| VII. Explicit Scope, No Silent Deferral | The four exclusions above, each with its reason. |
