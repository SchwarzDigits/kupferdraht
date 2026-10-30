# Step 125 — The browser `Client` answers the account's handle

**Status: commissioned 2026-10-06** — proposed by an embedder of the browser
surface, which carries a REST call of its own for one string this surface
already holds.
Scope: one getter on the wasm `Client`, its twin in the tab mirror
(ADR-0026), the two doc homes. No new ADR — no decision is surfaced. **No CLI
command**: the surface is browser-only, as in Step 121; today's CLI has the
write side (`profile-set --handle`) and no read side for any profile field.
The step number is proposed, not claimed — renumber in review if 125 is taken.

## Problem

An embedder whose session type carries `handle` beside `displayName` and
`email` has to answer all three. The wasm `Client` has `setHandle` (Step 2g)
and no reader. Step 122 closed the same hole for `email` and left this one
open.

The cost is not cosmetic. The embedder keeps a REST module alive for this
single field: it mints a short-lived access token from the browser's `zuid`
cookie and calls `GET /self` to read one string the engine holds in its own
account row. That module was meant to disappear when `email` landed; the
handle is why it is still there.

The engine already has it. `Account::handle` is persisted with the self user
at login, and `Kupferdraht::accounts()` lists it today (`manager/mod.rs:403,551`).
Nothing needs fetching, storing or syncing — only giving back.

## Scope

One getter, three mirror lines, two doc homes:

1. `wasm::manager::Client::handle` — `#[wasm_bindgen(getter)]`,
   `Option<String>`, reading `self.inner.account().handle()`. The shape of
   `Client::email` from Step 122, which is also the precedent for
   "`undefined` when the account has none on record".
2. The tab mirror (Step 94 / ADR-0026): a `GETTERS` row in `protocol.js`, the
   getter in `proxy.js`, `readonly handle: string | undefined` in
   `index.d.ts`. A getter is the snapshot the leader took when it handed the
   wrapper out (`proxy.js` says so), which is right here: a handle changes
   only through `setHandle` on the same instance.
3. `docs/bindings.md`'s `Client` getter list, and
   `docs/api-mapping-matrix-rust-sdk.md`.

**Not in scope**, both their own decisions: making that snapshot live after a
`setHandle` in another tab, and a CLI read command for profile fields.

## TDD (phase 3)

`kupferdraht-core/tests/step_125_client_handle.rs` — source and doc pins in the
idiom of `step_94_tab_proxy_parity.rs`:

- the wasm surface exports the getter — red;
- the tab mirror carries it in all three files — red;
- both doc homes name the step — red.

`step_94_tab_proxy_parity.rs` then pins the pair independently and from both
sides: a `GETTERS` row whose class has no such getter fails as
"protocol.js GETTERS names non-existent", and the row drives
`proxy.js` / `index.d.ts` through `proxy_and_types_define_every_member`. So
the mirror cannot drift either way once this lands — which is why this file
pins the getter's existence and leaves the class association to that test.

**Not testable in CI: the value itself.** The getter reads an account row
only a real login fills, and the wasm target has no CI execution (PLAN.md).
That is a **smoke** item, as `email` was in Step 122: resume on the test
stage and read `handle`. It costs no device and no PIN attempt.

## As built (2026-10-06)

- `wasm::manager::Client::handle` — `self.inner.account().handle()`, the
  shape of `Client::email`. No core change: the row was already persisted
  and already listed by `accounts()`.
- The mirror: a `GETTERS` row, `get handle()`, `readonly handle: string |
  undefined`.
- **One thing the plan did not foresee**, and it broke every tab-proxied
  session: `HandleProxy` stored a proxy's leader-assigned routing id as
  `this.handle`. The new getter shadowed that field with a getter-only
  accessor, so the constructor's own `this.handle = descriptor.$handle`
  threw `TypeError: setting getter-only property "handle"` and every
  `restoreLastSession` failed. The routing id is now **`$handle`** — the
  name the wire format already used (`descriptor.$handle`,
  `fresh.$handle`) — at its three sites in `proxy.js`. It is undeclared in
  `index.d.ts` and read nowhere outside that file, so the rename is
  internal; `leader.js`'s `live.handle.stop()` is a wasm `SyncHandle`, not
  a proxy, and stays.
- `step_94_tab_proxy_parity.rs` stayed green through the whole bug: it
  pins that both sides *have* the member. So this step adds
  `no_proxy_field_collides_with_a_mirrored_getter`, which walks the
  `GETTERS` table and asserts no proxy assigns `this.<getter> =`. It is
  red on the old code with the error that explains itself.

## Gate + smoke (2026-10-06)

Gate green on the pinned 1.98: fmt, 0 build warnings, `clippy --workspace
--all-targets -D warnings`, `cargo test --workspace --no-fail-fast`, `cargo
deny check`. **Four of those five never compile this code** — `pub mod wasm`
is `cfg(target_arch = "wasm32")` — so CI's wasm job was run too:
`cargo check -p kupferdraht-core --target wasm32-unknown-unknown` and
`cargo clippy … --lib -- -D warnings`, both clean.

Smoked through an embedder's showcase app with its engine and its vendored
`kupferdraht-tabs` rebuilt from this branch, against a shared test account on the
dev stage (resume, so no device and no PIN attempt):

| check | result |
|---|---|
| the footer's `@<handle>`, fed **only** by the new getter | `@dummy` |
| the getter beside the `GET /self` it replaces, same session | `{"fromEngine":"dummy","fromRest":"dummy"}` |
| JS errors / app warnings | none / none |
| before the `proxy.js` fix | `restoreLastSession failed TypeError: setting getter-only property "handle"` — the login screen, no session at all |

The last row is why the smoke was worth its minutes: the gate, the parity
test and the generated `kupferdraht.d.ts` were all green while the module was
broken.
