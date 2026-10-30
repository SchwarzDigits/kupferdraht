# Step 28 — Auto-commit external Remove proposals

> **Status:** committed on `step/28-*`. Fixes a verified correctness gap: Kupferdraht
> did not commit inbound external-sender Remove proposals, so server-driven
> removals (self-leave, user/device deletion) never took effect in a
> Kupferdraht-only group. Implements the ADR-0004 auto-commit decision that was
> assigned to "Step 6" but never built. `MlsClient::commit_external_removals` +
> `DecodedMessage::ProposalQueued` signal + `Client::commit_pending_external_removal`
> dispatch wiring; also dropped the per-commit `ensure_plaintext_handshake`
> backfill for a `debug_assert` invariant. Unit-tested + live multi-account
> smoke verified (B self-leave → A drops B and stays sendable). Surfaced by
> `docs/backend-spec.md` §10.5 / Q7. Archived from PLAN.md.
>
> **Sub-steps:** none.
>
> **Historical names:**
> - "Step 28" — current.
>
> **Branch / commit prefix:** `step/28-*`, `feat(step-28): …`.

---

## Step 28 — Auto-commit external Remove proposals

When the Wire backend (acting as the group's external sender) removes a
member it cannot commit itself — RFC 9420 forbids a non-member from
committing. It signs a Remove **proposal**; a remaining **member** must
commit it. Kupferdraht must be that committing member.

### Why — verified gap

Audit of `main` (2026-06-14):

- Inbound external proposals are stored:
  `encryption/messages.rs` — `ProcessedMessageContent::ProposalMessage`
  → `store_pending_proposal`.
- …then **discarded**: `encryption/group_invite.rs`
  (`load_group_for_commit`) calls `clear_pending_proposals` before
  building the next outbound commit, dropping the queued Remove.
- No `commit_to_pending_proposals` call exists anywhere; no
  external-signature validation; no test; no TODO marker.

Consequence: in a group of only Kupferdraht clients, a backend-initiated
removal never materializes — the removed leaf lingers (a "ghost"
member). This diverges from wire-android / wire-desktop and breaks the
backend's server-side-removal path (`docs/backend-spec.md` §10.5
path 2).

Decided already: **ADR-0004** ("integrate external senders extension")
chose Option B — auto-commit validated administrative proposals — and
listed it as integration point 4, touching "Step 6 (event stream /
proposal handling)". The decision stands; only the implementation is
missing.

### What must happen (per ADR-0004 + mls-deviations W-5)

On an inbound handshake that is a **Proposal** (not a Commit):

1. **Validate** the proposal is externally signed by the group's
   registered `external_senders` key (embedded at group create, W-5) —
   not an arbitrary member proposal. openmls validates the external
   signature during `process_message`; the proposal enters the pending
   queue only if it passed. The remaining check is policy: Kupferdraht
   auto-commits **administrative Remove** proposals from the external
   sender. Other proposal types keep current behavior.
2. **Commit** the pending proposal: `commit_to_pending_proposals`
   (openmls 0.8) → produce a commit + GroupInfo bundle.
3. **Upload** via `POST /v14/mls/commit-bundles` (existing
   `groups.rs` path) and `merge_pending_commit` locally.
4. **Race handling:** other members may commit the same proposal
   concurrently; a `409 mls-stale-message` means someone won — drop our
   commit, re-sync the epoch, and on re-processing the winner's commit
   the proposal is already applied. Must not loop.
5. **Self-removal case:** if the proposal removes our **own** leaf
   (e.g. we are being deleted), we MUST NOT try to commit it
   (`CannotRemoveSelf`); instead discard the group locally (the
   existing `discard_group` path, observations.md §31) — a *different*
   remaining member commits it for the rest of the group.

### Interaction with `clear_pending_proposals`

The current blanket `clear_pending_proposals` in `load_group_for_commit`
(rationale: avoid `CannotRemoveSelf` on our own outbound atomic commits)
must be reconciled: an external Remove proposal targeting **another**
leaf must be committed, not cleared. Distinguish "queued external admin
proposal" from "leftover peer proposal we intend to drop". This is the
delicate part of the step — likely a dedicated inbound-proposal handler
that commits immediately on arrival, rather than relying on the next
outbound commit to fold it in.

### Public API

None new — this is internal MLS-protocol behavior driven by the sync
stream. Observable effect: after a server-driven removal, the affected
`Room`'s member list loses the removed user on the next sync (today it
wrongly retains them), and the group epoch advances.

### Wire API v14 endpoints

- `POST /v14/mls/commit-bundles` (existing) — uploads the auto-commit.
- Inbound proposal arrives on the existing `conversation.mls-message-add`
  event (no new surface).

### RFC 9420 sections

- §12.1.2 (Remove proposal), §12.2 (Commit), external senders
  (§12.1.8.2 / the `external_senders` extension). The external sender
  signs the proposal; a member commits it.

### Wire Message Types

None — handshake messages (Proposal / Commit), not `GenericMessage`.

### Dependencies

- Step 6 (event-stream proposal handling) — where `store_pending_proposal`
  lives; this step adds the missing commit half.
- ADR-0004 (external senders extension) — the governing decision.

### Verification

- **TDD:** unit test that feeds an inbound external-sender Remove
  proposal (signed with a test external-sender key embedded in the
  group) targeting a *peer* leaf, and asserts a commit is produced,
  uploaded, and merged (peer leaf gone, epoch advanced).
- Unit test for the **self-removal** variant: proposal targets our own
  leaf → no commit attempt, group discarded locally.
- Unit test for the **stale-race** variant: `409 mls-stale-message` on
  upload → no error surfaced, epoch re-synced, no commit loop.
- Full gate (`cargo fmt/build/clippy/test/deny`).
- Live (multi-account) smoke — run via a **temporary local script**
  (step smokes are not checked in; only `scripts/smoke-baseline.sh` is —
  see the `scripts/` convention). Two accounts via `KUPFERDRAHT_HOME`; B
  self-leaves. Assert: B drops from A's roster and A can still send.
  Diagnostic: check whether Wire delivered a *proposal* (Step 28 fired —
  the `info!` "Step 28: committing external-sender Remove proposal"
  line) or a ready-made *commit* (old Handshake path).
  **Result (2026-06-15, `observations.md` §59):** the marker appeared —
  Wire delivers a *proposal*; Step 28 committed it; B dropped from A's
  roster and A stayed sendable. Confirms the proposal path (§10.5).
