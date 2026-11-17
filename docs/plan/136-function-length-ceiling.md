# Step 136 — A function-length ceiling clippy enforces

**Status: implemented** on `step/136-function-length-ceiling`. Rewritten on 2026-11-17
(Volker, option B) after phase 1 checked the commissioned residuals against the code.

**Commissioned 2026-11-02** as "Step 95c residuals: lock-poison recovery, payload caps,
function-length debt" (transferred from open.md §70).

## What phase 1 found

1. **Lock-poison recovery guarded against nothing that can happen.** A lock is
   poisoned only by a panic while it is held. The app's handlers run outside the
   locks, on a copy of the handler list, and under the 21 cache locks there is only
   a clone, an insert, a replacement or a `retain`. Recorded as NG-32.
2. **Payload caps would add little.** Wire's `/v14/mls/messages` bounds every message
   to kilobytes (ADR-0011); many large tasks fill the store as many text messages do.
   Per-field caps would be a protocol decision for every client. Recorded as NG-33.
3. **The function-length debt is wider than the plan's list.** Of the seven named
   functions, four were within 40 lines by then; across the library 74 of 2014
   functions were over 40 lines and 14 over 60 (a brace count; clippy, counting code
   lines only, flagged 13: six in the library, seven tests). The rule "split when
   touched" had not held — Step 156 grew `ClientBuilder::login` from 52 to 60.

## Scope

1. `clippy.toml` with `too-many-lines-threshold = 60`: `too_many_lines` is part of the
   `clippy::pedantic` the library denies, so its default ceiling of 100 drops to 60.
2. The 13 functions over 60 split, behaviour unchanged: `sync_stream`,
   `ClientBuilder::resume`, `TimelineItem::from_record`, `persist_outgoing_asset`,
   `dispatch_content_variant`, `resolve_members`; the tests
   `mark_message_resent_flips_only_live_failed_rows`,
   `mark_send_outcome_settles_only_sending_rows_and_never_downgrades_an_echo`,
   `expire_ephemeral_messages_tombstones_only_due_rows_and_is_idempotent`,
   `clear_messaging_state_empties_rooms_and_cursor`, `with_slot_answers`,
   `assert_conversation_sites_refuse`, `assert_account_sites_refuse`.
3. CLAUDE.md names the enforced ceiling; 41 to 60 lines stay a review matter, split
   when a step touches the function.

## Out of scope

- The functions between 41 and 60 lines (about 60): split when a step touches them.
- The CLI crate, which does not deny `clippy::pedantic`.
