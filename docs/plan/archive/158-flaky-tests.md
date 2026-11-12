# Step 158 — Two flaky tests: the tab election's CI-only timeout and the sender's log capture

**Status: MERGED 2026-11-12** — `5b7ba68` on `step/158-flaky-tests`, merged to `main`
as PR #78 (`58e7ef1`); locally fmt, 0 build warnings, clippy, the library's tests (856,
one ignored by design), the wasm32 check, `clippy --lib` and CI's wasm clippy line,
`step_94_leader_tab` in the browser (26) and the license headers; CI green on the first
run, the browser job included. Measured first: the sender's capture
failed 28 of 400 local runs; three candidates measured (rebuilding tracing's interest
cache 3 of 400, a second live dispatcher 0, a process of its own 0), and Volker chose
the process of its own (0 of 400 after the change). The tab timeout: two CI hits in
210 runs of a throwaway branch (PR #77, closed unmerged), neither in the election —
a candidate worker that never started under wasm-bindgen's test runner, and one
unexplained missed event (observations §102); Volker decided on no workaround, the
timeout's report stays. No smoke (test harness only), no CLI command, no CHANGELOG
entry (nothing consumer-facing).

**Commissioned 2026-11-11** (Volker), from the retrospectives of Steps 146 and
157 (LEARNINGS §5.90, §5.93): both flakes were met again at the 0.5.0 release, the
tab timeout twice more since, each time green on a rerun. A third rerun is no fix;
this step measures each cause first and then removes it.

## Problem

**The tab election's timeout, CI only.** The browser suite's `step_94_leader_tab`
drives `kupferdraht-tabs` through `kupferdraht-core/tests/web/step_94_driver.js`,
whose `once(target, type, predicate, timeoutMs = 8000)` rejects with "timeout waiting
for leader-changed". On CI it has failed in three scenarios, never locally (the local
suite: 18 suites, 87 tests, green on every run):

- run 42685179307 (PR #66, release 0.5.0):
  `take_over_evicts_the_leader_and_the_evicted_tab_re_queues` timed out, and the next
  scenario, `service_token_calls_reach_the_leader`, failed with it ("Failed to detect
  test as having been run. It might have timed out."). A failure that carries into the
  next scenario suggests state of the failed one outliving it: a tab still holding the
  `kupferdraht:instance` lease, or a leader still announcing.
- run 42894317339 (PR #72, Step 157): `the_leader_names_its_version`.
- the 0.5.0 release itself (LEARNINGS §5.90).

The suspected cause (§5.93), unmeasured: a closed leader's late announcement, with a
higher epoch, holds a fresh tab, so the `leader-changed` the scenario waits for never
comes. Whether the product or the harness is at fault is not known; the timeout's
message says nothing about the state the tab was in.

**The sender's log capture.**
`sync::sender::tests::a_dropped_non_basic_leaf_is_logged_at_warn_with_the_message_id`
(`kupferdraht-core/src/sync/sender.rs`) captures the `warn!` with a scoped subscriber
(`tracing::subscriber::with_default`) inside the library's test binary, where other
tests run in parallel. `tracing` caches a callsite's interest for the whole process: a
parallel test that reaches the callsite first, with no subscriber, can hide the event
from the capture (LEARNINGS §5.63, Step 120, which solved the same flake by moving the
test into a binary of its own). This test checks a private function, so that move
does not fit as it is.

## Scope

- **Make the tab timeout tell what happened.** The driver's timeout names the waiting
  tab's state: its `leaderTabId`, the epoch it last saw, the `leader-changed` events it
  did receive, and whether the lease is held. The next CI failure is then evidence.
- **Reproduce it.** Run the scenarios repeatedly under CI-like load (the browser suite
  in a loop, the CPU constrained) until the timeout comes, and read the state it names.
- **Remove the cause** where the measurement puts it: in `kupferdraht-tabs` if a tab
  follows a stale announcement (a product defect a user would meet as a tab without a
  leader), in the driver if a scenario leaves tabs or the lease behind for the next.
  Every scenario closes its tabs and releases the lease before it returns.
- **Make the log capture deterministic** without giving up what it checks (the drop is
  a `warn!` naming the message id and the credential class).

## Out of scope

- A longer timeout or an automatic retry in CI: either hides the flake instead of
  explaining it, unless the measurement shows a legitimately slow path.
- A change of the tabs' protocol beyond what the measured cause needs.
- Other tests' timing: only these two flakes.

## Decisions phase 1 brings to Volker

- Where the tab fix lands, once measured: product (`kupferdraht-tabs`) or harness
  (the driver), and whether a product fix is consumer-visible (a CHANGELOG entry).
- How the log test becomes deterministic: its own test binary through a crate-internal
  test seam, rebuilding tracing's interest cache inside the capture, or a capture
  subscriber installed for the whole test binary.

## TDD sketch

- The driver's timeout message carries the tab's state (a scenario that waits for an
  event that cannot come shows it).
- For the measured tab cause: a scenario that forces the interleaving deterministically
  (for instance a late announcement of a closed leader with a higher epoch after a
  fresh tab joins), red before the fix.
- For the log capture: the callsite hit first on another thread with no subscriber,
  then the capture; red today, green after.
- No CLI command and no public API, unless the tab fix changes `kupferdraht-tabs`'
  behaviour; no smoke (test harness and browser module only).

## Docs

`docs/LEARNINGS.md` (what the measurement showed), `docs/observations.md` if the cause
is a browser behaviour, `bindings/web/kupferdraht-tabs/README.md` and `CHANGELOG.md`
only for a product fix.
