# Spec 019 — The button always answers

**Status**: in progress
**Started**: 2026-10-27
**ADR**: [ADR-0026](../../docs/adr/0026-the-button-is-never-disabled.md)
**Predecessor**: [018-the-page-on-a-static-host](../018-the-page-on-a-static-host/spec.md)

**Published**: <https://oib98vz3b8zwj.space.minimax.io>

## The problem

A reader opened the published page in Safari and said the obvious thing: *"No button on SAFARI."*

The button is there. The page's own screenshot shows it, at the top left, reading **Run the kernel
again**, and the run behind it had passed its checks: the masks identical bit for bit, the match count
12,726,115, commit `9dc7f8d`. So the complaint was not that the control is missing. It was that
pressing it does not do anything.

And there is a real reason a press can do nothing: **the button is disabled for the whole run.**

```js
async function run() {
  const button = el("run");
  button.disabled = true;          // ← inert for the length of the run
  ...
  button.disabled = false;
}
```

The page runs the kernel **on open** (spec 013), so the first thing a reader sees is a button that
cannot be pressed, for as long as the run takes. A browser does not ignore a click on a disabled
button silently in the reader's mind — it ignores it *entirely*: no run, no status change, no
feedback. The reader pressed a visible control and got nothing, and concluded there was no button.

The window is not short. In Safari the run takes **2.7 seconds** — measured, in the timeline below —
and the button is inert for every one of them.

This is the same class of defect the repository has fixed twice already on this page: the report POST
that claimed a report nobody received (spec 018, R3), and the commit that trusted a host's index
page (ADR-0025, decision 2). Both were "the page says something the reader cannot verify". This one
is "the page offers a control that does not respond". The page's whole claim is that it is willing
to be checked; a button that silently drops a press is the same failure in a different costume.

## The requirements

### R1 — The button is never disabled

- [ ] The page never sets the `disabled` attribute on the run button, at any point, on any path —
      including the paths where the module fails to load and the paths where the two masks disagree.
- [ ] The button is pressable at every moment the reader can see it, and a press always has a
      **visible effect**.

### R2 — A press during a run is not lost

- [ ] A press while a run is in flight is **kept**: it is run again when the run in flight ends.
- [ ] The button says so while it waits — the reader can see that their press was heard.

### R3 — The button's label says what is true

- [ ] While a run is in flight the button reads **Running…**.
- [ ] After a run has finished the button reads **Run the kernel again**.
- [ ] The page's first paint does not promise a run that has already started: the button's initial
      text is **Running…**, because the page runs on open.

### R4 — The page keeps the promises specs 018 and 019 made

- [ ] The report POST's status is still checked, and the page still says there is no report server
      on a host that has none.
- [ ] The commit is still checked against a token, not trusted from a host that answers an unknown
      path with a document.
- [ ] A check guards all of it, so a later edit cannot quietly put the `disabled` attribute back.

## Scope

**Out of scope, with the reasons:**

- **Publishing a Safari figure.** The probe below measured the same page in Safari's engine at
  **17.39×** — against the published **12.59×** for Chromium. That is a real difference and it is
  worth a reader's attention, but it is **one campaign**: the constitution wants a median of at least
  three, with the raw reports committed, and the probe's user agent is `WKWebView`'s, not Safari's.
  A published figure needs the reader's own Safari runs against the local server, committed as
  reports. That is the next spec, not this one.
- **Making the run shorter.** 2.7 seconds in Safari is what the measurement costs — 20 million
  rows, five rounds, two implementations. Shortening it would mean measuring less, and the page
  exists to publish the number it measures.
- **Changing the measurement.** Rows, iterations, rounds and the threshold are the benchmark's, and
  they are the native benchmark's. This spec touches the page's behaviour and nothing it measures.
- **A second control** — a "run once" button, a "copy" button, a form. One button, one meaning.

## Evidence

**The defect.** A WKWebView — the engine Safari runs on — loading the deployed page and polling it
every 100 ms:

```
[  0.79s] status=«loading the module…» btn=«Run the kernel» disabled=yes rows=«—»
[  3.27s] status=«running… round 5 of 5» btn=«Run the kernel» disabled=yes rows=«20,000,000 (rebuilt in 261 ms)»
[  3.49s] status=«done · there is no report server at this address (405) …» btn=«Run the kernel again» disabled=no
```

The button is `disabled=yes` for the whole run — **2.70 seconds** from the first paint to the last
round — and the page runs on open. Nine presses then followed, each one re-running the kernel
cleanly: the defect was never that the re-run was broken, but that for the first 2.70 seconds the
only control on the page could not be pressed.

**The fix.** The same probe, recording the page's **own** history — the page's measurement is one
long synchronous block, so a probe cannot read the DOM in the middle of it, and the history is the
only way to see the states the page passed through:

```
       6ms  run   Running…
      10ms  status loading the module…
     2363ms  status done · the report was written to target/web/browser-report.json
     2363ms  run   Run the kernel again
     2413ms  run   Running…                          ← a press after the run
     2413ms  status loading the module…
     2414ms  run   Running… — it will run again      ← a press during the run: queued, and said so
     4698ms  status loading the module…              ← the queued run, when the second ended
     4698ms  run   Running…
     6992ms  status done · the report was written to target/web/browser-report.json
     6992ms  run   Run the kernel again
```

Three runs, one press of the probe's own and one press the page made on itself at the first yield
of a run — which is where a browser delivers a click that arrives while the page's JavaScript is
busy. The button was **never disabled**, the label always said what was true, and every run ended
with the masks identical bit for bit and 12,726,115 matches.

**The numbers, in the same probe:** oracle 3.130 ns/row (0.319 G), `simd128` 0.180 ns/row
(5.556 G), **17.39×** — the reader's own Safari run measured 17.50×, with the same 5.556 G and the
same 12,726,115 matches.

**Chromium, the browser §1a's published figure comes from:** 12.56× before the press, 12.59×
after it, 12,726,115 matches both times, masks identical bit for bit, and no console warning or
error. The button reads **Running…** while it works — dark, not greyed — and **Run the kernel
again** when it is done.

**On the static host**, the page still says what is true: *"done · there is no report server at this
address (501) — the numbers above are this browser's"*. On the repository's own server it says
*"the report was written to target/web/browser-report.json"*, and the report is there.


## References

| principle | how this spec serves it |
|---|---|
| I. One Predicate, Six Implementations, One Answer | The button is the reader's way of asking the question again; it must ask it again, in every browser. |
| IV. Evidence over claims | The page's controls are checked against what they do, in the reader's browser — and the timeline above is the receipt for the defect this spec fixes. |
| V. The Comparison Is Reproducible by a Stranger | A stranger with a link can re-run the measurement and get the same answer. A button that drops a press is a stranger's run that does not happen. |
| VII. Explicit Scope, No Silent Deferral | The Safari figure is named as out of scope, with the reason: one campaign is an estimate, and the published set needs three with their raw reports committed. |
