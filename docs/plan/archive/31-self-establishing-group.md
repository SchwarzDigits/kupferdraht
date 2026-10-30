# Step 31 — Self-establishing MLS group (sendable on create)

> **Status:** implemented. A freshly-created room is now advanced from MLS
> epoch 0 to epoch 1 by an establishing commit on `create_room`, so it is
> sendable immediately instead of being inert until the first invite. Corrects
> the resolution of `CHANGELOG.md` §25 (superseded by §61). Committed on
> `step/31-*`; unit-tested + live-validated against Wire production
> (2026-06-16; `room-create` → epoch 1, `send` accepted). Archived from PLAN.md.
>
> **Sub-steps:** none (one branch, one commit).
>
> **Branch / commit prefix:** `step/31-*`, `feat(rooms): …`.

---

## Why

A new MLS group starts at **epoch 0**, where Wire rejects application messages
(`mls-protocol-error: "Application messages at epoch 0 are not supported"`).
Until this step `create_room` deliberately left a zero-invitee room inert at
epoch 0 — `send_text` / `send_knock` / `send_location` / `send_file` all failed
until the first `Room::invite` produced an Add-commit that advanced the epoch.
That made "create a room, then type a message" impossible without a separate
invite step.

`CHANGELOG.md` §25 had concluded this was unavoidable (three self-update
bootstrap attempts were rejected by Wire). **That conclusion was wrong.** Wire
accepts a group-establishing commit on create — the same way Wire's own client
brings a solo conversation online. §25's attempts failed on framing/params, not
because Wire refuses a self-update.

## Approach

`create_room` runs `Client::establish_group` right after the local
`create_group`, before returning the `Room`:

1. Claim the creator's own remaining clients' KeyPackages via
   `claim_key_packages_for_users(&[])` — the §27 self-bootstrap claim, which
   skips the current device (`skip_own`).
2. Branch:
   - **Non-empty** (the creator has other devices) → **Add-commit**
     (`MlsClient::add_members_commit`, reused from the invite path).
   - **Empty** (solo / single device) → **self-update commit**
     (`MlsClient::self_update_commit`, new): openmls `self_update` with default
     `LeafNodeParameters` (same credential, no re-sign, no `force_self_update`),
     serialized as `commit || group_info` via `serialize_bundle`.
3. Upload + merge via the existing `upload_and_merge_commit_bundle`
   (`POST /v14/mls/commit-bundles`, `message/mls`), then `sync_metadata` to
   refresh the cached room at its epoch-1 state. On rejection the pending commit
   is cleared and the error surfaces — the room is left at epoch 0 (the prior
   inert state), local state clean.

Commits ship as **PublicMessage** under `MIXED_PLAINTEXT_WIRE_FORMAT_POLICY`
(already on the create config) — Wire's bundle parser cannot read a
`PrivateMessage` commit and folds it into "Missing commit".

## Why §25 failed (re-diagnosed — see observations.md §61)

- *empty `LeafNodeParameters` → "Missing commit"*: the commit was
  `PrivateMessage`-framed; `MIXED_PLAINTEXT` (added later for the Add path) fixes
  this, but the self-update path was abandoned before benefiting.
- *explicit credential params → "Client identity does not match"*: changed
  credential params re-signed the leaf. The correct call uses **default**
  `LeafNodeParameters`.
- *`force_self_update(true)` → "mls-client-mismatch"*: not needed; a plain
  `self_update` keeping the same credential is what Wire expects.

## Tests

- Unit (`group_create.rs`): `self_update_commit` produces a non-empty bundle
  whose commit is `PublicMessage` (locks the framing §25 got wrong) and is a
  valid, mergeable commit.
- The full `create_room` round-trip (claim + commit-bundle upload) is only
  provable live — no integration test drives it.

## Verification

- Live smoke (the real test): CLI `room-create` reports `epoch: 1`, and a
  subsequent `send` is accepted (no `mls-protocol-error`). **Confirmed
  2026-06-16.**
- Gate: `cargo fmt --check`, build 0 warnings, `clippy -D warnings`,
  `cargo test --workspace`, `cargo deny`.

## Out of scope / follow-up

- The multi-device Add branch (creator with a second device) reuses the same
  `claim_key_packages_for_users` logic as `invite`; live-validated only for the
  solo self-update path so far. A second-device smoke would close the last gap.
