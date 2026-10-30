# Step 65 — Timeline correctness: five projection fixes

> **Status:** MERGED to `main` — `bd76d37` (+ simple-dispatch guard
> in `469f543`), fast-forward 2026-08-14; live-smoked 2026-08-14
> (reply quote + missing-parent hard error). Originally implemented
> on `chore/review-triage` (2026-08-13,
> branch-per-step waived for the review-fix series). 4 contract tests
> green (S4 integration, S12 storage, S5/S10 hydration units).
> Design deviations from the first cut: the asset reconstruction
> cannot live in `from_record` (the metadata sits in the attachments
> table) — the shared seam is `TimelineItem::hydrated` +
> `reconstruct_asset_kind` moved to `timeline::items`; S6 has no
> cheap test seam (inbound-text dispatch is MLS-decrypt-gated, the
> gap-B.11 class) — two-line guarded move, review-verified. Seam
> analysis insight for the record: Wire does not deliver events back
> to the originating client and MLS replay protection rejects
> replayed ciphertexts before persist, so the `inserted = false`
> branch is rarer than the review assumed (same-device echo), while
> the other-device path still inserts fresh rows and notifies.
>
> **Historical names:**
> - "Step 65" — proposed 2026-08-13.
>
> **Branch / commit prefix:** `step/65-*`, `feat(step-65): …`.

---

## Goal

Five small, sharply-cut fixes where the timeline surface diverges
from what actually happened:

1. **`send_reply` without a local parent (S4).** `build_text_message`
   silently drops the quote when the parent is not in the store
   (`timeline/text.rs:189`) — other Wire clients render plain text
   while Kupferdraht renders a reply. Fix: hard error
   (`InvalidInput("reply parent not found locally — sync first")`),
   matching the observations-§33 constraint that the quote needs the
   parent's body + send time.
2. **Asset degradation in live updates (S5).**
   `TimelineItem::from_record` has no `kind == "asset"` branch;
   `Client::messages` compensates via `reconstruct_asset_kind`, but
   `emit_message_update_at` (reactions/receipts/edits) does not — a
   reaction on a file emits an `UpdateAt` whose item is Text with
   the filename. Fix: move the asset reconstruction into
   `from_record` (single source of truth).
3. **Duplicate `on_message` (S6).** `dispatch/text.rs:43` calls
   `notify_message` outside the `inserted` guard — self-echoes and
   redeliveries fire duplicate binding callbacks while the
   TimelineDiff beside it is correctly guarded. Fix: move the call
   behind `inserted` (an other-device own-message still inserts
   fresh, so nothing wanted is lost).
4. **Mentions missing from the Append diff (S10).** Mentions are
   persisted but the diff item is built from `message_to_record`
   with empty mentions — subscribers see them only after a reload.
   Fix: hydrate the mention records into the Append item.
5. **Self-echo never upgrades `delivery_status` (S12).** The UNIQUE
   conflict skips the row without a status touch. 1:1 rooms are
   covered by Step-25b delivery receipts; group sends stay
   `Submitted` forever. Fix: on the self-echo conflict, upgrade the
   existing row's status (and emit `UpdateAt`).

## TDD candidates

One failing test per fix; all five are httpmock/unit-testable with
the existing dispatch fixtures. S6's test doubles as a regression pin
for the push path (frame redelivery must not re-notify).
