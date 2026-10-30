# Step 107 — Step 97a review follow-ups

**Status: MERGED 2026-09-30** — commissioned 2026-09-29 (Markus, after
Volker's review of PR #61 — Step 97a). `8f4b19a`. Merged to `main` as PR
#78 (`53d2c31`), after main and the browser-storage chain were merged into
it (documentation only); CI green. Smoke 2026-09-29 (schwarz-dev-wire,
release CLI). Scope shipped: a resend in flight is not listed as failed,
the id lookup settles abandoned sends, the send-lifecycle contract written
down. Retrospective: `docs/LEARNINGS.md` §5.47. Archived with this header.

## Review points and decisions (2026-09-29, Markus)

| # | Point | Decision |
|---|---|---|
| Q1 | The in-flight set lives on the `Client`, not the process: two Clients on one store read each other's live `Sending` rows as `Failed`; a resend then uploads again (receivers dedupe on the id). | Accept and document (`library.md` "Send lifecycle"); correct D2's "this process" to "this `Client`" in `docs/plan/97-durable-send-and-receive.md`. |
| Q2 | A cancelled send emits no settling diff (`Drop` is sync). | Document as the consumer contract: after a cancel, re-read (`messages`, `failed_sends`). |
| Q3 | `finish_send` ignores `Ok(false)` from `mark_send_outcome`. | `warn!` when the row did not flip and is not `received`. |
| Q4 | Orphan blobs (cancelled blob upload; a deleted `Failed` file row). | A line in Step 84's scope (`open.md`). |
| N | `ARCHITECTURE.md` §5.2 steps 3/4 | Rewrite to persist-first. |
| N | `find_message_by_id` returns the raw `Sending` of an abandoned row | Read through `settled_status`. |
| N | `Sending` first shifts the UniFFI enum indices | Keep (merged; moving it back shifts them again) — noted in the CHANGELOG. |
| N | CHANGELOG names only Swift/Kotlin | Rust exhaustive matches break as well. |
| N | Ephemeral `sent_at_ms` stamped before the upload | Keep, document. |
| N | `step_97a` test 2: 500 ms sleep vs. 1500 ms delay | Wait for the row, not a fixed sleep; longer upload delay. |
| N | `resend_row` holds no in-flight guard | Take the in-flight guard for the whole resend; a row whose send is in flight is never resendable and not in `failed_sends` (it still reads `Failed`, as since Step 79). |
| N | No 97a entry in LEARNINGS | Add. |

## TDD

- `tests/step_97a_persist_before_upload.rs`: `find_message_by_id` reads an
  abandoned send as `Failed`; while a resend's upload is in flight the
  row is not in `failed_sends` and a second resend of it is refused.
- The Q3 warning is a log line — no dedicated test.

**As built (2026-09-29).**
- `timeline/send_state.rs`: `InFlightSends::insert` reports whether the id
  was new; `try_begin_send` (a resend) refuses an id already in flight;
  `is_in_flight`; `is_resendable_row` refuses any in-flight row;
  `finish_send` → `warn_if_unsettled` when `mark_send_outcome` changed no
  row and the row is not `received`.
- `timeline/resend.rs`: `resend_row` holds the in-flight guard for the
  whole resend; `failed_sends` leaves in-flight rows out.
- `client/message_lookup.rs`: `find_message_by_id` applies
  `settled_status`.
- A first cut also read a `Failed` row in flight as `Sending`; it broke
  `step_79::a_failed_resend_leaves_the_row_failed_for_the_next_attempt`
  (a failed first send's own settling `UpdateAt` fires while its guard is
  held) and was dropped — during a resend the row reads `Failed`, as
  since Step 79.
- Docs: `library.md` send-lifecycle contract (Q1, Q2, ephemeral), plan 97
  D2 wording, ARCHITECTURE §5.2, CHANGELOG (97a entry + this step), Step
  84 scope (Q4), LEARNINGS 5.47.
- Smoke 2026-09-29 (schwarz-dev-wire, release CLI, user 1's smoke home):
  a text send settles, `failed-sends` lists nothing — no backend surface
  changed.
