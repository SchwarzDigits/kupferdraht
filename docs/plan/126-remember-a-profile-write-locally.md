# Step 126 — A profile write is remembered locally, not only sent

**Status: commissioned 2026-10-06** — proposed by an embedder of the browser
surface after Step 125. Scope: `set_handle` and `set_display_name` update the
persisted `self_user` row after the backend accepts the change. No surface
change, no ADR, no CLI command (the CLI's `profile-set` is the same call and
gains the behaviour for free). The step number is proposed, not claimed.

## Problem

`Client::set_handle` and `Client::set_display_name` are pure HTTP calls
(`client/profile.rs:43,54`): they send the change and keep nothing. Nothing
else refreshes the account either —

- `Account` is immutable (`client/account.rs:18-38`), held as
  `Arc<Account>` (`client/mod.rs:128`), so it cannot be updated in place;
- it is built at login from `GET /self` (`login_pipeline.rs:690`) and at
  `resume` from the persisted `SelfUserRecord` (`resume.rs:265`);
- `save_self_user` is called from exactly one place, after login
  (`manager/mod.rs:549`);
- and the sync path skips self deliberately — "Self's name / handle come from
  `/v14/self` (already in the account snapshot)" (`client/sync.rs:515`).

So a changed handle is invisible to the engine until the next **login**.
Measured end to end on `32388bc` against the test backend: after
`setHandle("dummy2")` the backend had `dummy2` while `client.handle` answered
`dummy`, through a reload, until a password login.

A consumer of this surface had to remember the value itself — and lost it at
the next reload. The engine already stores the row `resume` rebuilds the
account from; a write the backend accepted belongs in it.

## Scope

One helper and two call sites in `client/profile.rs`:

1. After `update_self_handle` succeeds, write the `self_user` row with the new
   handle; the other fields come from the row as stored (an earlier write of
   the session is there, not in the account snapshot), from the account
   snapshot only when there is no row yet.
2. The same for `update_self_profile_name` with the new name.
3. No store attached (a metadata-only session) → nothing to write, `Ok(())`.

`save_self_user` is already an upsert keyed on `id`
(`storage/sqlite/self_user.rs:22`), so writing the whole row is correct, and
`Client` already reaches the store through `store_handle()` as
`device_watch.rs:98` does.

**Not in scope:** the live `Arc<Account>` of the running session. Replacing it
touches a field every part of `Client` reads, and a consumer can cover its own
writes for the session's lifetime. This step makes the **next** `resume`
correct, which is the measured symptom. Re-reading self on `resume` — which
would also catch a change made on another device — is a separate decision
about a network call at startup.

## TDD (phase 3)

`kupferdraht-core/tests/step_126_profile_write_is_remembered.rs`, on the existing
login harness (`common::mount_full_login_flow` mocks `GET /self` with
`handle: "alice"`, `name: "Alice Example"`):

- `set_handle("alice2")` → the persisted `self_user.handle` is `alice2`, and
  name and email are untouched — red today, the row keeps `alice`;
- `set_display_name("Alice Two")` → the persisted `name` is `Alice Two`, the
  handle untouched — red today;
- a session without a store answers `Ok(())` rather than failing;
- review follow-up: `set_handle` then `set_display_name` in one session —
  both values survive in the row (red against the first version, which
  rebuilt the untouched fields from the account snapshot and so put the
  login's handle back).

The second assertion of each is the one that matters beyond the fix: the row
is written whole, so a careless helper would clobber the fields it was not
asked to change.

## As built (2026-10-07)

`Client::remember_self_profile(handle, name)` in `client/profile.rs`, called
after the `?` of each of the two HTTP calls. It takes the store through
`store_handle()`, loads the row (`load_self_user`), changes only the fields
it was asked to, and writes it whole — `save_self_user` upserts on `id`. The
untouched fields come from the stored row, not the account snapshot (which
keeps the login's values, so an earlier write of the session would be
reverted); the snapshot fills a missing row only.
No store (a metadata-only session) returns `Ok(())`. The live `Arc<Account>`
is untouched by design.

The module doc said "each is a thin transport call living on `Client`", which
two of them no longer are, so it says what they are now.

## Gate + smoke (2026-10-07)

Gate green on the pinned 1.98: fmt, 0 build warnings, `clippy --workspace
--all-targets -D warnings`, **1288** workspace tests (1286 before this step),
`cargo deny check`, plus the wasm32 check and `clippy --lib` that the five
never reach.

Smoked through an embedder's showcase app with its engine rebuilt from this
branch, against a shared test account on the dev stage. Both directions, so
the reading discriminates — a run where the old and new handle are the same
string proves nothing, which is how the first attempt was wasted:

| | before (a `resume`) | after Save, same session | after a reload |
|---|---|---|---|
| `dummy` → `dummy2` | `@dummy` | `@dummy2` | **`@dummy2`** |
| `dummy2` → `dummy` | `@dummy2` | `@dummy` | **`@dummy`** |

The last column is the step. On `32388bc` it read the login's value in both
rows, measured three times yesterday. The middle column is the embedder's own
overlay over its session, which can go once this is released.

Two traps for whoever writes the next browser measurement here, both the same
mistake: the app's profile field is **seeded from the session info after the
element exists**, so typing as soon as the input is in the DOM gets clobbered
back to the old value and the app then dutifully sets the handle it already
had. And closing the settings overlay remounts the view behind it, so reading
the footer straight after the click reports no handle at all. Wait for the
value, never for the element.
