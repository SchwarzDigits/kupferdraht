# Step 39 — Single `Client` per account + resilient session handling

> **Status:** MERGED to `main` — `014cda7`, PR #33 (squash-merge, no
> separate merge commit), 2026-06-19. Scope: Kupferdraht-side half of the iOS
> session-handling fix — one `Client` per account, durable rotated-`zuid`
> persistence, typed terminal `SessionExpired`. The full cross-repo analysis
> + decisions live in natrium's `natrium-core/docs/session-invalidation.md`;
> this doc is the Kupferdraht implementation plan. Live verification pending (as
> of 2026-07-16) — the file's own TDD section states "real multi-instance /
> iOS-lifecycle behaviour needs device testing", and no later record
> confirms that happened.
>
> **Sub-steps:** none.
>
> **Branch / commit prefix:** `step/39-*`, `fix(step-39): …`.

---

## Why

iOS shows a repeating `rebind_access_token: 403 invalid-credentials` ("Invalid
zauth token") in the sync loop, and the consumer is never told to re-login. Root
analysis (see the natrium doc) found the `zuid` refresh cookie is an
**account-level singleton** (Wire keeps one valid generation, rotated on every
`/access`), but Kupferdraht serializes refresh only **per `Client`**, and
`manager.resume()` builds a **fresh `Client` on every call** with no cache. Two
coexisting Clients for one account race on the `zuid` (and the `mls.db` /
notification cursor) → one gets a 403. A few hours offline should *not* end a
session — this is a self-inflicted desync, not a dead account.

## Scope (Kupferdraht side)

Ordered; each part is independently valuable.

### A3 — one `Client` per account (the root fix)

- `manager/mod.rs` (`resume` @178, `login`): cache the live `Client` per
  `account_id` in a thread-safe registry (`Mutex<HashMap<String, Weak<Client>>>`).
  `resume()` returns the existing live instance instead of building + rebinding a
  new one; otherwise build, cache, return. `Weak` lets a fully-dropped Client
  fall out.
- Evict on `logout()` and on a confirmed `SessionExpired` (part B).
- Effect: only one `Client` per account exists, so the per-`Client`
  `refresh_lock` (`client/mod.rs:115`) becomes the de-facto account lock — the
  `zuid` / MLS-store / sync-cursor races all close.

### A-persist — durable rotated-`zuid` persistence

- In-session refresh (`client/session.rs:271-281`): the persist of the rotated
  `zuid` is **best-effort** today (failure only `warn!`d → silent desync across a
  process kill). Make it **synchronous + a hard error**.
- Persist immediately after `rebind` returns in all rebind paths
  (`session.rs:242-244`, `resume.rs:113-117`, `login_pipeline.rs`).
- Verify resume assembles the `Client` from the **post-rotation** session
  (`resume.rs` orchestration), not a pre-rotation copy.

### B — typed terminal session expiry (D3: narrow)

- `error.rs`: new `SessionExpired` variant (terminal; distinct from
  `BackendRejected`).
- `wire/http/auth.rs`: `rebind_access_token` (@111-117) parses the non-2xx body
  with the existing `WireErrorBody { label }` (the pattern `login()` already uses)
  and maps **`403` + `label == "invalid-credentials"`** → `SessionExpired`. Every
  other non-2xx stays `BackendRejected`.
- `client/session.rs`: `refresh_access_token` on `SessionExpired` **clears the
  persisted auth tokens** (`save_session` with empty `access_token`/`refresh_token`,
  `device_id` + `mls.db` + chats kept) so a later `resume` can't retry a dead
  cookie, then propagates `SessionExpired`.
- `ffi/sync.rs`: add `SyncListener::on_session_expired(&self)`; `run_sync_loop`
  (@384-420) matches `SessionExpired` → calls `on_session_expired()` + **stops**
  the loop (no retry).

natrium side (separate, in the natrium repo): single-session guard +
`EngineClient.close()` + route `on_session_expired` → `AuthEvent.LoggedOut`.

## Preserve vs clean (D2)

- **Preserve**: MLS state + chats — valid after re-login (Kupferdraht reuses the
  device + `mls.db` via `reuse_existing_device_if_valid` /
  `resume_existing_mls_state`). Never purge them.
- **Clean**: only the dead auth tokens.

## TDD / verification

- `manager`: `resume()` twice for one account returns the **same** `Arc<Client>`
  (pointer-eq); after `logout`/`SessionExpired` a fresh one is built.
- `wire/http`: `rebind_access_token` 403 `invalid-credentials` → `SessionExpired`;
  other 403 labels → `BackendRejected`.
- `ffi/sync.rs`: `run_sync_loop` calls `on_session_expired` and stops on
  `SessionExpired` (extend the `ScriptedListener` tests).
- `session.rs`: a `SessionExpired` refresh clears persisted tokens but keeps
  `device_id`.
- CI gate: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
  -- -D warnings`, `cargo test --workspace --all-targets`.
- Real multi-instance / iOS-lifecycle behaviour needs device testing.
