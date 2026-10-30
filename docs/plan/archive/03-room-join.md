# Step 3 — Room join (external commit)

> **Status:** merged — Step 3 (Room join via external commit + `Room::members`) `85e2ade` (live 2026-04-25, epoch 1→2; Wire-MLS deviations W-1/W-3). Sub-steps: 3a `9b36af6` · 3b `457af38` · 3c `d553bea` · 3d `3b7c1ed` · 3e `21286cb` · 3f `521826c`. Archived from PLAN.md.
>
> **Sub-steps:** 3a, 3b, 3c, 3d, 3e, 3f.
>
> **Historical names:**
> - "Step 3" — current. (No renames; promoted from a late-stage cleanup footnote in PLAN.md v1 to a first-class primitive in v2 — that's a scope change, not a rename.)
>
> **Branch / commit prefix:** `step/3-*`, `feat(step-3): …`, `chore(step-3): …`.

---

## Step 3 — Room join (external commit)

A fresh Kupferdraht device is **not** a member of any Wire room
that existed before its first login. To send or receive in
such a room, it must join locally via MLS's external-commit
mechanism (LEARNINGS §2.1 — this came in too late in the
first pass; it belongs before Send/Receive).

### matrix-rust-sdk analog

No direct MLS equivalent. Matrix's `Room::join()` for invited
rooms is conceptually adjacent.

### Public API

```rust
impl Room {
    pub async fn join(&self) -> Result<(), KupferdrahtError>;

    /// Re-derive the member list from local MLS group state
    /// (openmls leaves) and resolve display info via Wire.
    /// Available immediately after `join` — no dependency on
    /// later steps. Persists to `room_members` in the Store.
    pub async fn refresh_members(&self) -> Result<Vec<RoomMember>, KupferdrahtError>;
}

impl Client {
    pub async fn join_all_rooms(&self) -> Result<JoinSummary, KupferdrahtError>;
}

pub struct JoinSummary {
    pub attempted: u32,
    pub joined: u32,
    pub already_joined: u32,
    pub failed: Vec<(String, String)>,   // (room_id, error_detail)
}
```

UDL extension on `Room` (added on top of Step 2's `Room`
interface):

```idl
interface Room {
    // ... from Step 2

    [Async, Throws=KupferdrahtError]
    void join();

    [Async, Throws=KupferdrahtError]
    sequence<RoomMember> refresh_members();
};
```

- `Room::join` joins one room. Used standalone (CLI
  `room-join`) and internally by `join_all_rooms`.
- `join_all_rooms` iterates `sync_metadata`'s room list,
  filters to those without local MLS state, and runs
  `Room::join` for each. Lands with the step but can be
  omitted from MVP; production UX layer.
- `Room::refresh_members` is pulled forward from Step 6
  (was 6g) into Step 3 because it's available the moment a
  group is loaded — and loading a group is exactly what
  `join` produces. Having members listable here removes
  the leaf-index → user-id mapping problems the first
  pass hit in late stages: every subsequent step (Send,
  Receive, Kick) operates on a member list that's already
  proven correct.

### Wire API v14 endpoints

- `GET /v14/conversations/{cnv_domain}/{cnv}/groupinfo` —
  fetch TLS-serialized `MlsMessage(GroupInfo)` (§5).
- `POST /v14/mls/commit-bundles` — upload external-commit
  bundle (§6).
- `POST /v14/list-users` (qualified-id shape) — resolve
  display names + handles for the leaf-derived user IDs
  (§8). Same endpoint Step 2 uses for 1:1 counterpart
  resolution; reused here for full member lists.

### RFC 9420 sections

- §12.5 — External Commits. Specifically §12.5.1 (Init
  Secret from Group Context) and §12.5.2 (External Commit
  validation).
- §12.4.3 — GroupInfo structure for external-commit input.

### Wire Message Types

None — external-commit payload is MLS-protocol only.

### Store additions

- `openmls_*` tables absorb new group state via
  `StagedWelcome::new_from_welcome` / finalized
  external-commit — no Kupferdraht-specific migration.
- `rooms` row for the joined room gains `epoch = epoch+1`
  after successful commit.
- `room_members` table (already created in Step 2) is
  re-populated from the local MLS group's leaves after
  `refresh_members`. Step 2 wrote what Wire's metadata
  said; Step 3's `refresh_members` overwrites with what
  the local MLS state authoritatively shows.

### Implementation notes

- GroupInfo parsing: try `MlsMessageIn::tls_deserialize` +
  `extract() == MlsMessageBodyIn::GroupInfo`. Fall back to
  `VerifiableGroupInfo::tls_deserialize` if Wire ever
  delivers raw (defensive — v14 should always deliver
  MlsMessage-framed per observations.md §15).
- External-commit builder chain via openmls (see `reference/
  matrix-rust-sdk` is not useful here — this is pure openmls;
  use `openmls-0.8.1/tests/external_commit.rs` as the
  canonical example).
- Leaf parameters for the external commit **must** declare
  `[Basic, X509]` capabilities explicitly — the default
  `Capabilities::default()` only lists Basic, which fails
  `LeafNodeValidation` on e2ei-enrolled groups (LEARNINGS §1.3).

### TDD starter tests

- GroupInfo bytes starting with `00 01 00 04 …` parse as
  `MlsMessage(GroupInfo)`.
- `Room::join` produces a commit bundle whose commit segment
  is `MlsMessage`-framed.
- External-commit with default capabilities fails; with
  `[Basic, X509]` capabilities succeeds (regression test).
- Post-join `Room::epoch()` matches server epoch + 1.
- `Room::refresh_members` after a successful `join`
  enumerates `group.members()` and yields one `RoomMember`
  per leaf, with `user_id` parsed from the leaf credential's
  `{uuid}@{domain}` identity.
- `Room::refresh_members` resolves display names + handles
  by batching the unique user IDs into one
  `POST /v14/list-users` call.
- Repeated `refresh_members` is idempotent — no duplicate
  rows in `room_members`.

### CLI command

```
kupferdraht-cli room-join --id <room-id>
kupferdraht-cli rooms-join-all                # runs join_all_rooms
kupferdraht-cli room-members --id <room-id>   # list members from local MLS state
```

### Smoke test

```bash
kupferdraht-cli sync
# pick an existing room from `rooms` output
kupferdraht-cli room-join --id <room-id>
kupferdraht-cli room-members --id <room-id>
# members must list every leaf in the local MLS group with
# resolved display name / handle. Compare against the
# counterpart iOS/desktop's own member list — should match.
# on the counterpart's iOS/desktop: a "new device" system message appears
```

### CLI commands

**Step 3 — join + members:**

| Command | Options |
|---|---|
| `room-join` | `--id` |
| `rooms-join-all` | — |
| `room-members` | `--id` |
