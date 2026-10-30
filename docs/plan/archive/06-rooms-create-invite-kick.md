# Step 6 — Rooms: create, invite, kick, members

> **Status:** merged — Step 6 (Rooms: create / invite / kick), 6a–6d all merged. 6a live 2026-04-27 (epoch 0; bootstrap → 6b, CHANGELOG.md §25); 6b live 2026-04-28 (epoch 0→1, send unblocked; CHANGELOG.md §25–§27); 6c live 2026-04-29 (epoch 0→1→2; observations.md §28); 6d live 2026-04-29 (`Room::kick` via Remove-commit, 3→2 members, send after kick ok). Archived from PLAN.md.
>
> **Sub-steps:** 6a, 6b, 6c, 6d.
>
> **Historical names:**
> - "Step 6" — current.
>
> **Branch / commit prefix:** `step/6-*`, `feat(step-6): …`, `chore(step-6): …`.

---

## Step 6 — Rooms: create, invite, kick, members

Outgoing MLS operations. Decomposed into sub-steps that
mirror the MLS-dance granularity (LEARNINGS §2.3 — per
sub-step commits worked well in the first pass and stay).
Deferred until **after** Step 5 because the first pass
hit extreme problems here (ciphersuite, X509 capabilities,
Z-Client header, GroupInfo framing). Sequencing room
creation after a proven send/receive round-trip on
externally-created rooms (Steps 3–5) means Steps 6a–6d
run against an already-validated ciphersuite/credential/
transport baseline.

### matrix-rust-sdk analog

`Client::create_room`, `Room::invite_user_by_id`,
`Room::kick_user`, `Room::members`. Kupferdraht's shape follows
these directly.

Source references:

- `crates/matrix-sdk/src/room/mod.rs` — invite/kick shapes.
- openmls examples in `reference/matrix-rust-sdk` are not
  applicable (Matrix uses Olm/Megolm, not MLS); use openmls
  `tests/external_commit.rs` and `src/group/mls_group/*` as
  direct references.

### Public API

```rust
impl Client {
    pub async fn create_room(
        &self,
        opts: CreateRoomOptions,
    ) -> Result<Arc<Room>, KupferdrahtError>;

    pub async fn create_channel(
        &self,
        opts: CreateChannelOptions,
    ) -> Result<Arc<Room>, KupferdrahtError>;
}

pub struct CreateRoomOptions {
    pub name: String,
    pub invitees: Vec<String>,          // qualified user IDs
}

impl Room {
    pub async fn invite(
        &self,
        user_ids: Vec<String>,
    ) -> Result<InviteOutcome, KupferdrahtError>;

    pub async fn kick(
        &self,
        user_ids: Vec<String>,
    ) -> Result<(), KupferdrahtError>;
}

// `Room::refresh_members` ships in Step 3 — both invite and
// kick should call it internally to keep `room_members` in
// sync with the new MLS epoch.

pub struct InviteOutcome {
    pub requested: u32,
    pub invited: u32,       // excludes users with empty KeyPackage pools
    pub skipped: Vec<String>,
}
```

### Pre-conditions (established earlier — not re-implemented here)

Step 6 inherits these from earlier steps and assumes them
green. Treat as regression checkpoints, not implementation
work:

- `MlsClient` with P-256 signing key, `BasicCredential`,
  `[Basic, X509]` leaf capabilities — built in **Step 1**
  (cross-cutting `encryption/` setup).
- Initial KeyPackage batch (`POST /v14/mls/key-packages/self/{device}`)
  uploaded at login — built in **Step 1** (client bootstrap).
- External-senders extension wired from the backend removal
  key (`GET /v14/mls/public-keys`) — built in **Step 3**
  (external-commit join needs it).

If any of these regress, fix in the originating step — do
not patch in Step 6.

### Sub-steps

| Sub-step | Scope | Wire API v14 | RFC 9420 | matrix-rust-sdk analog |
|---|---|---|---|---|
| 6a | Zero-invitee `create_room` — build empty local group, POST conversation, no commit needed | `POST /v14/conversations` | §11 (group creation) | `Client::create_room` with no invitees |
| 6b | With-invitees `create_room(users)` — claim KeyPackages, stage add commit, upload bundle | `POST /v14/mls/key-packages/claim/{domain}/{user}`, `POST /v14/mls/commit-bundles` | §12.1.1 (Add), §12.4 (Commit), §12.4.3 (GroupInfo) | `Client::create_room` with invitees |
| 6c | `Room::invite` on an already-created room | same as 6b applied to a loaded group | §12.1.1 | `Room::invite_user_by_id` |
| 6d | `Room::kick` (invite/kick auto-refresh members via Step 3's `refresh_members`) | `POST /v14/mls/commit-bundles` (for kick) | §12.1.2 (Remove) | `Room::kick_user` |

### Known-answer checklist (from LEARNINGS)

- **Ciphersuite 2** (P-256) throughout — not 1. LEARNINGS §1.2.
- **Leaf capabilities** declare both `Basic` and `X509` credential
  types. LEARNINGS §1.3.
- **Z-Client** via access-token binding, never client-supplied.
  LEARNINGS §1.4.
- **Claim ciphersuite query param is decimal** (e.g. `2`, not
  `0x0002`). `docs/wire/api-notes.md` §4.
- **Always pass `skip_own=<own_device_id>` on claim** — Wire
  defensively returns empty pools without it (docs/observations.md
  resolved §7).
- **Commit bundle segment order**: `commit || group_info ||
  welcome?`. Every segment **MlsMessage-framed**. The
  GroupInfo needs explicit wrapping via `MlsMessageOut::from(GroupInfo)`.
  LEARNINGS §1.5.

### Wire Message Types

None — Step 6 is membership management, not user-visible
content. Commits carry MLS-protocol messages, not
`GenericMessage`.

### Store additions

- `mls_group_state` (openmls-managed, via `SqliteStorageProvider`).
- `key_package_pool` counter per ciphersuite (for future
  top-up logic; Step 6 ships the initial 100 only, top-up is
  not yet scheduled).

### TDD starter tests

6a:
- `create_room(name, no invitees)` persists a `rooms` row
  with epoch 0.

6b:
- Commit bundle bytes begin with MlsMessage framing for
  the commit (version 0x0001, wire_format PublicMessage or
  PrivateMessage).
- `group_info` segment begins with MlsMessage framing
  (version 0x0001, wire_format GroupInfo = 0x0004).
- Response `201` from `/v14/mls/commit-bundles` is accepted
  as success (`is_success()` covers 200/201).

6c:
- `Room::invite` on a loaded group advances the local epoch.

6d:
- `Room::kick` resolves user IDs to leaf indices via
  `group.members()` and removes all leaves matching the
  `{uuid}@{domain}` prefix of any provided user ID.

### CLI command

```
kupferdraht-cli room-create -n "Name" --user <arg> --user <arg>
kupferdraht-cli channel-create -n "Name"
kupferdraht-cli room-invite --id <room-id> --user <arg>
kupferdraht-cli room-kick   --id <room-id> --user <arg>
```

`room-members` was introduced in Step 3 — it works on
created rooms identically.

`--user <arg>` accepts `<uuid>@<domain>`, `@handle`, or
`@handle@domain`. Handle resolution via
`POST /v14/list-users` (qualified-handles shape).

### Smoke test

```bash
ROOM=$(kupferdraht-cli room-create -n "Smoke $(date +%H%M%S)" --user "@alice" | awk '/ID:/ {print $2}')
kupferdraht-cli room-members --id "$ROOM"           # from Step 3
kupferdraht-cli room-invite  --id "$ROOM" --user "@bob"
kupferdraht-cli room-members --id "$ROOM"           # second invitee now listed
kupferdraht-cli room-kick    --id "$ROOM" --user "@bob"
kupferdraht-cli room-members --id "$ROOM"           # back to original two
# on the counterpart's iOS/desktop: the new room appears, invitee
# receives a Welcome, kicked user sees the room removed.
```

### CLI commands

**Step 6 — room CRUD:**

| Command | Options |
|---|---|
| `room-create` | `-n/--name`, `--user <arg>` repeatable |
| `channel-create` | same, `--channel` implied |
| `room-invite` | `--id`, `--user <arg>` repeatable |
| `room-kick` | `--id`, `--user <arg>` repeatable |

`room-members` was introduced in Step 3 and remains
available for created rooms.

`--user <arg>` accepts `<uuid>@<domain>`, `@handle`, `@handle@domain`.
