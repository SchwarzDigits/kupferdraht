# Step 29 — Decompose `client/runtime.rs`

> **Status:** MERGED to `main` — commits `a678a13`..`a3cc0f2` (29a-f),
> PR #22 (merge `95315d0`, 2026-06-16). Scope: pure structure —
> `client/runtime.rs` decomposed into `client/{mod,session,profile,
> sync,rooms,membership,timeline_read}.rs`; no public API / FFI / Wire
> change. No smoke (refactor).
>
> **Sub-steps:**
> - 29a — scaffold: `Client` struct + `new` + shared types land in
>   `client/mod.rs`; declare child modules; `runtime.rs` becomes a thin
>   re-export shim (one commit, zero behaviour change).
> - 29b — `client/session.rs` (session getters, token refresh,
>   `TokenRefresh`, `logout`, crate-private *token* helpers)
>   **and** `client/profile.rs` (self-account mutators
>   `set_display_name`, `set_handle`, `update_email`, `change_password` —
>   the `ffi/admin.rs` surface; a distinct concern, **not** token/session,
>   so a separate file in the same PR). The crate-private `http_*`
>   wrappers do **not** all land in session — see Target layout
>   (`http_calls.rs`).
> - 29c — `client/sync.rs`: `sync*` family, `sync_metadata`,
>   `add_event_handler`, `commit_pending_external_removal`, room-cache
>   helpers used only by sync (`persist_rooms`, `backfill_user_profiles`,
>   `collect_conversation_ids`, `load_rooms_from_store`).
> - 29d — `client/rooms.rs`: room-list accessors (`rooms`, `room`,
>   `room_or_load`, `joined_rooms`, `invited_rooms`, `rooms_filtered`),
>   create/DM (`create_room`, `create_dm`, `dm_room`, `dm_with`),
>   room metadata mutations (`rename_room`, `delete_room`,
>   `set_room_access`, join-link CRUD, `join_via_link`).
> - 29e — `client/membership.rs`: MLS membership lifecycle
>   (`invite_members`, `kick_members`, `leave_room`, `join_room`,
>   `join_all_rooms`, `resync_existing_rooms`, `resync_room`,
>   `sync_members` + private helpers `upload_and_merge_commit_bundle`,
>   `claim_key_packages_for_users`, `resolve_members`, …).
> - 29f — `client/timeline_read.rs` + `client/mentions.rs`: move
>   `messages`, the timeline-listener registry (`add_timeline_listener`,
>   `dispatch_timeline_diff`), `paginate_timeline_rows`,
>   `apply_timeline_overlays`; and mention resolution
>   (`resolve_mentions_in_body`, `scan_at_handles`, `resolve_qualified_handles`,
>   plus the `AtHandleCandidate` struct and `is_handle_byte` helper).
>   Move `impl Client` typing methods (`send_typing_started`/`_stopped`,
>   `notify_typing`, `record_incoming_typing`, `reset_typing_on_sync_start`,
>   `http_send_typing`) into existing `client/typing.rs`; delete
>   `runtime.rs`; optional rename `mls_client` → `mls` (closes audit §A.3).
>
> **Historical names:**
> - "Step 29" — current.
>
> **Branch / commit prefix:** `step/29*-*`, `chore(step-29*): …`.

---

## Step 29 — Decompose `client/runtime.rs`

`kupferdraht-core/src/client/runtime.rs` is ~3200 lines total, ~1741 **code**
lines (CLAUDE.md count rule, `#[cfg(test)]` excluded). It hosts the
`Client` struct and essentially every `impl Client` method added since
Step 1 — sync, rooms, profile, timeline read, typing, token refresh,
membership, join links. The name `runtime` was meant as "the live session
after `ClientBuilder::login` / `::resume`", but in Rust it reads like
"async runtime" and does not describe any single responsibility.

This step is **pure structure**: move code, no behaviour change. Closes
[`open.md`](../../open.md) Backlog → Code quality (cq-A.1) row
for `runtime.rs`. Optionally closes §A.3 (`mls_client` field rename)
in 29f when touching the struct definition.

### Why now

- Step 28 added more `impl Client` surface (`commit_pending_external_removal`).
- `client/` already splits construction (`builder`, `login_pipeline`,
  `resume`) and typing state (`typing.rs`); only the operational
  `Client` body remains monolithic.
- UniFFI / FFI parity is unaffected — no `pub` signature changes.

### Target layout

Rust allows multiple `impl Client` blocks in child modules of `client/`;
they retain access to `Client`'s private fields.

```
kupferdraht-core/src/client/
  mod.rs              # Client struct, TokenState, JoinSummary, ResyncSummary,
                      # Client::new, Debug, mod declarations, pub use, AND the
                      # cross-module private plumbing used by several children:
                      # mls_client_required, group_id_for_room, has_mls_group,
                      # assets_dir, store_handle, weak, unqualified_room_id
  session.rs          # getters, token refresh, TokenRefresh, logout
  profile.rs          # self-account mutators: set_display_name, set_handle,
                      # update_email, change_password (the ffi/admin surface)
  http_calls.rs       # crate-private `http_*` thin wire wrappers Client exposes
                      # to sibling modules (fetch_notifications, up/download_asset,
                      # fetch_feature_configs, fetch_property_value, *_client, http_send)
  sync.rs             # sync* + event-handler registration + external-remove dispatch hook
  rooms.rs            # list + create/DM + room metadata + join links
  membership.rs       # invite / kick / leave / join / resync / sync_members
  timeline_read.rs    # Client::messages, timeline listeners, pagination helpers
  mentions.rs         # @handle scan (AtHandleCandidate, is_handle_byte) + list-users resolution
  typing.rs           # TypingTracker (existing) + impl Client typing methods
  builder.rs          # unchanged
  …                   # other existing modules unchanged
```

Placement rationale: the `http_*` wrappers serve **different** concerns
(sync, assets, feature-configs, devices) — they are grouped by "thin
crate-private wire wrapper" into `http_calls.rs` rather than scattered or
dumped into `session.rs`. The handful of cross-module private helpers
(`mls_client_required`, `group_id_for_room`, …) stay in `mod.rs` next to
the struct so every child `impl Client` block reaches them without a
re-export.

`client/session.rs` today is a Step-1a placeholder — **replaced** by
the session/token module above (no conflict: placeholder never shipped
logic).

`runtime.rs` is **deleted** after 29f. `client/mod.rs` re-exports
`Client`, `JoinSummary`, `ResyncSummary` (same as today via
`pub use runtime::…`).

### matrix-rust-sdk analog

N/A — file organisation only. Public API shape stays
`matrix_sdk::Client`-aligned; only internal module boundaries change.

### Public API

**None.** Every `pub` / `pub(crate)` symbol keeps the same path
(`crate::client::Client`, …). No `docs/library.md` or
`docs/api-mapping-matrix-rust-sdk.md` update unless a doc explicitly
mentions `runtime.rs` as a path (grep-and-replace in the same commit).

### Wire Message Types / Wire API v14

None.

### TDD / test strategy

Not a feature step — **no new failing tests**. Validation gate:

1. `cargo fmt --all -- --check`
2. `cargo build --workspace` (0 warnings)
3. `cargo clippy --workspace --all-targets -- -D warnings`
4. `cargo test --workspace --no-fail-fast`
5. `cargo deny check`

Unit tests currently inside `runtime.rs` (`scan_at_handles`,
`paginate_timeline_rows`, `TYPING_AUTOSTOP`) move with their modules
(`#[cfg(test)]` blocks stay adjacent to the code they test).

Smoke baseline (`scripts/smoke-*.sh`) — run once after 29f merge; no
new Wire behaviour to validate.

### Sub-step execution notes

**29a — scaffold (mechanical, must stay green).**

- Move `Client`, `TokenState`, `JoinSummary`, `ResyncSummary`,
  `TYPING_AUTOSTOP`, and top-level private helpers that are shared
  (`unqualified_room_id`) into `mod.rs` (or keep helpers next to their
  only caller when splitting later).
- Add `mod session; mod sync; mod rooms; …` to `mod.rs`.
- Leave all method bodies in `runtime.rs` temporarily **or** move bodies
  in 29a only if the commit stays a pure `git mv` + `impl` block move
  with zero logic edits. Prefer: 29a = struct + `new` + empty child
  modules; 29b–29f = cut/paste `impl` sections.

**29b–29e — cut `impl Client` sections.**

- One sub-step per branch (`step/29b-session`, …) or batch 29b+29c if
  each merge stays reviewable (<500 LoC moved per PR).
- Preserve function boundaries — **do not** refactor logic while moving
  (no "while we're here" cleanups). Exception: splitting a function
  that already violates the 40-line cap is out of scope unless required
  for the move.
- After each sub-step: full test gate green.

**29f — tail + delete.**

- Move remaining timeline/mention/typing pieces.
- Delete `runtime.rs`; update `client/mod.rs` `pub use`.
- Grep repo for `client/runtime` references (`docs/observations.md`, plan docs,
  audit) — update paths in the same commit.
- Verify per-file code-line counts (CLAUDE.md awk rule): no new file
  should exceed 200 code lines **and** multiple responsibilities. If
  `membership.rs` still exceeds 200 lines, split further into
  `membership.rs` + `membership_join.rs` in a 29g follow-up (only if
  needed — document in docs/observations.md if deferred).

### Implementation pattern (reference)

```rust
// client/mod.rs
mod membership;
mod mentions;
mod rooms;
mod session;
mod sync;
mod timeline_read;
// typing.rs already exists

pub struct Client { /* fields unchanged */ }

impl Client {
    pub(super) fn new(/* … */) -> Arc<Self> { /* … */ }
    // crate-private accessors shared across child modules
}

pub use /* JoinSummary, ResyncSummary re-exported from mod or child */;

// client/sync.rs
use super::Client;

impl Client {
    pub async fn sync_once(&self, /* … */) -> Result</* … */> { /* … */ }
}
```

### Audit closures

- `docs/v1-code-smells-audit.md` §A.1 — `runtime.rs` row: mark resolved
  when 29f merges (or note partial if 29g needed).
- `docs/v1-code-smells-audit.md` §A.3 — `mls_client` field rename:
  optional in 29f, same commit as struct touch.

### Out of scope — explicitly _not_ in Step 29

- **`kupferdraht-cli/src/main.rs` decomposition** — separate step (audit §A.1
  second row; same class of work, different crate).
- **`sync/dispatch.rs` / `timeline/send.rs` splits** — audit §A.1
  siblings; not bundled here.
- **Public API renames** (`refresh_members`, new method names, etc.).
- **Moving logic off `Client` onto `Room`** — Step 14d territory;
  already partially done.
- **`MlsClient::process_inbound_message` rename** — audit §A.2;
  separate chore.
- **Behaviour changes** to `sync_metadata` vs `sync_once` semantics
  (CHANGELOG.md §46) — unchanged.
