# Step 69 — absorb `conversation.mls-welcome`

> **Status:** merged to `main` as `c376219` (2026-08-14) — archived.
> **Live smoke 2026-08-14** against the own instance: user 1 invites
> user 2 into a fresh group and sends before user 2 ever syncs;
> user 2's first sync absorbs the Welcome (`on_room_invited` fires)
> and **reads that first message** — the exact loss gap-B.24
> described. Round-trip at the welcomed epoch verified.
> **DM first contact live-verified same day** against a freshly
> created third test user (the 1:1 between users 1↔2 is permanently
> established, so only a never-DM'd pair exercises it): `create_dm`
> took the Step-68 establish branch (epoch 1), the message sent
> before user 3's first-ever sync was read on that sync — the
> originally motivating scenario end-to-end.
> Closes `open.md` gap-B.24, surfaced by the Step-68 live smoke
> (observations §67): a claimed device receives the Welcome, logs the
> Step-6 "can't yet absorb" warning, skips it, and only joins via the
> out-of-order heal or `rooms-join-all` — *after* the initiator's
> first message, which is then lost to MLS forward secrecy. Real Wire
> clients absorb the Welcome and read it.
>
> **Historical names:**
> - "Step 69" — proposed 2026-08-14.
>
> **Branch / commit prefix:** `step/69-*`, `feat(step-69): …`.

---

## Goal

The sync drain processes `conversation.mls-welcome` instead of
skipping it: the receiving device joins the MLS group at the epoch
the Add-commit created — *before* any application message arrives —
so the initiator's first message decrypts like every later one.

## Design

1. **`MlsClient::absorb_welcome(welcome_bytes) -> group_id`** — new
   `encryption/welcome.rs` (receive-side sibling of
   `group_invite.rs`'s send side). `MlsMessageIn` TLS-parse →
   `Welcome` body → `StagedWelcome::new_from_welcome` (join config
   mirrors our group config; ratchet tree comes from the Welcome's
   ratchet-tree extension — Wire commits carry it) → `into_group` →
   persisted by openmls storage; returns the joined `group_id`.
   The private key material the Welcome encrypts to is the locally
   stored key-package secret from registration/replenish — nothing
   new to fetch. A Welcome for a group that already exists locally
   (external-join happened first, or redelivery) is **skipped, not
   an error**.
2. **`sync/service.rs` dispatch** — the warn+skip arm becomes
   `dispatch_mls_welcome`: `event.data` is a **bare b64 string**
   (backend-spec §12.2, the one non-object `data`),
   `qualified_conversation` names the room. Absorb → `persist_mls`
   → metadata refresh so the room + epoch materialize. Any failure:
   warn + continue (recoverable-errors rule — wire data must never
   panic or poison the drain; the heal path stays as fallback), no
   room-recovery trigger (no local state is broken by a bad
   Welcome — and a garbage frame must not be able to churn epochs,
   Step-64 narrowing).
3. **No new CLI command / no FFI change** — the consumer is the
   existing `sync` / `sync-stream` drain; `MlsClient` is internal.
   The CLI phase is satisfied by the receive path the smoke drives
   (like Steps 64/65).

## TDD list (red)

Fixture: the client uploads **real** key packages at
`register_device`; a fn-pointer matcher captures the upload body
into a static, the fixture peer (`common::mls`) builds a real
Add-commit over one of them under the conversation's group id — a
genuine Welcome addressed to the test client.

1. `welcome_joins_the_group_before_any_message` — notification page
   carries the mls-welcome event; after `sync_once` the room is
   cached and sending into it succeeds (local group exists at the
   welcomed epoch — no join_room, no groupinfo fetch: 0 hits).
2. `welcome_redelivery_and_existing_group_are_skipped` — the same
   Welcome delivered twice: second absorb is a no-op skip, drain
   stays green, exactly one local group.
3. `malformed_welcome_warns_and_continues` — garbage `data`: the
   drain neither errors out nor triggers recovery; later events in
   the same page still dispatch.

## Out of scope

- Backfilling messages sent before this ships (already lost to FS).
- Subconversation Welcomes (`subconv` — no subconversation support).
- The self-conversation family (`open.md` §63).
