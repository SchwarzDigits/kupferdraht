# Step 86 — Self-conversation: establish, join, send

**Status: implemented 2026-09-24** — `1e5e234` (the step: library +
FFI + CLI + tests + docs) and `bbabc27` (docs fix-up) on
`step/86-self-conversation`, `main` merged in at `916e40a`; merged to
`main` by Volker; archived with this header. Commissioned 2026-09-10
(transferred from open.md §63; open.md §64's priority thesis ranked it
#1 — triple leverage). **Probe verdict (live 2026-09-24, observations
§82):** Wire lists the self conversation at epoch 0 with a `group_id`
but serves no `GroupInfo` (`404 mls-missing-group-info`) — the first
device establishes (the Step-31 commit), later devices external-join;
not an RFC deviation. **Scope shipped:** `Client::join_room` branches
on `SelfRoom && epoch == 0` → `establish_self_room`
(`client/self_room.rs`; a rejected bundle discards the local group,
Step-66 pattern), inherited by `join_all_rooms` / `Room::join` /
`send_text_auto_join`; `Client::self_room()` (FFI-mirrored); CLI
`self-room`; seven integration tests incl. a real openmls second device
(`common::mls::PeerDevice`). **Smoke 2026-09-24** (schwarz-dev-wire,
test user 3): stage A (single device establishes, sends, reads back,
`rooms-join-all` 0 failed) and stage B (second device external-joins at
epoch 2 and reads a task sent after its join) green; the smoke fixtures
were rebuilt the same day (observations §82, LEARNINGS §5.32).
Follow-ups (open.md Proposed steps): `LastRead` sync, draft sync,
personal task list + the Step-89 `DeleteForMe` path over this channel;
gap-B.32 records the roster caveat.

## Problem (live-verified, observation §64 of the 60c smoke)

The self conversation (`room_type: self_room`) appears in the rooms
snapshot but carries no local MLS group state — every send fails
with `MlsGroupMissing`. Everything that wants a private,
cross-device channel has no home:

- the personal task list (ADR-0014's "own list" use case — today a
  1-member group as a workaround),
- cross-device read position (`LastRead` rides the self
  conversation in Wire's model),
- draft sync (same channel, later).

## Open protocol question (orientation phase, probed live)

Does Wire accept an external commit into a never-established self
conversation (the 7/9 re-probe of 2026-08-24 saw
`mls-missing-group-info` on exactly this case), or must the client
establish the group at epoch 0 (the Step-31 self-establishing
path)? Probe against schwarz-dev-wire decides the design; both
paths exist in the codebase as precedents.

## Scope

1. Establish-or-join the self conversation (per the probe's
   answer), wired into `join_all_rooms`' classification so a fresh
   device heals it automatically.
2. Send/receive into the self room through the normal timeline
   (it is an ordinary MLS group of one user's devices).
3. Surface it: `Client::self_room()` accessor (matrix-rust-sdk
   naming check in orientation), FFI + CLI.
4. NOT in scope (follow-ups riding this rail later): `LastRead`
   cross-device read sync, draft sync, moving the personal task
   list — each its own step once the channel exists.

## TDD sketch

- Fixture: self room in metadata without group state →
  establish/join path runs, send succeeds (69-family harness).
- Second device of the same user (fixture peer with same user id)
  receives the message — the "own devices" fan-out pin.
- Non-self rooms take the existing join path unchanged
  (preservation).

## Smoke

Live: fresh smoke home → self room established → task added into it
from device A, visible on device B after sync (two homes, same
account — the Step-7e device-reuse pattern).
