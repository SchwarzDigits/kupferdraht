# Step 19 — Core `UserId` type (bare/qualified unification)

> **Status:** merged (2026-06-10) — `feature/user-id-type`. Validated string
> newtype (`localpart@domain`) closing the bare/qualified seam + nullable-domain
> fragility + `peek_session_user_id`. Sub-step commits:
> 19a `17ceb54` · 19b `b7f3d69` (storage unified, migration V21) · 19c `dbd7a4a` · 19d `3a3a4dd`. Archived from PLAN.md.
> **Sub-steps (commits on one branch):** 19a (`UserId` type), 19b (owner identity
> end-to-end — in-memory + storage + migration V21), 19c (remove
> `peek_session_user_id`), 19d (comparison cleanup + boundary mapping).
> **Branch / commit prefix:** **one branch** `feature/user-id-type`; sub-steps land as
> commits (`feat(step-19): …`), TDD per commit. (Deviates from CLAUDE.md's
> one-branch-per-sub-step on purpose — 19a–19d are tightly coupled around the identity
> type.)
> **Background:** `docs/finding-qualified-user-id.md` (analysis); supersedes the
> deferred "typed identifiers" item from Step 15 *for user ids only* (see below).

---

## Context

Kupferdraht has **no core-owned user-id type**. User ids travel as raw `String`s in two
formats: **bare** `<uuid>` and **qualified** `<uuid>@<domain>`. The format depends on
the call site, not the type — which is the root of three symptoms:

- **The bare/qualified column seam.** Owner rows are keyed bare (`session.user_id`,
  `self_user.id`, `devices.user_id`); foreign-user rows are qualified
  (`room_members.user_id`, `timeline_events.sender_user_id`, reactions/receipts/
  mentions, `rooms.counterpart_user_id`). So `sender_user_id == self_user.id` is
  *always false* — every "is this me?" check must first reconcile formats. Nothing in
  the type system enforces that.
- **The nullable-`self_user.domain` fragility.** The qualified owner id is reconstructed
  at resume from `session.user_id` (bare) + `self_user.domain` (nullable column, added
  retroactively in V5). If the column is `NULL`, resume errors out
  (`client/resume.rs` domain check).
- **`peek_session_user_id`.** A `Store` trait method that exists only to read the bare
  owner id back out of the `session` row, because the qualified `account_id` the caller
  already holds was never stored in a directly-usable owner-row form.

**Goal:** one canonical, validated `UserId` value type (the matrix-rust-sdk `OwnedUserId`
equivalent) carrying `id` + `domain`. The format becomes uniform by construction:
"is this me?" is `a == b`, the domain is non-optional, the seam disappears.

## Relation to Step 15 / `CHANGELOG.md` §49 (why a typed user id *now*)

Step 15 considered, then **rejected**, "soft typed identifier wrappers" (`kupferdraht::ids::*`)
— newtypes around `String` with **no** parse validation and **no** semantics, whose only
job was compile-time anti-mixing (don't pass a `UserId` where a `RoomId` goes). §49
rejected that as cosmetic churn and shipped a `&str` ergonomics micropass instead.

This step is **not** that. The distinction is deliberate:

| | Step-15 wrapper (rejected) | Step-19 `UserId` (this) |
|---|---|---|
| Scope | all id types (user/room/device/message) | **user ids only** |
| Validation | none (opaque string) | parses + validates `id@domain` |
| Carries domain | no | **yes** (fixes the seam) |
| Justification | cosmetic type-mixing safety | a real correctness seam + nullable-domain fragility |

Room/device/message ids have no domain and no two-format problem — they **stay `&str`**.
`UserId` earns a type because it fixes a concrete correctness defect, not for ergonomics.

## Current state (from the Step-19 research sweep)

- **Already qualified, no change needed:** `wire::dto::QualifiedId`; class-B storage
  columns; timeline items; rooms; the MLS leaf identity; the store directory + keystore id
  + `Client::account_id`.
- **Bare holdouts = the seam to close:** `Client.user_id`, `Account.{user_id,
  domain: Option}`, the owner rows (`session`/`self_user`/`devices`), `peek_session_user_id`.
- **No UniFFI yet** (`kupferdraht.udl` is empty) → `UserId` is a pure-Rust internal type now. The
  FFI representation is a deferred, isolated boundary choice: when the UDL is populated we can
  either expose a `#[derive(uniffi::Record)]` `{ localpart, domain }` to consumers, or flatten
  to `String` (matrix-rust-sdk-ffi convention) — the internal string-newtype does not lock
  either out (a small boundary DTO/converter bridges it).
- **Migrations are SHA-256-pinned** (`tests/migration_immutability.rs`, Step 18b) → schema
  changes require a **new** migration (V21); existing migrations are never edited.

## Decision (resolved): unify the owner storage

**Owner rows store the qualified id; the nullable `self_user.domain` column is dropped.**
After this, *every* user-id column (owner + foreign) is qualified — the seam is gone at
rest, matching `mls.db` / keystore / store-directory (already qualified). Pre-v1, no prod
data → no data migration; dev DBs are recreated on the next login. (Chosen over
"keep bare + make domain `NOT NULL`", which would leave two formats.)

## Sub-steps (commits on one branch; TDD each: red tests → approval → green)

### 19a — `UserId` core type
New module `kupferdraht-core/src/identity/`. A **validated string newtype**:
`pub struct UserId(String)` with the invariant "exactly one `@`, both sides non-empty",
holding the canonical `localpart@domain`. Constructors: `UserId::new(localpart, domain)`
(infallible — both parts required; used at the Wire `QualifiedId` boundary) and
`UserId::parse(&str)` (validates — used for flat strings from DB columns / CLI args).
Accessors `localpart()` (slice before `@`) / `domain()` (slice after `@` — **not** `Option`)
/ `as_str()` (the qualified string, free). `Display` ⇄ `localpart@domain`. `#[derive(Clone,
PartialEq, Eq, Hash, Debug)]`. **Not** in `wire::` (ADR-0009); takes over the in-core role of
`wire::dto::qualified_user_id()`. The domain is guaranteed by construction — there is no
domain-less `UserId`, which is what removes the nullable-domain fragility.
Tests: round-trip, reject malformed/empty/multi-`@`, `localpart()`/`domain()` slicing,
`Eq`/`Hash`.

### 19b — owner identity on `UserId`, end-to-end (in-memory + storage + V21)
*Bundled deliberately — the in-memory type and its storage representation change together
(splitting would need a throwaway adapter).*
- **In-memory:** `Account` holds a `UserId` (drop `user_id: String` + `domain: Option`),
  `Account::user_id()` → `&UserId`; remove `Account::qualified_user_id()` and its `wire::dto`
  call. `Client`: collapse `user_id` (bare) + `account_id` (qualified) into one `UserId`;
  `Client::user_id()` → `&UserId`; **remove the `Client::account_id()` getter** (callers use
  `user_id()` / `user_id().as_str()`). `login_pipeline.rs::finalize` builds the `UserId` from
  `self_user.qualified_id` via `UserId::new`.
- **Storage:** new `V21__…sql` storing the qualified id in `self_user.id` / `session.user_id`
  / `devices.user_id` and dropping nullable `self_user.domain`. `storage/records.rs`
  (Session/SelfUser/Device) carry `UserId`; sqlite read/write serialize `UserId` ⇄ string at
  the column boundary. No data migration (recreate-on-login). `global.db`'s `accounts.account_id`
  is untouched (separate store, already qualified — the registry key stays).

### 19c — remove `peek_session_user_id`
Resume derives the owner `UserId` from the explicit `account(id)` it already receives. Drop
the trait method (`storage/records.rs`), impl (`storage/sqlite/mod.rs`), test
(`storage/sqlite/session.rs`); the single-account guard becomes a `debug_assert` or is
dropped; the nullable-domain error path in `resume.rs` disappears.

### 19d — comparison cleanup + boundary mapping
Self-checks become `sender == client.user_id()` (`UserId == UserId`); remove the
`qualified_user_id_required()` / `parse_qualified` shims where the type now suffices
(timeline send/receive, rooms `sync_members` / `group_leaves`). Boundary mapping: `UserId`
⇄ `wire::dto::QualifiedId` lives **at the wire boundary** (core never imports `QualifiedId`);
`UserId` ⇄ canonical string at the SQLite boundary.

## Resolved decisions
- **D1 — storage:** unify — owner rows store qualified, drop nullable `self_user.domain`.
- **D2 — `UserId` shape:** validated **string newtype** (matrix-rust-sdk `OwnedUserId` style;
  free serialization / hashing / comparison — the common case). FFI `Record` exposure stays
  open and is decided when UniFFI is designed.
- **D2 — accessor naming:** `localpart()` + `domain()` (format-agnostic structural names; the
  localpart is a Wire UUID *today* but not type-guaranteed, so the name avoids `uuid()`).
- **D3 — `account_id()`:** remove the `Client::account_id()` getter (one identity = `user_id()`).
  The `global.db` `accounts.account_id` **column** stays (registry key, already qualified).
- **D-Placement:** new top-level module `identity/` (cross-cutting type; keeps the dependency
  direction clean — `storage` must not depend on `client`).
- **D4 — staging:** **one branch** `feature/user-id-type`; 19a–19d land as sequential TDD
  commits.

## Risk
Identity-critical path (login / resume / MLS leaf identity / self-comparison). A bug here
breaks messaging, not just a getter. Mitigation: strict TDD per sub-step, the full gate, and
a **live smoke** of the identity round-trip (login → resume → send → room-members).

## Verification
Per sub-step: `cargo fmt --all -- --check`; `cargo build` (0 warnings); `cargo clippy
--workspace --all-targets -- -D warnings`; `cargo test --workspace --no-fail-fast`;
`cargo deny check`. Grep counter-check: no bare/qualified `String` user-id seam remains in
the owner paths. Live smoke of the identity round-trip before marking 19 done.
