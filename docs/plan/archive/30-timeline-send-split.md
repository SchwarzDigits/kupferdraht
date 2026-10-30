# Step 30 — Decompose `timeline/send.rs` by interaction

> **Status:** MERGED to `main` — commits `6615ad6`..`b46af9d` (30a-e),
> PR #20 (merge `8a4f3a8`, 2026-06-16). Scope: pure structure —
> `timeline/send.rs` decomposed by interaction into `timeline/{mod,
> text,reaction_confirm_delete,knock_location_typing}.rs` residues; no
> public API / FFI / Wire change. No smoke (refactor).
>
> **Sub-steps:**
> - 30a — scaffold: move the `Timeline` + `MessageHandle` structs and
>   their core accessors (`new`, `client_handle`, `room_id`) into
>   `timeline/mod.rs`; declare the new child modules; `send.rs` becomes a
>   thin shim (or start moving) — pure, must stay green.
> - 30b — `timeline/text.rs`: the text family — `send_text` (facade +
>   `impl Client` orchestrator), `send_text_auto_join`,
>   `send_text_with_mentions`, `send_reply`, `edit` (facade) +
>   `edit_message` (orch), plus the `build_text_message` and
>   `to_proto_mentions` helpers.
> - 30c — discrete interactions: `timeline/reaction.rs` (`toggle_reaction`
>   facade + orch), `timeline/confirmation.rs` (`mark_as_read` facade +
>   `send_read_confirmation` + `send_delivered_confirmation` orch),
>   `timeline/delete.rs` (`redact` facade + `redact_message` orch).
> - 30d — the small ephemeral sends + typing facade:
>   `timeline/knock.rs` (`send_knock`), `timeline/location.rs`
>   (`send_location`), `timeline/typing.rs` (the `Timeline`
>   `send_typing_started`/`send_typing_stopped` facade — the `impl Client`
>   side is Step 29's `client/typing.rs`).
> - 30e — read-side lookups + listener wiring; delete `send.rs`:
>   `timeline/lookup.rs` (`canonical_message_id`, `lookup_timeline_event`,
>   `load_reactions_for_message`, `load_read_by_for_message`,
>   `load_delivered_to_for_message`); move `Timeline::items` into
>   `items.rs` and `Timeline::subscribe` + `RoomScopedListener` +
>   `on_update` into `subscription.rs`; delete `send.rs`; update
>   `mod.rs` `pub use`.
>
> **Historical names:**
> - "Step 30" — current.
>
> **Branch / commit prefix:** `step/30*-*`, `chore(step-30*): …`.

---

## Step 30 — Decompose `timeline/send.rs` by interaction

`kupferdraht-core/src/timeline/send.rs` is ~565 **code** lines (CLAUDE.md
count rule; the file has **no** `#[cfg(test)]` block — all of it is
production code). Despite the name "send", it is a grab-bag of unrelated
responsibilities: the `Timeline` + `MessageHandle` structs themselves,
the read accessors, typing, and the outbound path of **every** message
type / interaction (text, reply, edit, delete, reaction, read/delivered
confirmation, knock, location), plus read-side lookup helpers that
aren't sends at all.

"Send" is not a responsibility in a messenger — everything sends
*something*. The natural SRP axis is the **interaction / message type**,
which the codebase already uses elsewhere: assets live in
`asset_send.rs` / `asset_receive.rs`. Step 30 brings the rest of the
timeline up to that pattern.

This step is **pure structure**: move code, no behaviour change. Closes
[`open.md`](../../open.md) Backlog → Code quality (cq-A.1) row
for `timeline/send.rs`.

### Why now

- `send.rs` is the second-worst timeline file-size violation after the
  asset pair was already split out.
- The interaction axis is established (`asset_send.rs`), so this is
  consistency work, not a new pattern.
- No inline tests to migrate (tests are integration tests in `tests/`
  that drive the public API — unaffected by the module reshuffle).

### Sibling of Step 29 — no overlap

Step 29 decomposes `client/runtime.rs` (the `Client` body). Step 30
decomposes `timeline/send.rs` (the `Timeline` body). Different files,
different types — they can land in any order. The one shared touch is
typing: Step 29 moves the `impl Client` typing methods to
`client/typing.rs`; Step 30 moves the `impl Timeline` typing facade to
`timeline/typing.rs`. Each owns its own type's side; no conflict.

### Target layout

```
kupferdraht-core/src/timeline/
  mod.rs          # Timeline + MessageHandle structs, new/client_handle/room_id,
                  # mod declarations, pub use (Timeline, MessageHandle, …)
  text.rs         # send_text (facade+orch), send_text_auto_join,
                  #   send_text_with_mentions, send_reply, edit (+edit_message),
                  #   build_text_message, to_proto_mentions
  reaction.rs     # toggle_reaction (facade + orch)
  confirmation.rs # mark_as_read (facade) + send_read_confirmation + send_delivered_confirmation (orch)
  delete.rs       # redact (facade) + redact_message (orch)
  knock.rs        # send_knock (facade + orch)
  location.rs     # send_location (facade + orch)
  typing.rs       # Timeline send_typing_started / send_typing_stopped (facade)
  lookup.rs       # read-side lookups by message id: canonical_message_id,
                  #   lookup_timeline_event, load_reactions/read_by/delivered_to_for_message
  items.rs        # (existing) + Timeline::items() read-render facade
  subscription.rs # (existing) + Timeline::subscribe + RoomScopedListener + on_update
  asset_send.rs   # unchanged (the precedent for this layout)
  asset_receive.rs / pipeline.rs   # unchanged
  # send.rs deleted after 30e
```

Each new file groups one interaction's **facade** (`impl Timeline`) with
its **orchestrator** (`impl Client`) — exactly as `asset_send.rs` holds
both `Timeline::send_file` and the `Client` upload orchestrator. Rust
allows multiple `impl Timeline` / `impl Client` blocks across child
modules of `timeline/`; private fields stay reachable.

`knock.rs` + `location.rs` are tiny (one facade + one orchestrator
each). They MAY be combined into a single `timeline/ephemeral.rs` if the
reviewer prefers fewer files; the plan keeps them separate to match
"one message type, one file."

### matrix-rust-sdk analog

N/A — file organisation only. The public surface stays
`matrix_sdk::Timeline`-shaped; only internal module boundaries change.

### Public API

**None.** Every `pub` / `pub(crate)` symbol keeps its path
(`crate::timeline::{Timeline, MessageHandle}`, …). `timeline/mod.rs`
re-exports `Timeline` + `MessageHandle` exactly as today (currently
`pub use send::{MessageHandle, Timeline}` → becomes a `mod`/child
re-export). No `docs/library.md` / `docs/bindings.md` /
`api-mapping` change unless a doc names `timeline/send.rs` as a path
(grep-and-replace in the same commit). FFI parity unaffected — no `pub`
signature changes.

### Wire Message Types / Wire API v14

None. The `GenericMessage` builders (`wire/proto.rs`) and the
`POST /v14/mls/messages` path are untouched; only the Rust call sites
move.

### TDD / test strategy

Not a feature step — **no new failing tests**. `send.rs` has no inline
`#[cfg(test)]` block; the send paths are covered by integration tests in
`kupferdraht-core/tests/` (`step_4_send_text.rs`, the 6-II reaction/edit/
delete/receipt tests, `step_11f_send_file.rs`, `step_26_*`) which drive
the public API and are unaffected by the reorg. Validation gate:

1. `cargo fmt --all -- --check`
2. `cargo build --workspace` (0 warnings)
3. `cargo clippy --workspace --all-targets -- -D warnings`
4. `cargo test --workspace --no-fail-fast`
5. `cargo deny check`

Run the smoke baseline (`scripts/smoke-baseline.sh`) once after 30e
merge; no new Wire behaviour to validate.

### Sub-step execution notes

- **30a is mechanical and must stay green.** Move the two structs + the
  three core accessors + `mod` declarations into `mod.rs`. Either leave
  the method bodies in `send.rs` temporarily, or move them only as pure
  `impl`-block relocations (no logic edits).
- **30b–30e cut `impl` sections.** One sub-step per branch
  (`step/30b-text`, …), or batch where each merge stays reviewable
  (< ~400 LoC moved per PR). **Do not** refactor logic while moving (no
  "while we're here" cleanups). Splitting a function that already
  violates the 40-line cap is out of scope unless the move forces it.
- After each sub-step: full test gate green.
- **30e** also relocates the non-send residue: the read-model lookups go
  to `lookup.rs`; `Timeline::items()` joins the read model in `items.rs`;
  `Timeline::subscribe` + `RoomScopedListener` join the listener model in
  `subscription.rs`. Then delete `send.rs` and update `mod.rs`. Verify
  per-file code-line counts (CLAUDE.md awk rule): no new file should
  exceed 200 lines **and** host multiple responsibilities. `items.rs`
  (~213 LoC today) and `pipeline.rs` (~267) are already at/over the cap —
  do **not** dump the read helpers into them; that is why `lookup.rs` is
  a separate file.

### Audit closures

- `docs/v1-code-smells-audit.md` §A.1 — `timeline/send.rs` row: mark
  resolved when 30e merges.

### Out of scope — explicitly _not_ in Step 30

- **`sync/dispatch.rs` (inbound) split** — the receive side of these
  same interactions (audit §A.1 sibling). Pairing send+receive per
  interaction (as assets do) is attractive but is its own step.
- **`client/runtime.rs` decomposition** — Step 29.
- **`kupferdraht-cli/src/main.rs` decomposition** — separate step.
- **Public API renames** or moving logic between `Timeline` and `Room`.
- **Behaviour changes** to any send path.
