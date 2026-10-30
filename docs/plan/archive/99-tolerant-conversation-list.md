# Step 99 — One unknown conversation must not stop the sync

**Status: MERGED 2026-09-29** — commissioned 2026-09-28 (Markus — finding
H2 of the production-readiness review, `open.md` §71). `4e7230f`. Merged
to `main` as PR #68 (`0935ce2`); CI green. Smoke 2026-09-28 against
schwarz-dev-wire (release CLI). Scope shipped: the conversation and user
lists of the metadata sync decode entry by entry; an undecodable entry
is logged and skipped. Archived with this header.

## Problem (verified in code 2026-09-28)

`sync_metadata` — the first phase of every sync, polling and push alike
— reads `POST /conversations/list` into `ConversationsResponse`, whose
`found: Vec<OwnConversation>` decodes strictly: `type` must be 0–3,
`protocol` one of `proteus` / `mls` / `mixed`. A single conversation of
a kind the backend adds later fails the whole chunk (up to 1000), the
metadata sync fails, and with it the whole sync — no room, no message.
The display-name backfill in the same phase has the same shape:
`POST /list-users` → `ListUsersResponse.found: Vec<UserProfile>`, where
one profile without a `name` fails the sync although the backfill is
documented as best-effort.

## Decisions (2026-09-28)

- **D1 — decode the lists element by element.** An entry of
  `found` that does not decode is logged (warn, with its id when it has
  one) and skipped; the rest of the list stands. Same principle as for
  unknown message types: read, log, discard — never block everything.
- **D2 — scope:** the two lists of the metadata sync
  (`ConversationsResponse.found`, `ListUsersResponse.found`). Responses
  to Kupferdraht's own requests (create a conversation, fetch a 1:1) stay
  strict — they only carry kinds Kupferdraht asked for.

## TDD (sketch)

`tests/step_99_tolerant_conversation_list.rs`: a conversation list with
one unknown `type`, one unknown `protocol` and one ordinary group →
the sync succeeds and the group is there; a user list with one profile
that does not decode → the sync succeeds and the room is there.

**As built (2026-09-28).**
- `wire/dto.rs`: `skip_undecodable_entries` (serde `deserialize_with`)
  reads the array as JSON values and decodes each on its own;
  `decode_or_skip` warns with the entry's `qualified_id.id`. Applied to
  `ConversationsResponse.found` and `ListUsersResponse.found`.
- Tests: `tests/step_99_tolerant_conversation_list.rs` (2 red → green;
  red: `Encoding("list_conversations body: error decoding response
  body")` / `…list_users…` from `sync_once`). Gate 2026-09-28: 963
  passed.
- Smoke 2026-09-28 (release CLI, schwarz-dev-wire): `sync-metadata`
  (7 rooms), `sync`, a message from user 1 reaches user 2 — no
  regression on real data. An unknown conversation kind cannot be
  produced on the test instance; the tolerance rests on the tests.
