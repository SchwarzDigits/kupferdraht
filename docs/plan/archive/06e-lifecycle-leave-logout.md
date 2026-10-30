# Step 6e — Lifecycle: leave + logout

> **Status:** merged — 6e-1 `Room::leave` (server-driven REST) live 2026-04-30 (observations.md §31); 6e-2 `Client::logout` (with optional `--purge-device`) live 2026-04-30 (soft logout + `--purge-device`). Archived from PLAN.md.
>
> **Sub-steps:** 6e-1 (Room::leave), 6e-2 (Client::logout). Landed between Step 6 and Step 6-II.
>
> **Historical names:**
> - "Step 6e" — current.
>
> **Branch / commit prefix:** `step/6e-*`, `feat(step-6e): …`, `chore(step-6e): …`.

---

## Step 6e — Lifecycle: leave + logout

Closes the membership-management chapter opened in Step 6.
6a–6d cover *outbound* membership operations against other
users (create / invite / kick). 6e covers the *self*-side:
remove your own leaf from a room (`Room::leave`) and revoke
the device's access token server-side (`Client::logout`).

Self-kick was explicitly rejected in 6d ("logout /
device-revocation lifecycle is a future Step"). 6e is that
future step. It must land before any production rollout —
without `Client::logout`, the only way for a user to sign
out is `rm -rf ~/.kupferdraht`, which leaves the device's access
token live on Wire until it expires.

### matrix-rust-sdk analog

- `Room::leave()` — emits a Remove proposal for the caller's
  own leaf and commits.
- `Client::logout()` — invalidates the access token, drops
  local session state, optionally deletes the registered
  device.

Source references:

- `crates/matrix-sdk/src/room/mod.rs::Room::leave` — leave
  shape.
- `crates/matrix-sdk/src/client/mod.rs::Client::logout` —
  logout shape.

### Sub-steps

| Sub-step | Scope | Wire API v14 | RFC 9420 |
|---|---|---|---|
| 6e-1 | `Room::leave` — self-Remove commit, Wire `DELETE /v14/conversations/{id}/members/{self}` is *not* used (MLS-managed; commit is the source of truth). Local row marked left, MLS group dropped. | `POST /v14/mls/commit-bundles` (Remove of self-leaf) | §12.1.2 (Remove) |
| 6e-2 | `Client::logout` — `POST /v14/access/logout` revokes the access token; if `purge_device=true`, also `DELETE /v14/clients/{device}` and wipes the local DB. | `POST /v14/access/logout`, optional `DELETE /v14/clients/{id}` | (none) |

### Public API

```rust
impl Client {
    pub async fn logout(
        &self,
        purge_device: bool,
    ) -> Result<(), KupferdrahtError>;
}

impl Room {
    pub async fn leave(&self) -> Result<(), KupferdrahtError>;
}
```

`Client::logout` already appears in the CLI design block
(line 455 + line 489); 6e is where it actually lands.

### Wire Message Types

None — both operations are protocol-level, not user-visible
content. `Room::leave` produces an MLS Remove commit, never
a `GenericMessage`.

### Implementation notes

- **Self-Remove is just `MlsGroup::remove_members(self_leaf_idx)`.**
  The kick path from 6d already supports this — 6d only
  *blocked* the self-case at the orchestrator level. 6e
  unblocks it via a dedicated `leave()` entry point.
- **After self-Remove, drop the openmls group state.** The
  caller is no longer a member; future commits in that group
  cannot be processed. Mark the local `rooms` row as
  `state='left'` and stop polling it.
- **`Client::logout` order matters:** revoke the access token
  *first*, so the network is dead before we touch the local
  DB. If `DELETE /v14/clients/{id}` is requested but fails,
  the local cleanup still proceeds — the device row may be
  left orphaned on Wire (admin can clean up; user can also
  delete it via Wire web). Document this in
  `docs/wire/api-notes.md`.
- **`purge_device=false`** keeps the registered device alive
  on Wire and just clears the access token + local session.
  Useful for "logout but keep this device registered for next
  login" UX. The default in CLI is `false`; `--purge-device`
  opts in.

### TDD starter tests

6e-1:
- `Room::leave` produces a commit bundle whose only action
  is Remove(self_leaf_idx).
- After `leave()`, `rooms.state` flips to `'left'`.
- `Timeline::send_text` on a left room returns
  `KupferdrahtError::NotInRoom`.

6e-2:
- `Client::logout(purge_device=false)` calls
  `POST /v14/access/logout` and clears the in-memory token.
- `Client::logout(purge_device=true)` additionally calls
  `DELETE /v14/clients/{device}` and wipes
  `~/.kupferdraht/<account>/`.

### CLI commands

```
kupferdraht-cli room-leave --id <room-id>
kupferdraht-cli logout [--purge-device]
```

### Smoke test

```bash
kupferdraht-cli room-leave --id "$ROOM"
kupferdraht-cli rooms                   # left room no longer listed
kupferdraht-cli send --id "$ROOM" -m "x" # fails: NotInRoom

kupferdraht-cli logout
kupferdraht-cli rooms                   # fails: NotLoggedIn (exit 2)
```
