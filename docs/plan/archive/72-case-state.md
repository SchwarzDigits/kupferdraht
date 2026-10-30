# Step 72 — Case state: `CasePayload` snapshots (ADR-0018)

> **Status:** merged to `main` (72a `d3d59d5`, 72b `d45de9c`, 72c
> `69ab85b`; plus the toolchain fix `59857bb` in between) —
> **archived 2026-08-21.** ADR-0018 fully implemented. 72c green
> on `step/72c-case-send` (2026-08-21; red review-hardened — the
> no-fallback proto pin). **Live smoke 2026-08-21** against the own
> instance, users 1+2+3, all stages: admin create rev 1 (extensions
> incl.), member sees case + [admin] marker, member set rejected
> client-side, update rev 2 propagates, and the invitee sees the
> case on their FIRST sync (invite re-broadcast — the ADR MUST).
> The forged-non-admin variant is fixture-covered (72b). Side note:
> the fresh-home smokes had exhausted Wire's device cap on all three
> test users — 14 stale devices cleaned via `--delete-device` /
> `device-delete`. Implements ADR-0018 (Accepted 2026-08-17): per-room
> case state as full-snapshot `CasePayload` messages in
> `GenericMessage` field 262001, LWW fold by `revision`, core fields
> admin-gated on Wire conversation roles, invite re-broadcast, no
> text fallback. Mirrors the Step-60 task machinery (ADR-0014).
>
> **Sub-steps:** 72a (payload + member roles), 72b (fold + store +
> read API), 72c (send + re-broadcast + CLI).
>
> **Historical names:**
> - "Step 72" — proposed 2026-08-17 (ADR-0018 commit).
>
> **Branch / commit prefix:** `step/72{a,b,c}-*`, `feat(step-72a): …`.

---

## Goal

A Kupferdraht client can attach a typed case snapshot to a room, update
it, and every member — including late joiners — folds the latest
authorized snapshot into local state; consumers read it as
`Room::case()`.

## 72a — payload + member roles

1. **Proto** (`kupferdraht-core/proto/generic_message.proto`):
   `optional CasePayload case = 262001;` outside the `content` oneof
   (sibling of `task = 262000`; the receive dispatch checks case,
   then task, then the oneof — a message carries at most one).
   `CasePayload { revision u64=1, case_ref=2, external_ref=3,
   title=4, case_type=5, state CaseState=6, state_detail=7,
   assignee QualifiedUserId=8, due_at_ms=9, priority CasePriority=10,
   opened_at_ms=11, closed_at_ms=12, authority_ref=13,
   map<string,string> extensions=100 }`. Enums `CaseState {OPEN=0,
   ACTIVE, WAITING, RESOLVED, CLOSED, CANCELLED}`, `CasePriority
   {LOW=0, NORMAL, HIGH, URGENT}`; unknown enum values decode to the
   0 variant (ADR-0014 precedent — a snapshot from a newer Kupferdraht
   stays visible, the next one heals it).
2. **Wire roles → `RoomMemberRole`**: decode `conversation_role` on
   `ConvMemberSelf` / `ConvMemberOther` (v14 `Member` / `OtherMember`,
   `RoleName` string; absent → member). Public type outside `wire/`:
   `RoomMemberRole { Administrator, Member }` (matrix-rust-sdk names
   its admin variant `Administrator`; Wire has exactly two roles —
   `wire_admin` / `wire_member` — mapped in `wire/` only; any other
   `RoleName` maps to `Member`). `RoomMember.role` field; persisted
   in `room_members.role` (migration V26, SHA-pinned); FFI mirror.
3. **Codec module** `timeline/case/{types,encode}.rs`: `CaseSnapshot`
   (public, matrix-shaped names) ↔ `CasePayload` round-trip.

## 72b — fold + read (design settled at the 72b hard stop, 2026-08-21)

- **Fold-on-read, no materialized table** (V27 dropped): cases have
  no cross-room views in v1 (unlike tasks, whose V23 cache exists
  exactly for those), so `Room::case()` folds the room's persisted
  `timeline_events` rows (`kind='case'`) on read. Redaction needs no
  rebuild hook — a tombstoned row simply drops out of the next read
  (the inbound MessageDelete → tombstone path is pinned by the
  existing delete tests; composing the two pinned contracts covers
  the production path). A materialized cache becomes its own step
  if cross-room case views ever land.
- **Receive-time role capture:** the dispatch persists
  `sender_was_admin` (bool, from the Step-72a metadata roles at
  receive time) inside the row body, so the read-fold keeps the
  ADR's receive-time semantics — a later demotion never invalidates
  previously authorized snapshots.
- Dispatch `dispatch_inbound_case` (field 262001, checked before the
  task check): persist `kind='case'`, route **every** case row —
  both roles — through `apply_case_event` (review note: a
  persist-only shortcut for non-admins would kill the any-member
  extensions rule); failures never trigger room recovery.
- Fold rules (`timeline/case/fold.rs::apply_case_event`, unit-pinned):
  admin core LWW by `revision` (equal → later arrival), non-admin
  events touch `extensions` only (**wholesale replace** — one
  display-only unit, pinned against per-key merge) and need existing
  state.
- Read API: `Room::case() -> Option<CaseSnapshot>` (one case per
  room in v1 — no `cases()`), `TimelineItemKind::Case` (+ FFI/CLI
  arms; pinned by the integration test — an unknown-kind row would
  degrade to Text and render the packed body as chat text).

## 72c — send + re-broadcast + CLI

- `Room::set_case(patch) -> CaseSnapshot`: patch onto the current
  fold, `revision + 1`, send full snapshot (no `Text` fallback —
  ADR-0018 §Decision), local fold on send (self is admin or the
  server would have refused nothing — the fold gate applies to
  inbound only; sending as non-admin is a consumer-visible
  `InvalidInput`).
- **Invite re-broadcast (MUST):** `invite_members` sends the current
  snapshot right after the Add-commit merges, when the room has one.
  MAY-rebroadcast on observed member-join is out of 72c (needs the
  member-join event, which Kupferdraht currently discards — follow-up).
- CLI: `case-set --id ROOM [--title …] [--state …] …`, `case-show
  --id ROOM`; docs (cli/library/bindings/api-mapping/ARCHITECTURE
  §7); FFI mirror.

## TDD list — 72a (red)

1. `case_payload_round_trips_through_generic_message` (unit) — encode
   a full `CaseSnapshot` (all core fields + two extensions) into a
   `GenericMessage`, decode, equal; `content` oneof stays unset.
2. `unknown_case_state_decodes_to_open` (unit) — a **raw**
   `CasePayload { state: 42, priority: Some(99), … }` (built without
   the encoder, so the decoder contract is pinned in isolation —
   review feedback 2026-08-18) decodes to `Open` / `None`, rest
   intact.
3. `member_roles_decode_from_conversation_metadata` — `/conversations/
   list` mock with `conversation_role` on self (`wire_admin`), bob
   (`wire_member`), carol (field absent): `Room::members()` yields
   `Administrator` / `Member` / `Member`, and **all three** survive
   the store round-trip (`load_rooms_from_store` replaces the cache;
   carol asserted in both halves — review feedback).
4. `sync_members_keeps_roles_from_the_metadata_snapshot` — fixture
   group under the advertised `group_id`, `join_room`, then
   `Room::sync_members()` (leaf-derived rebuild): self stays
   `Administrator`, bob `Member` — the rebuild must MERGE roles from
   the metadata snapshot, not reset to `Member` (review feedback: a
   silent admin wipe would starve the 72b fold gate).

## TDD list — 72c (red, review-hardened 2026-08-21)

1. `build_case_leaves_content_oneof_and_task_unset` (crate unit,
   `wire/proto.rs`) — THE no-fallback pin: `content` oneof unset,
   task field unset, case field set. The integration tests see only
   MLS ciphertext, so this is the single enforceable place for
   ADR-0018's wire contract (task precedent:
   `build_task_without_fallback_leaves_content_oneof_unset`).
2. `set_case_creates_updates_and_folds_locally` — create = rev 1
   with generated `case_ref`; patch update keeps unpatched fields,
   bumps to rev 2, stable identity; one `/mls/messages` POST per
   send; own sends fold locally.
3. `set_case_rejects_non_admin_sender_client_side` — `wire_member`
   self → `InvalidInput` before any traffic, no local fold.
4. `invite_rebroadcasts_the_current_snapshot` — after the invite,
   message hits go 1 → 2 and the folded revision stays unchanged
   (re-broadcast repeats, never updates). **Green note (review):**
   the hit count cannot pin ordering — the send MUST go out only
   after `upload_and_merge_commit_bundle` returns (the invitee
   decrypts only post-commit epochs).
5. `invite_without_case_sends_no_snapshot` — day-0 green guard
   against a spuriously firing re-broadcast hook.

## Live-smoke plan (72c)

Own instance, users 1+3 (fresh homes): user 1 (admin — creator)
`case-set`, user 3 sees it after sync; user 3 (member) `case-set` →
rejected client-side, and a forged non-admin snapshot (raw send)
is stored but not folded on user 1; invite user 2 → user 2's first
sync shows the case (re-broadcast); revision bump visible.

## Out of scope

- Finer roles than Wire's admin/member (ADR-0018 Negative).
- Rebroadcast on member-join events; join snapshot for tasks (§63).
- Text fallback for plain Wire clients (ADR-0018 §Decision).
