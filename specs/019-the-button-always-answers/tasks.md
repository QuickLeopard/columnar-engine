# Spec 019 — The button always answers — tasks

**Published**: <https://oib98vz3b8zwj.space.minimax.io>

## R1 — The button is never disabled

- [x] `app.js` never sets `disabled` on the run button, on any path — the load path, the
      disagreement path, or the ordinary one.
- [x] The dead `button[disabled]` styling is removed, so the page has no way back to an inert
      button by accident.
- [x] A press always has a visible effect: the status line changes and the button's label changes.
      Measured in WebKit and in Chromium: a press after a run starts another run, and a press
      during a run is answered with **Running… — it will run again**.

## R2 — A press during a run is not lost

- [x] A press while a run is in flight is queued, and the queued run happens when the run in flight
      ends.
- [x] The button says the press was heard: **Running… — it will run again**.
- [x] Exactly one queued run, however many times the reader presses: a press is a request for one
      more run, not for a queue of them.

## R3 — The button's label says what is true

- [x] **Running…** while a run is in flight.
- [x] **Run the kernel again** after a run has finished.
- [x] The button's initial text in `index.html` is **Running…**, because the page runs on open and the
      first paint must not promise a run that has already started.

## R4 — The page keeps the promises specs 018 and 019 made

- [x] The report POST's status is still checked; the page still says there is no report server on a
      host that has none — **501** on the plain file server, **405** on the static host.
- [x] The commit is still checked against a token.
- [x] `scripts/check-page.py` guards all of it — nine checks — and is wired into `just check` and
      into CI. It was mutation-tested: a `disabled` attribute put back into the page makes it fail.

## The evidence

- [x] The WebKit probe (Safari's engine) against the **local server**: a press after a run re-runs
      it, a press during a run is queued and the queued run happens, the button is never disabled,
      and the masks stay identical bit for bit with 12,726,115 matches. The page's own history is
      in the spec.
- [x] The same probe against a **plain file server** (no report endpoint): the page says there is
      no report server here, and the button behaves identically.
- [x] Chromium, through the in-app browser: **12.56×** before the press, **12.59×** after it,
      12,726,115 matches both times, masks identical bit for bit, and no console warning or error.
- [x] The **deployed** page: the same probe, after the site was updated. The button was never
      disabled, a press re-ran the kernel, a press during a run was queued and ran, the masks
      stayed identical bit for bit, and the match count stayed 12,726,115.

## The gate

- [x] `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, the
      x86-64 clippy pass, the `wasm32` cross-compile check, the full test suite and `cargo deny`
      are green.
- [x] `python3 scripts/check-published-figures.py` is green: 118 checks, 0 failures.
- [x] `python3 scripts/check-page.py` is green: 9 checks, 0 failures.
- [x] The deployed page is updated **in place** at the same URL, after the user confirmed.

## The deployment itself

- [x] The user confirmed: the site is **public**, and `source_path` is uploaded to **private** cloud
      storage. The source uploaded is the same 3.0 MB snapshot of the tracked tree.
- [x] The site is updated in place, at <https://oib98vz3b8zwj.space.minimax.io>, with the build
      output only: `index.html`, `app.js`, `columnar_web.wasm`, naming commit `1c661d9`.
- [x] The deployed page is opened and read: the button is never inert, and a press re-runs the
      kernel. In WebKit: three runs, a press after a run and a press during a run, the masks
      identical bit for bit and 12,726,115 matches every time. In Chromium: **12.56×** before the
      press, **12.49×** after it, and no console warning or error.
