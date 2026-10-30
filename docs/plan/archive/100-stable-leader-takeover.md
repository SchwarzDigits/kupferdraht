# Step 100 — A tab evicted by a takeover follows the new leader

**Status: MERGED 2026-09-29** — commissioned 2026-09-28 (Markus — the
flaky browser test of `open.md` §70 kept blocking CI). `48d612c`. Merged
to `main` as PR #69 (`7b397f1`); CI green. No smoke (a browser-host race,
covered by the scripted Step-94 driver). Scope shipped: a tab evicted by
a takeover stays a thin client of the new leader. Archived with this
header.

## Problem (found 2026-09-28)

`take_over_evicts_the_leader_and_the_evicted_tab_re_queues`
(`tests/step_94_leader_tab.rs`) failed in roughly every second or third
CI run ("the evicted tab re-queued as a thin client"). Not a test-timing
issue but a race in `bindings/web/kupferdraht-tabs/host.js`: when tab B takes
over, tab A receives B's announcement on the broadcast channel and its
own worker's `evicted` message in no fixed order. Announcement first →
A is `thin`; then `#onEvicted` set the state back to `electing`
unconditionally, and since leader and epoch no longer change, no later
`leader-changed` corrects it — A reported `electing` for good while
following B (calls through A still worked).

## Decision

`#onEvicted` keeps `thin` when the host already follows another leader;
`electing` only when no other leader is known.

## TDD

`an_eviction_after_the_new_leader_is_known_leaves_the_tab_thin` drives
the host with a scripted worker and a hand-posted announcement in the
bad order — deterministically red before the fix.

**As built (2026-09-28).** One condition in `host.js::#onEvicted`; the
driver scenario `evictionAfterTheNewLeaderIsKnown`
(`tests/web/step_94_driver.js`, a `ScriptedWorker`). Locally the Step-94
browser suite (8 tests) ran green five times in a row.
