# Step 116 — An evicted leader's late message moves no tab back

**Status: MERGED 2026-09-30** — commissioned 2026-09-30 (found while
merging PRs #78–#99: the Step-94 takeover scenario failed on CI twice, on
PR #82 and PR #100). `274dbe1`. Merged to `main` as PR #101 (`3cd3208`);
CI green. No smoke: the browser test plays the race exactly.
Retrospective: `docs/LEARNINGS.md` §5.59. Archived with this header.

## Problem

A takeover (`host.takeOver()`, or the lease hand-off) evicts the old
leader A; the new leader B announces a higher epoch. A's last heartbeat or
event may still be on its way: `BroadcastChannel` orders messages per
sender, not between senders, so it can arrive *after* B's announcement.
`TabTransport.#leaderSeen` (`bindings/web/kupferdraht-tabs/proxy.js`) takes
every announcement or event whose leader differs from the one it follows
as a new leader — without comparing epochs. Every tab, B's own host
included, then follows the dead leader A until B's next heartbeat (up to
2 s): B's host reports `state` `thin`, calls go to A and wait or fail with
`LeaderChanged`. On a slow CI runner the takeover scenario read B's state
inside that window.

Reproduced deterministically: after the takeover, the page posts an
announcement and an event in A's name with A's old epoch — both tabs move
back to A (`["A → the evicted leader", "B → the evicted leader"]`).

## Design

- `TabTransport` takes a leader announcement or event as a sighting only
  when its epoch is not lower than the epoch of the leader it follows: an
  older leader is never the current one (epochs rise with every
  leadership). A stale announcement changes nothing else either (not the
  leader's getters). A stale *event* is still delivered — its data was
  stored before the eviction, and a tab would otherwise miss, say, a
  message callback — it just moves no view.
- Equal epochs keep today's behaviour.
- Nothing changes on the leader side, in `host.js` or in the protocol.

## Tests (red first)

- `tests/web/step_94_driver.js` → `aStaleLeaderMessageMovesNoTab`, with
  `a_stale_leader_message_moves_no_tab_back` in `tests/step_94_leader_tab.rs`:
  after a takeover, a stale announcement and a stale event move no tab
  (no `leader-changed` towards A), B stays `leader`, A follows B; the
  stale event still reaches the tabs.

Browser-only fix in the reference tabs module; no CLI, no smoke (the
browser test plays the race exactly).

## As built (2026-09-30)

- `TabTransport.#stale(epoch)` in `proxy.js`: a leader announcement of a
  lower epoch is dropped whole (not even its getters); an event of a lower
  epoch is delivered but is no sighting. One condition, used twice.
- The red test failed as predicted (both tabs moved back to the evicted
  leader) and passes with the fix; the Step-94 suite passed five runs in a
  row. The scenario that failed on CI twice
  (`take_over_evicts_the_leader_and_the_evicted_tab_re_queues`) read the
  new leader's `state` inside exactly that window.
- The first version of the red test checked the end state after 300 ms and
  passed without the fix: the tabs had already recovered at the next
  message. It now records every view change.
