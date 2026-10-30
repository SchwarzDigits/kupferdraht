# Step 33 — Decompose `kupferdraht-cli/src/main.rs`

> **Status:** MERGED to `main` — commits `7e0e6bf`..`51678f5` (33a-e),
> PR #42 (merge `f226898`, 2026-07-05). main.rs 2343 -> 930 raw lines
> (clap types + dispatch only); nine cli/ domain files, 50-324 lines.
> Live smoke 2026-07-05: full baseline OK against Wire production
> (status, sync, rooms, rooms-join-all 18/18 idempotent, send
> round-trip). Executed as five commits 33a-33e. Deviations from the mapping below,
> driven by post-plan code drift (Steps 34-43): persist_session_state +
> kupferdraht_db_exists live in cli/auth.rs (their only callers);
> download_asset joined cli/assets.rs; sync_stream + CliEventHandler in
> cli/sync.rs; backend.rs (Step 43) stays a main.rs sibling (arg-level,
> not a handler domain).
>
> **Sub-steps:**
> - 33a — scaffold: add `mod cli;` to `main.rs`; create `cli/mod.rs`
>   holding the **shared plumbing** moved out of `main.rs`
>   (`resume_client`, `resume_client_and_store`, `resume_with_room`,
>   `resume_with_room_for_message`, `kupferdraht_home`, `read_password`,
>   `base64_encode`, `cli_keystore`, `persist_session_state`, `parse_csv`,
>   `prefix`), made `pub(crate)`; declare the child modules. The clap
>   `Command` enum + `main()` stay in `main.rs`; the per-command handlers
>   stay temporarily and now call `cli::…` for the shared helpers. Pure,
>   must stay green.
> - 33b — `cli/auth.rs`: login / register / activate / logout /
>   change_password / accounts / status (+ `recover_device_limit`).
> - 33c — `cli/rooms.rs` (rooms / join / members / create / invite / kick /
>   rename / leave / delete / DM) and `cli/access.rs` (room-access +
>   join-link CRUD + join-via-link).
> - 33d — `cli/timeline.rs` (send / reply / edit / delete / react /
>   mark-read / timeline / knock / location + `resolve_room_for_message`)
>   and `cli/assets.rs` (send-file / download-asset).
> - 33e — `cli/sync.rs` (sync / sync-metadata / sync-stream),
>   `cli/devices.rs` (devices / delete / rename), `cli/config.rs`
>   (profile-set / features / properties); reduce `main.rs` to the
>   `Command` enum + `main()`; mark the audit row resolved.
>
> **Historical names:**
> - "Step 33" — current.
>
> **Branch / commit prefix:** `step/33*-*`, `chore(step-33*): …`.

---

## Step 33 — Decompose `kupferdraht-cli/src/main.rs`

`kupferdraht-cli/src/main.rs` is ~1712 **code** lines (CLAUDE.md count rule) —
the single largest file in the workspace and the top row of
[`open.md`](../../open.md) Backlog → Code quality (cq-A.1). It is
the CLI switchboard: the clap-derived `Command` enum (~55 subcommands),
~50 per-command handler functions, the shared session/room plumbing they
all reuse, and `main()` (the dispatch `match`).

A switchboard was always going to be wide, but 8× the cap is structural
debt: a reviewer fixing one subcommand pages through fifty unrelated
ones. The natural axis is the **command domain** (auth, rooms, timeline,
assets, …) — the same axis `clap` already groups the subcommands by in
the user's `--help`.

This step is **pure structure**: move code, no behaviour change. Closes
the `kupferdraht-cli/src/main.rs` row in §A.1.

### Why now

- It is the largest single offender (1712), ~5× the next library file.
- The handlers are **free functions** (not trait/`impl` methods), so the
  move is mechanically simpler than the library splits (Steps 29/30/32) —
  no private-field / `impl`-block visibility subtlety, just a
  private→`pub(crate)` bump so `main()` can still call each handler.
- It completes the §A.1 "big files" cluster: after Steps 29/30/32 the
  library side is decomposed; `main.rs` is the CLI counterpart.

### Target layout

`main.rs` stays the binary crate root: it keeps the clap types and the
dispatcher, and gains `mod cli;`. Everything else moves into a `cli/`
module, grouped by command domain.

```
kupferdraht-cli/src/
  main.rs            # the clap `#[derive(Parser)]` Cli + Command enum +
                     #   the ServerArg/value enums + main() (the dispatch
                     #   match) + `mod cli;`. Nothing else.
  cli/
    mod.rs           # shared plumbing every handler reuses, pub(crate):
                     #   resume_client, resume_client_and_store,
                     #   resume_with_room, resume_with_room_for_message,
                     #   kupferdraht_home, read_password, base64_encode,
                     #   cli_keystore, persist_session_state, parse_csv,
                     #   prefix  + `mod` declarations
    auth.rs          # login, recover_device_limit, register, activate,
                     #   logout, change_password, accounts, status
    sync.rs          # sync, sync_metadata, sync_stream
    rooms.rs         # rooms, room_join, rooms_join_all, rejoin_all,
                     #   room_members, room_create, room_invite, room_kick,
                     #   room_rename, room_leave, room_delete,
                     #   dm_create, dm_show, dm_with, print_dm_result
    access.rs        # room_access_show/set/allow_guests,
                     #   join_link_generate/revoke/current, join_via_link
    timeline.rs      # send, reply, edit, delete_message, react, mark_read,
                     #   timeline, knock, location, resolve_room_for_message
    assets.rs        # send_file, download_asset
    devices.rs       # devices, device_delete, device_rename
    config.rs        # profile_set, features (+print_feature_config_detail),
                     #   properties (+print_user_property_detail,
                     #   brief_property_value)
```

`main()` dispatches exactly as today, only the call targets gain a
module path: `Command::Send { id, message } => cli::timeline::send(id,
message).await`, etc.

### Handler → file mapping (authoritative, from the current file)

| Destination | Handlers / helpers |
|---|---|
| `cli/mod.rs` | resume_client, resume_client_and_store, resume_with_room, resume_with_room_for_message, kupferdraht_home, read_password, base64_encode, cli_keystore, persist_session_state, parse_csv, prefix |
| `cli/auth.rs` | login, recover_device_limit, register, activate, logout, change_password, accounts, status |
| `cli/sync.rs` | sync, sync_metadata, sync_stream |
| `cli/rooms.rs` | rooms, room_join, rooms_join_all, rejoin_all, room_members, room_create, room_invite, room_kick, room_rename, room_leave, room_delete, dm_create, dm_show, dm_with, print_dm_result |
| `cli/access.rs` | room_access_show, room_access_set, room_access_allow_guests, join_link_generate, join_link_revoke, join_link_current, join_via_link |
| `cli/timeline.rs` | send, reply, edit, delete_message, react, mark_read, timeline, knock, location, resolve_room_for_message |
| `cli/assets.rs` | send_file, download_asset |
| `cli/devices.rs` | devices, device_delete, device_rename |
| `cli/config.rs` | profile_set, features, print_feature_config_detail, properties, print_user_property_detail, brief_property_value |

### Visibility — the only mechanical change

The handlers are private `async fn`s today; `main()` calls them by bare
name. Once they move under `cli::{domain}`, `main()` (in the crate root)
reaches them only if they are `pub(crate)`. So each relocated handler and
each shared helper in `cli/mod.rs` becomes `pub(crate)`. No other change —
the handlers are free functions taking already-parsed primitive args
(`room_id: String`, …), so they carry no `clap` or `impl`-privacy
coupling. The clap `Command` enum and its `#[derive(Parser)]` stay in
`main.rs`.

### File-size note

The split divides ~1712 across nine domain files. Two are heavy:
`cli/rooms.rs` (~15 handlers) and `cli/timeline.rs` (the `timeline`
render handler alone is ~120 lines) will likely land in the 250–350 LoC
range — still **single-domain** (one `responsibility` = "the room CLI" /
"the timeline CLI"), so a borderline cap overage by a cohesive file, not
a multi-responsibility violation. If either is judged too big, a finer
split is allowed in the same step: `cli/timeline.rs` +
`cli/timeline_render.rs` (the long `timeline` print), or `cli/rooms.rs` +
`cli/dm.rs`. Decide per file at 33c/33d; don't over-split pre-emptively.

### matrix-rust-sdk analog

N/A — this is the CLI (`kupferdraht-cli`), which has no matrix-rust-sdk
analog; the library public API is untouched.

### Public API / FFI

**None.** `kupferdraht-cli` is a binary that consumes `kupferdraht-core`'s public
API; no `pub` core symbol changes, no FFI change. The CLI's **observable
behaviour** (subcommands, flags, output) is **unchanged** — this is a
pure internal reorg — so `docs/cli.md` needs **no** update (the freshness
rule fires only on a CLI *behaviour* change).

### Wire Message Types / Wire API v14

None.

### TDD / test strategy

Not a feature step — **no new failing tests**. The CLI is validated by
`scripts/smoke-baseline.sh` (live, against Wire) and the integration
tests in `kupferdraht-core/tests/`; the handlers themselves have no inline
`#[cfg(test)]` blocks to migrate. Validation gate after each sub-step:

1. `cargo fmt --all -- --check`
2. `cargo build --workspace` (0 warnings)
3. `cargo clippy --workspace --all-targets -- -D warnings`
4. `cargo test --workspace --no-fail-fast`
5. `cargo deny check`

Run the smoke baseline once after 33e merge to confirm the binary still
drives a full session (no new Wire behaviour to validate).

### Sub-step execution notes

- **33a is the scaffold** and must stay green: move the shared helpers to
  `cli/mod.rs` (made `pub(crate)`), add `mod cli;`, and repoint the
  still-in-`main.rs` handlers' helper calls to `cli::…`. No handler moves
  yet.
- **33b–33e move handler groups** verbatim (private→`pub(crate)`), one
  domain per file. **Do not** refactor handler logic while moving (no
  "while we're here" cleanups; the §2 function-size items — `run_login`,
  `run_timeline` — are a *separate* concern, out of scope here). Update
  `main()`'s `match` arms to the new `cli::{domain}::{fn}` paths as each
  group lands.
- After each sub-step: full test gate green.
- **33e** reduces `main.rs` to the clap types + `main()` and verifies
  per-file code-line counts (CLAUDE.md awk rule). Mark the §A.1 row
  resolved in the same commit.

### Audit closures

- `docs/v1-code-smells-audit.md` §A.1 — `kupferdraht-cli/src/main.rs` row:
  mark resolved when 33e merges.

### Out of scope — explicitly _not_ in Step 33

- **Function-size fixes** (`run_login` ~72 lines, `run_timeline` ~92
  lines) — audit §2; their own chore, not bundled with the file split.
- **New subcommands / flag changes / output changes** — behaviour stays
  identical.
- **`clap` restructuring** (subcommand groups, `#[command(subcommand)]`
  nesting) — the enum stays flat as today.
- **`sync/dispatch.rs` / library splits** — Steps 32 / 29 / 30.
