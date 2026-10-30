# Step 121 — A login that meets a retired device forgets its keys

**Status: MERGED 2026-10-05** — commissioned 2026-10-05, found by the
natrium-web SDK while adopting Step 120. `6be8d83`, review follow-ups
`4b05479` (a failed forgetting keeps `DeviceRetired`; no assertion on
keystore state). Merged to `main` as PR #111 (`a43e0c5`); CI green.
Smoked through the natrium-web showcase. Scope shipped: a registration
that meets `DeviceRetired` forgets the account's keys in this browser, as
`resume` does, so the next login offers recovery; `rules/0040` widened to
both ways in. No CLI command (browser-only surface), no new ADR. Archived
with this header.

## Problem

ADR-0040 keeps one remote device per user and says a replaced browser
"learns it (`DeviceRetired`) and resets itself". It resets itself on one of
the two ways in. `Kupferdraht::resume` forgets the account's keys when the slot
refuses the open (`remote_resume.rs`); a **login** did not, and that leaves a
browser with no way out at all:

- its `resume` fails first with `SessionExpired`, because a new setup retires
  the replaced device's Wire client, so the key-forgetting on that path never
  runs;
- a password login then fails with `DeviceRetired` and **keeps the keys**, so
  every further login fails the same way;
- `PendingLogin::recover` refuses while a seed is present, so the current
  device's recovery data cannot be used either;
- and nothing an embedder can call drops those keys: `purge` with
  `purge_local` needs a `Client`, which such a browser can no longer get. The
  wasm surface has `accounts`, `login`, `resume`, `recover`, `open` — nothing
  that forgets.

The only remedy left is outside the application: clear the origin's site
data. Measured on 2026-10-05 by a tester of the natrium-web SDK, whose browser
answered "login failed" and could not be recovered from inside the app; the
SDK now reports `Failure.DeviceReplaced` and tells the user to clear site
data, which is a message about a dead end rather than a way out of it.

## Scope

One behaviour, two call sites, one rule sentence:

1. `PendingLogin::forget_retired_device` — forget the account's secrets for
   this home and flush the keystore's pending writes, so the forgetting is
   durable before it returns (Step 104 persists in the background; a seed that
   outlives it sends the next login into the same refusal). The registry entry
   needs no removal: `accounts()` lists only accounts that have a seed.
2. `PendingLogin::register_device_with` calls it when the open failed with
   `DeviceRetired`, then returns that error unchanged. The error is still the
   truth; what changes is the state it leaves behind.
3. `rules/0040` says "whichever way in met it", with the reason the keys must
   not be left.

Out of scope: `recover`'s own path (it refuses while a seed is present, which
is entry 3 of the natrium-web gaps document and a separate question), and
anything about the slot itself.

## Tests (TDD red first)

Native and real, in `manager::seed_login_tests` — `httpmock`, an in-memory
keystore, the fixture the cancel tests use:

- `forgetting_a_retired_device_leaves_no_keys_of_the_account` — after a setup
  has left a seed, the forgetting leaves no seed, no store key, no MLS key and
  no registry entry.
- `after_forgetting_a_retired_device_the_next_login_asks_for_the_keyfile` —
  the next login reports `keys` `Missing`, which is the point: recovery, not
  the same failure.

`DeviceRetired` itself comes from the browser's storage layer, so the wiring
cannot be executed natively. `tests/step_121_retired_device_pins.rs` holds it:
the registration path matches `Err(KupferdrahtError::DeviceRetired(_))` and calls
the forgetting, and the resume path still does — the two ways in stay
symmetric. The first version of that test looked for the *name*
`forget_retired_device` and passed against a `todo!()` stub; a guard a stub
satisfies guards nothing, so it asserts the match and the call.

## Smoke

Browser-only, so it is smoked through the natrium-web showcase: its engine
rebuilt from this branch, then the tester's exact path — a profile holding a
replaced device's keys logs in. Before: `DeviceRetired`, keys kept, repeatable
forever. After: the keyfile step.
