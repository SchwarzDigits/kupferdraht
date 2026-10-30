# Step 50 — Browser login: `zuid` cookie split (W6) + login binding (W10)

> **Status:** MERGED to `main` — `7425762`, PR #44 (merge `b8dd044`,
> 2026-07-07). Scope: **W6** `zuid` browser-cookie-jar split
> (`wire/http/auth.rs`) + **W10** login binding (`src/wasm/manager.rs`).
> Browser-verified per step notes ("browser-verified, incl. persistence" —
> a real Wire-dev login running end to end from the prototype, and
> "reload → resume succeeds with no re-login"), no dated smoke record.
> Note: the file's own Verification section separately lists "Pending: a
> live login round-trip against a Wire dev backend" — an internal
> inconsistency in the original prose, left as-is here (not resolved by
> this fold).
>
> Resume needed three wasm fixes (all `cfg`-split, native unchanged): the
> `refresh_stale_access_token` `debug_assert!(!refresh_token.is_empty())` is
> native-only (wasm's persisted token is the empty W6 sentinel — the assert
> panicked in debug builds → an unsettled promise / silent hang); `tokio::time`
> `::sleep` in the persist-retry became `crate::time::sleep` (gloo-timers — no
> tokio timer driver on wasm, W4); and `resume_mls_state` skips the file-based
> MLS reload on wasm (MLS is snapshot-persisted separately — W3 — and its
> restore into a resumed client is a follow-up). Added `console_error_panic_hook`
> so wasm panics surface in the console.
>
> **Branch / commit prefix:** committed on `wasm`; `step/50-*`.

## Goal

Make a real Wire login work from the browser, driven by the Vue/TS prototype —
the first end-to-end exercise of the wasm stack: fetch (W5), the `zuid` refresh
cookie (**W6**), and the persistent `IndexedDb` keystore (W2). Two parts.

## Part A — W6: `zuid` browser cookie jar (`wire/http/auth.rs`)

Wire authenticates `/access` with the `zuid` refresh cookie and rotates it via
`Set-Cookie`. Native reads/writes `zuid` itself (reqwest's own jar is off so it
forwards Wire's `Path=/access` cookie reliably — observations.md §18). In a
browser JS/wasm **cannot** read `Set-Cookie` or set the `Cookie` request header —
the browser owns them. So on wasm the **browser cookie jar** carries `zuid`:

- Two `cfg`-split helpers: `browser_credentials(rb)` adds
  `fetch_credentials_include()` on wasm (no-op native); `with_zuid_cookie(rb, t)`
  sets `Cookie: zuid=…` on native (no-op wasm). `login`/`rebind_access_token`/
  `logout` route their request builders through both.
- `extract_zuid` and the login `Set-Cookie` parsing are **native-only**; on wasm
  the persisted `refresh_token` is an empty sentinel (the jar holds the real
  value). The `!refresh_token.is_empty()` debug-asserts are native-gated.

Native behaviour is byte-for-byte unchanged (helpers are identity/no-op there);
the existing `rebind`/`extract_zuid` unit tests still pass.

## Part B — W10: the login binding (`src/wasm/manager.rs`)

The first real W10 wrapper objects, mirroring `ffi::manager` (an `Arc<Core>` per
stateful type, `async fn → Promise`, `KupferdrahtError → JsError`):

- `Kupferdraht.open(permit_http)` — pre-opens the `IndexedDbKeyStore` (async) and
  builds the manager; `permit_http` allows the `http://localhost` dev proxy.
- `Kupferdraht.login(apiBase, wsUrl, title, email, password, sfCode?, deviceLabel?)`
  → `PendingLogin`.
- `PendingLogin.registerDevice()` → `Client`; plus `userId`/`email` getters.
- `Client.userId`/`Client.displayName` getters. `Kupferdraht.resume`/`accounts` also
  exposed (`accounts` returns a JS array via `serde-wasm-bindgen`).

No `debug_*` test hooks — the wasm backends are verified **transitively** by the
real flow (login ⇒ W5+W6+W2). Sync (W1) and send (W3) are the next slices.

## The prototype (`kupferdraht-wasm-prototyp`, a colleague's local checkout)

A login form drives `Kupferdraht.open → login → registerDevice`. The Vite dev proxy
makes Wire same-origin. The Kupferdraht client builds *absolute* Wire paths
(`/api-version`, `/v{n}/...`, `/register`, `/activate`), so the proxy matches
those root paths by regex (`^/(api-version|v[0-9]+/|register|activate)`) and
forwards them **as-is** — a `/api`-prefix + rewrite mangled `/api-version` →
`-version` (Wire 401). `cookieDomainRewrite: ''` + `cookiePathRewrite: '/'`
rewrite the `zuid` Set-Cookie so the browser stores + resends it against the dev
origin (Wire scopes it `Path=/access`, which wouldn't match otherwise).

The build toolchain: `npm run build:wasm` forces the rustup toolchain
(`$HOME/.cargo/bin`) — Homebrew's `rust` has no `wasm32-unknown-unknown` target.

## Verification

- Native (all-targets) clippy + `wire::http::auth` tests + full suite green.
- wasm (`--lib`) clippy green; `wasm-pack build --target web` emits the typed
  `Kupferdraht`/`PendingLogin`/`Client` API; the prototype `vue-tsc` typechecks +
  Vite-bundles against it.
- **Pending:** a live login round-trip against a Wire dev backend — proves the
  `zuid` cookie actually round-trips the jar (the real W6 proof) and the keystore
  persists across reload. Run: `WIRE_API=https://<wire-dev> npm run dev`.

## Out of scope / follow-ups

- **Sync (W1) + send/receive (W3)** as the next binding slices (`Client.sync_once`,
  `Room.timeline().send_text`, `EventHandler` as a JS callback).
- **Cross-origin production** (no proxy): needs Wire to set `SameSite=None; Secure`
  + CORS-with-credentials — the deployment-side half of W6, partly Wire-owned.
- `logout`/session-expiry surfacing on the browser session model.
