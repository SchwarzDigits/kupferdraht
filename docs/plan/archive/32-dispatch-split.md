# Step 32 — Decompose `sync/dispatch.rs` by interaction

> **Status:** MERGED to `main` — commit `4dde845` (32a-e squashed),
> PR #27 (merge `69b1778`, 2026-06-17). Scope: pure structure —
> `sync/dispatch.rs` decomposed by interaction into a `sync/dispatch/`
> folder; router stays in `service.rs`; no public API / FFI / Wire
> change. No smoke (refactor).
>
> **Sub-steps:**
> - 32a — scaffold: `git mv sync/dispatch.rs sync/dispatch/mod.rs` (the
>   file becomes the folder's module root). `mod dispatch;` in
>   `sync/mod.rs` is unchanged — it now resolves to `dispatch/mod.rs`.
>   Pure relocation, zero behaviour change, must stay green. The
>   per-interaction handlers move out in 32b–32e.
> - 32b — `dispatch/text.rs`: `dispatch_inbound_text` (+ `persist_inbound_text`,
>   `mentions_to_records`).
> - 32c — `dispatch/reaction.rs` (`dispatch_inbound_reaction` +
>   `parse_reaction_emoji_csv` **and its `#[cfg(test)]` block**),
>   `dispatch/delete.rs` (`dispatch_inbound_delete`), `dispatch/edit.rs`
>   (`dispatch_inbound_edit` + `persist_inbound_edit`).
> - 32d — `dispatch/confirmation.rs` (`dispatch_inbound_confirmation` +
>   `apply_confirmation_receipts` + `maybe_send_delivery_confirmation` +
>   `collect_confirmation_ids`), `dispatch/asset.rs` (`dispatch_inbound_asset`
>   + `DecodedAsset` + `decode_inbound_asset` + `persist_inbound_asset` +
>   `asset_timeline_item`).
> - 32e — `dispatch/knock.rs`, `dispatch/location.rs`, `dispatch/config.rs`
>   (`dispatch_feature_config_update` + `dispatch_user_property_set` +
>   `dispatch_user_property_delete`); finalize `dispatch/mod.rs` (shared
>   helpers only); apply the `pub(super)`→`pub(crate)` visibility fix; mark
>   the audit row resolved.
>
> **Historical names:**
> - "Step 32" — current.
>
> **Branch / commit prefix:** `step/32*-*`, `chore(step-32*): …`.

---

## Step 32 — Decompose `sync/dispatch.rs` by interaction

`kupferdraht-core/src/sync/dispatch.rs` is ~703 **code** lines (CLAUDE.md
count rule; the file's `#[cfg(test)]` block — the `parse_reaction_emoji_csv`
tests — is excluded from the count). It is the **receive-side** half of
the message pipeline: once `sync/service.rs` drains an event off Wire's
stream and decrypts the MLS message, `dispatch.rs` routes the decoded
`GenericMessage` variant to a per-content-variant handler and writes the
result into the local store.

Despite the name "dispatch", the file is a grab-bag of **every inbound
interaction's handler** — `dispatch_inbound_{text, knock, location,
confirmation, reaction, delete, edit, asset}` plus the Step-10d
server-config arms (`dispatch_feature_config_update`,
`dispatch_user_property_set`, `dispatch_user_property_delete`) —
interleaved with their persistence helpers. The natural SRP axis is the
**interaction**, exactly the axis Step 30 used to split the *send* side
(`timeline/send.rs`).

This step is **pure structure**: move code, no behaviour change. Closes
the `sync/dispatch.rs` file-size row in
[`open.md`](../../open.md) Backlog → Code quality (cq-A.1).

### Why now

- `dispatch.rs` (703) is the largest remaining timeline-interaction file
  after Step 30 split the send side.
- The interaction axis is established (`timeline/{text,reaction,…}.rs`
  from Step 30), so this is consistency work, not a new pattern.
- It buys **send ↔ receive symmetry**: `timeline/reaction.rs` (send) now
  has a sibling `sync/dispatch/reaction.rs` (receive); a reviewer landing
  on a reaction regression reads one file per direction.

### Sibling of Step 30 (send) — the receive counterpart

Step 30 decomposed `timeline/send.rs` (the **outbound** orchestrators);
Step 32 decomposes `sync/dispatch.rs` (the **inbound** handlers) of the
*same* interactions. The Step 30 plan flagged this explicitly as out of
its own scope — "`sync/dispatch.rs` (inbound) split … is its own step."
No overlap: different files, different control-flow entry point (the send
side is API/user-driven and lives in `timeline/`; the receive side is
event-loop-driven and lives in `sync/`). Pairing them per interaction is
the payoff, not merging them into one module.

### Target layout

Folder, not flat files. `dispatch` is a cohesive subsystem (the whole
inbound-routing surface), so it earns its own directory — and it keeps
`sync/` from gaining ~9 `dispatch_*.rs` siblings next to `service.rs`,
`event_handler.rs`, `settings.rs`. (Step 30 *deleted* `send.rs` because
"send" is not a real responsibility; "dispatch" is, so the name stays as
the folder.)

```
kupferdraht-core/src/sync/
  dispatch/
    mod.rs           # module doc + `mod` declarations + the shared receive
                     #   helpers: persist_inbound_message,
                     #   dispatch_inbound_simple, message_to_record
    text.rs          # dispatch_inbound_text (+ persist_inbound_text,
                     #   mentions_to_records)
    knock.rs         # dispatch_inbound_knock
    location.rs      # dispatch_inbound_location
    confirmation.rs  # dispatch_inbound_confirmation (+ apply_confirmation_receipts,
                     #   maybe_send_delivery_confirmation, collect_confirmation_ids)
    reaction.rs      # dispatch_inbound_reaction (+ parse_reaction_emoji_csv + tests)
    delete.rs        # dispatch_inbound_delete
    edit.rs          # dispatch_inbound_edit (+ persist_inbound_edit)
    asset.rs         # dispatch_inbound_asset (+ DecodedAsset, decode_inbound_asset,
                     #   persist_inbound_asset, asset_timeline_item)
    config.rs        # dispatch_feature_config_update + dispatch_user_property_set
                     #   + dispatch_user_property_delete  (Step 10d)
  service.rs         # unchanged: the drain loop + the router
                     #   (dispatch_event → dispatch_mls_message) that calls
                     #   the handlers. Also keeps dispatch_inbound_typing
                     #   (typing receive already lives here, not in dispatch).
```

Each child file holds one interaction's inbound handler (an
`impl SyncService` method) next to its private persistence helpers —
mirroring Step 30's `timeline/{interaction}.rs`. Rust allows multiple
`impl SyncService` blocks across child modules of `sync/dispatch/`.

### Function → file mapping (authoritative, from the current file)

| Destination | Items |
|---|---|
| `dispatch/mod.rs` | `persist_inbound_message`, `dispatch_inbound_simple`, `message_to_record` (shared); module doc; `mod` decls |
| `dispatch/text.rs` | `dispatch_inbound_text`, `persist_inbound_text`, `mentions_to_records` |
| `dispatch/knock.rs` | `dispatch_inbound_knock` |
| `dispatch/location.rs` | `dispatch_inbound_location` |
| `dispatch/confirmation.rs` | `dispatch_inbound_confirmation`, `apply_confirmation_receipts`, `maybe_send_delivery_confirmation`, `collect_confirmation_ids` |
| `dispatch/reaction.rs` | `dispatch_inbound_reaction`, `parse_reaction_emoji_csv` + its `#[cfg(test)]` tests |
| `dispatch/delete.rs` | `dispatch_inbound_delete` |
| `dispatch/edit.rs` | `dispatch_inbound_edit`, `persist_inbound_edit` |
| `dispatch/asset.rs` | `dispatch_inbound_asset`, `struct DecodedAsset`, `decode_inbound_asset`, `persist_inbound_asset`, `asset_timeline_item` |
| `dispatch/config.rs` | `dispatch_feature_config_update`, `dispatch_user_property_set`, `dispatch_user_property_delete` |

`knock.rs` + `location.rs` are tiny (one handler each, both lean on the
shared `dispatch_inbound_simple`). They MAY be folded into a single
`dispatch/simple.rs` if the reviewer prefers; the plan keeps them
separate to match Step 30's `timeline/knock.rs` / `timeline/location.rs`
("one interaction, one file").

### The one scaffold detail — visibility

The handlers are `pub(super) async fn` on `SyncService` so the router in
`sync/service.rs` (`dispatch_mls_message`) can call them. `SyncService`
is defined in `sync/service.rs` (`sync::service`). Two things follow:

1. **Call reach.** `pub(super)` from today's `sync::dispatch` reaches
   `sync`, so `service.rs` can call the handlers. Once a handler moves
   into `sync::dispatch::{interaction}`, its `pub(super)` only reaches
   `sync::dispatch` — `service.rs` (in `sync`, not under `sync::dispatch`)
   can no longer see it. **Fix:** widen each relocated handler to
   `pub(crate)` — the minimal change, the same cross-module-visibility
   pattern Step 29 used. The router stays in `service.rs` untouched.
2. **Field access is unaffected.** The handlers reach `SyncService`'s
   state through methods / already-visible (`pub(crate)`) fields — they
   must, because today's `sync::dispatch` is *not* a descendant of
   `sync::service` and still compiles. Moving a handler deeper (to
   `sync::dispatch::text`) keeps the same non-descendant relationship to
   `sync::service`, so it introduces **no new** private-field issue —
   only the `pub(super)`→`pub(crate)` call-reach change in point 1.

(If clippy flags `doc_markdown` on a moved doc-comment in a new child
file, add `#![allow(clippy::doc_markdown)]` at its top, matching the
existing suppression — do not edit the doc text.)

### matrix-rust-sdk analog

N/A — file organisation only. The receive pipeline's behaviour and the
public `SyncService` surface are unchanged; only internal module
boundaries move.

### Public API

**None.** Every handler is `pub(super)`/`pub(crate)`-internal (called
only by `service.rs`'s router); no `pub` consumer symbol changes. No
`docs/library.md` / `docs/bindings.md` / FFI change. The only doc touch
is the `v1-code-smells-audit.md` §A.1 row (same commit as 32e).

### Wire Message Types / Wire API v14

None. The decoded `GenericMessage` variants and the `GET /v14/events`
drain path are untouched; only the Rust handlers move.

### TDD / test strategy

Not a feature step — **no new failing tests**. The receive paths are
covered by the existing integration tests in `kupferdraht-core/tests/` (the
6-II receive tests, `step_11g_*`, `step_26_*`, etc.) which drive the
sync pipeline end-to-end and are unaffected by the module reshuffle. The
one inline `#[cfg(test)]` block (`parse_reaction_emoji_csv`) moves with
`reaction.rs`. Validation gate after each sub-step:

1. `cargo fmt --all -- --check`
2. `cargo build --workspace` (0 warnings)
3. `cargo clippy --workspace --all-targets -- -D warnings`
4. `cargo test --workspace --no-fail-fast`
5. `cargo deny check`

Run the smoke baseline (`scripts/smoke-baseline.sh`) once after 32e
merge; no new Wire behaviour to validate.

### Sub-step execution notes

- **32a is a pure `git mv` + folder creation** and must stay green: the
  module's content is byte-identical, only its path changes
  (`dispatch.rs` → `dispatch/mod.rs`); `mod dispatch;` in `sync/mod.rs`
  resolves to either form.
- **32b–32e cut handler sections.** One sub-step per branch
  (`step/32b-text`, …) or batch where each merge stays reviewable.
  Move handler bodies + their helpers + (for reaction) the test block
  **verbatim** — **do not** refactor logic while moving (no "while we're
  here" cleanups; splitting a function already over the 40-line cap is
  out of scope unless the move forces it). Apply the `pub(crate)`
  visibility bump as each handler lands in its child file.
- After each sub-step: full test gate green.
- **32e** finalizes `dispatch/mod.rs` down to the shared helpers + `mod`
  declarations, then verifies per-file code-line counts (CLAUDE.md awk
  rule): every new file should be well under 200 (the split divides ~703
  across ~9 interaction files + a small `mod.rs`). Mark the audit row
  resolved in the same commit.

### Audit closures

- `docs/v1-code-smells-audit.md` §A.1 — `sync/dispatch.rs` row: mark
  resolved when 32e merges.

### Out of scope — explicitly _not_ in Step 32

- **`sync/service.rs` decomposition** (the drain loop + router) — a
  separate concern; the router stays put.
- **`kupferdraht-cli/src/main.rs` split** — audit §A.1 sibling, its own step.
- **Pairing send + receive into one module per interaction** — rejected
  (different control-flow entry points; ADR-0009 layering keeps the
  API-driven send side in `timeline/` and the event-driven receive side
  in `sync/`).
- **Behaviour changes** to any receive path.
