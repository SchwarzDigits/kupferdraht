# Step 18 — hygiene (CLI naming + shipped-migration immutability)

> **Status:** merged — 18a + 18b merged. 18a dropped `run_*` prefix from 43 CLI handlers (live-validated 2026-06-05, smoke baseline); 18b `tests/migration_immutability.rs` pins each migration's SHA-256 (closes `CHANGELOG.md` §50). Archived from PLAN.md.
>
> **Sub-steps:**
> - 18a — drop the `run_*` prefix from every CLI handler in
>   `kupferdraht-cli/src/main.rs`; update the dispatch `match` arms.
> - 18b — shipped-migration immutability guard: a workspace test that
>   pins the SHA-256 of every `storage/migrations/VNN__*.sql` so an
>   edit to an already-released migration fails the test gate
>   (`CHANGELOG.md` §50).
>
> **Branch / commit prefix:** `step/18*-*`, `chore(step-18*): …`.

---

## Step 18 — CLI handler naming hygiene

The Kupferdraht CLI consistently prefixes its subcommand handler functions
with `run_*`:

```rust
async fn run_rejoin_all() -> Result<(), KupferdrahtError> { ... }
async fn run_room_join(room_id: String) -> Result<(), KupferdrahtError> { ... }
async fn run_send(room_id: String, body: String) -> Result<(), KupferdrahtError> { ... }
```

This is a tutorial-era clap idiom. It violates Kupferdraht's own naming
rule recorded in [`CLAUDE.md`](../../CLAUDE.md) §Conventions → §Design:

> *"Function names must describe their effect: Network I/O:
> `upload_key_package`, `send_message`, `fetch_rooms`. Pure
> computation: `encrypt_message`, `build_mls_group`, `derive_key`.
> **No generic names: `process`, `handle`, `do_work`.**"*

`run_` is generic — it says "this is a CLI handler", not what the
function does. The two practical roles it plays inside the bin
crate are weak:

1. **Marker** — "this is a clap-subcommand entry point". Rust's
   module system already separates bin-crate functions from
   library imports; no marker needed.
2. **Disambiguation** — avoid collision with a library symbol of
   the same name. Since the bin crate cannot re-import a
   library function with the same exact path, the collision risk
   is theoretical, not real.

Step 18 removes the prefix from every CLI handler. ~20 functions,
plus the corresponding `match` arms in `main`. Mechanical,
single-commit refactor.

### matrix-rust-sdk analog

N/A — this is an internal Kupferdraht CLI convention question, not an
API-shape question against matrix-rust-sdk. Kupferdraht's library
layer (`kupferdraht-core`) already follows the effect-describing
convention.

### Sub-steps

| Sub-step | Scope | Wire API v14 | Value |
|---|---|---|---|
| 18a | Drop `run_` prefix from every async handler function in `kupferdraht-cli/src/main.rs` (`run_rejoin_all` → `rejoin_all`, `run_send` → `send`, `run_room_join` → `room_join`, …). Update the dispatch `match` arms in `main()` to call the renamed functions. Helpers `resume_with_room` / `resume_with_room_for_message` stay as they are (they already describe their effect). | none (pure Rust rename) | low |

### Public API

None. The CLI's outward-facing surface — subcommand names like
`rejoin-all`, `room-join`, `send` — stays identical. Only the
internal Rust function names change.

### Wire Message Types

None.

### Implementation notes

**18a — pure rename, no behavioural change.** The work is:

1. For every `async fn run_*` in `main.rs`, drop the `run_` prefix.
2. Update each call site (the `match command { ... }` block in
   `main()` is the only caller — handlers don't call each other).
3. Run the full pre-commit gate (`cargo fmt --check`, `cargo
   build` 0 warnings, `cargo clippy -D warnings`, `cargo test
   --workspace`, `cargo deny check`).

No tests need to change — the integration tests live in
`kupferdraht-core/tests/*.rs` and test the library, not CLI handlers.

**18a — what stays.** Names that already describe their effect or
are already prefix-free do not change:

- `resume_client`, `resume_client_and_store`, `resume_with_room`,
  `resume_with_room_for_message` — `resume_` is the actual effect.
- `print_dm_result`, `print_room_after_mutation` — `print_` is
  the actual effect.
- `mount_*` (test helpers) — `mount_` is the actual effect (HTTP
  mock mounting). Not in `main.rs` anyway.

The audit boundary is: a `_` prefix that names an *action*
(`resume`, `print`, `mount`, `fetch`, `parse`) survives. A
`_` prefix that names a *role* (`run`, `handle`, `process`, `do`)
is the smell Step 18 targets.

### Audit closures

None directly. Step 18 is style polish driven by a Conversation
2026-05-21 (recorded in this plan doc), not by an entry in
`v1-api-ergonomics-audit.md` or `v1-known-simplifications.md`.

### Out of scope — explicitly _not_ in Step 18

- **`v1-api-ergonomics-audit.md §1.3`** — the audit of which
  CLI handlers actually need a `sync_once` pre-call vs.
  `load_rooms_from_store`. Different concern (network
  efficiency, not naming); separate Step if/when it gets one.
- **Renaming clap subcommand verbs** (`rejoin-all`, `room-join`,
  etc.). These are the CLI's public surface and changing them
  breaks scripts and muscle-memory. Out of scope.
- **Renaming the `Command::*` enum variants** in `main.rs`. clap
  derives the subcommand verb names from these via
  `kebab-case`; renaming the variants would either break the
  subcommand names or require `#[clap(name = "…")]` overrides.
  Out of scope.
- **CLI subgroup restructure** (`kupferdraht-cli dm <create|show|with>`
  vs flat `kupferdraht-cli dm-create`). Audit §7.3 territory.
- **Library-side renames.** `kupferdraht-core`'s functions already
  follow the effect-describing rule.

### TDD starter tests

Step 18 is a pure rename — there is nothing to test that the
existing `cargo build --tests` and `cargo test --workspace`
do not already cover. The success condition is:

- `cargo build --workspace 2>&1 | grep -c "warning"` → `0`
- `cargo clippy --workspace --all-targets -- -D warnings` → 0 diagnostics
- `cargo test --workspace --no-fail-fast` → all green
- `grep -n "fn run_" kupferdraht-cli/src/main.rs` → 0 hits

### CLI commands

No new CLI commands. Subcommand names unchanged. Only internal
Rust symbol names change.

### Smoke test

Kupferdraht's existing 5-step smoke-baseline pass against Wire
production is sufficient verification that the rename did not
break dispatch:

```bash
kupferdraht-cli login --backend production --email "$EMAIL"
kupferdraht-cli sync
kupferdraht-cli rooms
kupferdraht-cli send --id "$ROOM_ID" -m "step-18 rename smoke"
kupferdraht-cli sync
```

Each subcommand exercises a different renamed handler. If any
arm of the `main()` dispatch points at the old name, the call
would fail at runtime (the build catches it earlier, but the
smoke confirms end-to-end).

### Open questions

None. The decision was clear in the 2026-05-21 conversation:
follow CLAUDE.md's effect-describing rule; drop the prefix;
keep §1.3 as a separate concern.

---

## Step 18b — shipped-migration immutability guard

`refinery` validates each already-applied migration by hashing the
**entire migration file — comments included** — and comparing it to
the checksum recorded in that database's `refinery_schema_history`.
A migration file is therefore append-only the moment it has been
applied anywhere: editing it (even a comment) changes the hash and
hard-fails every existing database at open with `applied migration …
is different than filesystem one …`.

This actually happened. Commit `f31ac5e` ("drop Kalium-source
citations from shipped artifacts") rewrote **comments only** in the
already-shipped `V12` and `V14` migrations; the SQL was byte-
identical. The checksum changed anyway, and the next `login` against
a pre-existing `kupferdraht.db` hard-failed. Surfaced during the Step-18a
live smoke — full write-up in [`CHANGELOG.md`](../../CHANGELOG.md) §50.

The §50 decision kept the comment rewrites (the Kalium-citation
removal, ADR-0009, outweighs migration immutability pre-v1; only the
developer's own device was affected) and deferred the *class* fix to
18b: a guard so a doc-comment sweep can never silently mutate a
shipped migration again.

### Implementation

A single workspace integration test —
`kupferdraht-core/tests/migration_immutability.rs` — runs inside the
existing `cargo test --workspace` gate (no new infra, no new
dependency: `sha2` is already a `kupferdraht-core` dependency):

- `PINNED_DIGESTS` pins the SHA-256 of every
  `src/storage/migrations/VNN__*.sql` at its shipped content.
- `shipped_migrations_match_pinned_digests` rehashes each file and
  asserts it equals its pin — any edit to a released migration fails
  here with a "shipped migrations are immutable: revert the edit"
  message.
- `every_migration_file_is_pinned` asserts the on-disk file set and
  the pin table match exactly — adding a migration forces a
  deliberate new pin entry; a removed/renamed file is caught.

SHA-256 of file content (not refinery's internal `u64` checksum) is
pinned deliberately: it catches every content change and stays valid
regardless of refinery's hashing internals.

### Guard discipline

- Adding a migration → append one `PINNED_DIGESTS` entry for the new
  (highest) version. This is the only legitimate edit to the table.
- A failing `shipped_migrations_match_pinned_digests` on an existing
  version means a shipped migration was mutated. The fix is to
  **revert the file**, not to re-pin — the old digest is already
  recorded in every existing database.

### Public API / Wire Message Types

None. Test-only addition.

### Open questions

None.
