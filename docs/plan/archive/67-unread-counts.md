# Step 67 — Unread counts per room

> **Status:** MERGED to `main` — `5a775c6` (fast-forward,
> 2026-08-14). Gate green
> (59 binaries / 660 tests), live-smoked 2026-08-14 against the
> self-hosted instance: V25 applied to an existing DB; `rooms` showed
> the honest no-marker default on real data (5 unread from the peer,
> 0 in rooms whose messages were own webapp sends — self-exclusion
> visibly correct); `mark-read` at the newest item sent the real READ
> confirmation AND advanced the marker — unread dropped 5 → 0.
> Requested by Volker for the kupferdraht-tui room list, pulled ahead of
> the create_dm investigation. Backlog origin: `open.md` §64
> "Conversation daily life" (unread counts).
>
> **Historical names:**
> - "Step 67" — proposed 2026-08-14.
>
> **Branch / commit prefix:** `step/67-*`, `feat(step-67): …`.

---

## Goal

A per-room unread count for room lists. The read position is a
**device-local marker** (`room_read_markers`, V25 — its own table
because `rooms` is replace-managed by every sync); cross-device sync
of the read position rides the self-conversation (`LastRead`) and is
deferred to the self-conv step (`open.md` §63/§64).

## Semantics (decided 2026-08-14)

- unread = inbound `timeline_events` rows (sender ≠ self, not
  redacted) with `sent_at_ms` **>** the room's marker.
- **No marker → everything inbound counts** (honest default; a
  consumer marks read on room open, which collapses it).
- Rooms without unread rows are **absent** from the bulk map —
  consumers treat missing as 0.

## Public API

- `Room::unread_count() -> Result<u32, KupferdrahtError>`
- `Client::unread_counts() -> Result<HashMap<String, u32>, KupferdrahtError>`
  — one grouped query for the whole room list (no n+1).
- `Timeline::mark_as_read(item_id)` **additionally persists the
  marker** (the target message's `sent_at_ms`) — and does so even
  when `ReceiptMode::Off` suppresses the outbound confirmation: the
  gate governs the wire signal, not the local truth.
- Store trait: `set_read_marker` / `read_marker` / count queries with
  no-op defaults (wasm pattern); SQLite implements.
- FFI mirrors for both read APIs; CLI: `rooms` listing shows
  `unread: N`.

## Out of scope

- Mention/highlight counts (later refinement).
- Cross-device read-position sync (self-conversation step).
- Emitting a dedicated "unread changed" event — consumers recompute
  after `TimelineDiff` / `on_message`.

## TDD list (red)

1. No marker → all inbound counted; own and redacted rows excluded.
2. `mark_as_read` with receipts **off** returns `Ok(None)` (no wire
   signal) but persists the marker — the count drops accordingly.
3. Bulk `unread_counts` groups per room; zero-unread rooms absent.
