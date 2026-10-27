# ADR-0026: The button is never disabled

**Status**: accepted
**Date**: 2026-10-27
**Reviewers**: TBD

## Context

The published page runs the kernel **on open**, so a reader never has to press anything to see the
measurement. The button under the title is for a second run: to re-measure on a quieter moment, or
to watch the check run again.

It was implemented the obvious way:

```js
button.disabled = true;   // while the run is in flight
…
button.disabled = false;  // when it is over
```

A disabled button is the standard way to say "not now". And it is exactly wrong here, for two
reasons.

1. **A browser drops a click on a disabled button entirely.** No run, no status change, no feedback
   of any kind. The reader pressed a visible control and nothing happened, which is indistinguishable
   from there being no button at all — and that is the report this ADR answers: *"No button on
   SAFARI."*
2. **The window is the whole run.** The page runs on open, so the button is inert from the first
   paint until the measurement finishes: **2.70 seconds** in Safari, measured, in the timeline in
   spec 019. The only control on the page is unusable for the first three seconds of the page's
   life, and the reader most likely to press it is the reader who opened the page in a hurry.

The re-run itself was never broken: nine presses in Safari's engine, nine clean re-runs, the masks
identical bit for bit and 12,726,115 matches every time. The defect was that for the first 2.70
seconds the button was a thing that looked pressable and was not.

## Decision

**The button is never disabled, and a press is never lost.**

1. **No `disabled`, ever.** The page does not set the `disabled` attribute on the run button on any
   path — not while a run is in flight, not when the module fails to load, not when the two masks
   disagree. A control that is visible is a control that answers.
2. **A press during a run is queued, not dropped.** The page keeps one pending re-run and runs it
   when the run in flight ends. The reader's press is a request for one more measurement, and it is
   honoured; one request is enough, so a reader who presses five times gets one extra run, not
   five.
3. **The label says what is true.** **Running…** while a run is in flight, **Running… — it will run
   again** while a press is waiting, **Run the kernel again** when the last run has finished. The
   button's initial text in the page is **Running…**, because the page runs on open: the first paint
   must not promise a run that has already started.
4. **A check guards it.** `scripts/check-page.py` fails if the page ever sets `disabled` again, and
   it also guards the two protections specs 018 added — the report POST's status and the commit's
   token check — so this class of regression cannot come back quietly.

## Consequences

**Easier.** The page has one rule for its only control: it always answers. There is no window in
which the reader can press it and get nothing, and no state in which the page needs to explain
that the button is busy — the label says it.

**Harder.** The page's run function now has a beginning and an end that are not the same thing: a
run can finish and immediately be followed by a queued one. That is a small amount of state
(`running`, `queued`) and one place where the two paths meet — and the alternative was a button
that lies about being available.

**Given up.** The button no longer *looks* unavailable while the page is busy, so a reader who
wanted to tell at a glance that the page is working now has to read the label instead. The label
says **Running…**, and the status line beside it counts the rounds, so nothing is hidden — but the
greyed-out button was a glanceable signal, and it is gone on purpose. A control that cannot be
pressed is a control that does not work.

## Measurement

**The defect, measured.** A WKWebView — the engine Safari runs on — loading the deployed page and
polling it every 100 ms: the button's `disabled` attribute was `yes` from `0.79s` to `3.49s`, the
whole run. Nine presses then followed, each one re-running the kernel cleanly.

**The fix, measured.** The same probe: the button's label is **Running…** during the run and never
carries `disabled`; a press during the run is answered with **Running… — it will run again** and
produces a second run when the first ends; a press after a run re-runs it. The masks stay identical
bit for bit and the match count stays 12,726,115 across every run.

**What this does not claim.** The page's published browser figure is unchanged: **12.59×**, Chromium,
the median of three campaigns, derived from the committed reports by `scripts/medians.py` and
checked by `scripts/check-published-figures.py`. The probe's **17.39×** in Safari's engine is a
single campaign and is **not** published; it is the reason the next spec exists — publishing a
Safari figure needs three campaigns with their raw reports committed, and the reader's own Safari
runs are how those get taken.

## Confidence

**high** — the change is small and the behaviour is checked directly, in the engine the complaint
came from, on the deployed page: the button's state is polled through the whole run and the
presses are observed rather than assumed.
