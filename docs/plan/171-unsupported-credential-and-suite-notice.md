# Step 171 — e2ei and a foreign cipher suite: refused visibly, not silently

**Status: commissioned 2026-11-25** (Volker). After Step 170. Two decisions of the same
day stand behind it: Kupferdraht supports no e2ei (X.509 credentials, ACME,
certificates) — neither its own nor the verification of others' — and Steps 154 and 155
are dropped, not parked (their prose is archived with Step 166's commissioning commit);
Kupferdraht stays on cipher suite 2 and only tells the user when a team asks for
another.

## Problem

Today a message, commit or Welcome from a device with an X.509 leaf is dropped silently
(Step 135, `encryption/leaf_identity.rs` reports it as unsupported). A team whose `mls`
feature requires e2ei (`mlsE2EId` enabled) looks to the user like a room where
messages go missing. And a team whose `mls` feature names a suite Kupferdraht cannot
speak fails with an openmls error instead of a sentence. Observations §94 also
records Wire's default suite as 1; Wire 5.38.9 defaults to suite 2
(`Wire.API.Team.Feature`, `instance Default MLSConfig`), as the Rust server does.

## Scope

1. **One system line per device and room.** The first message, commit or Welcome from
   an X.509 device that Kupferdraht refuses puts a room event in the timeline (a new
   `RoomEventKind` beside Step 145's: "message from <user> (device with an e2ei
   certificate) not readable: e2ei is not supported") and a log line with room, sender
   and device; the same device in the same room adds no further line until something
   changes (the device re-joins, the room's epoch resets).
2. **The team's requirement.** `mlsE2EId` read as `enabled` for the own team
   (`/feature-configs`, the `feature-config.update` event) raises one clear notice: the
   team requires e2ei, Kupferdraht does not support it, messages of verified devices
   may be missing. A room event in the personal room or a `ServerSetting` the embedder
   reads — phase 1 decides, with the embedder-reads-on-request rule in mind.
3. **No crash, no stop of sync, no leaving the group.** The epoch is followed as far as
   openmls allows without the credential; phase 1 measures what happens when it does
   not and the plan records it.
4. **Cipher suite.** The team's `mls` feature with `defaultCipherSuite` ≠ 2 or without
   2 in `allowedCipherSuites` raises one notice (the team's setting, Kupferdraht speaks
   suite 2 alone); a room of another suite is not joined, with a sentence instead of an
   openmls error. Observations §94 is corrected.
5. **Docs:** a new non-goal in `docs/non-goals.md` (e2ei, with the reason and the
   behaviour above); `docs/backend-spec.md` (Step 166's paragraph in the present
   tense); `docs/library.md` / `docs/bindings.md` (the new room event kind, the
   notice); `docs/wire/mls-deviations.md` if the suite handling touches it;
   `docs/observations.md` §94; `CHANGELOG.md`.

## Out of scope

- Any e2ei capability; verifying anyone's certificate; `/clients/{c}/nonce` and
  `/clients/{c}/access-token`.
- A second cipher suite.

## TDD sketch

- a test certificate as an X.509 leaf in a fixture group: one message, one commit, one
  Welcome from it → exactly one room event and one log line per device and room, a
  second message adds none; a device re-joining adds one — red;
- `mlsE2EId: enabled` in the server settings → one notice, a repeated refresh adds
  none; `feature-config.update` to enabled raises it once — red;
- a feature config with suite 1 as default → the notice, and a join of a suite-1 room
  fails with the sentence — red;
- the sync keeps running past all three (no `on_sync_error`) — red.

Smoke: in the stack only if it works without e2ei infrastructure; otherwise the unit
tests are the proof.
