# Step 48 — Browser HTTP (`fetch` backend) + wall-clock shim — W5 / W8

> **Status:** MERGED to `main` — `1cd2e43`, PR #44 (merge `b8dd044`,
> 2026-07-07). Scope: **W8** wall-clock shim (`time.rs`, browser
> `js_sys::Date::now()`) + **W5** reqwest `fetch` backend. Browser-verified
> per step notes ("browser-verified with a live `fetch` round-trip" — a
> `wasm32-unknown-unknown` test performs a real HTTP round-trip against the
> `wasm-bindgen-test` server's own origin in headless Firefox), no dated
> smoke record.
>
> **Branch / commit prefix:** committed on `wasm`; `step/48-*`.

## W8 — wall-clock shim (`time.rs`)

`SystemTime::now()` **panics** on `wasm32-unknown-unknown` (no platform clock),
so `now_ms()` is cfg-split: native keeps `SystemTime`, wasm reads the browser
wall clock via `js_sys::Date::now()` (an `f64` of ms since the epoch) clamped
into `i64` range. This was the last live runtime landmine from the Step 44
gating — every `now_ms()` caller (timeline timestamps, receipts, token expiry)
would otherwise panic on web. `getrandom/js` (the other half of W8) was already
enabled in Step 44.

## W5 — reqwest `fetch` backend

The **compile side was already done in Step 44** and is confirmed here:

- `reqwest` base features are wasm-safe (`default-features = false`, `json`
  only) — no `rustls-tls`/`ring` (the wasm32 blocker). The native-only
  dependency block adds `rustls-tls` + `cookies` + `stream`; on wasm reqwest
  falls back to the **browser `fetch`** backend (the browser does TLS).
- `HttpClient::new` gates the `connect_timeout`/`timeout` knobs native-only
  (fetch has none); the `TokenRefresh` future is `Send` on native and
  `!Send` on wasm (the fetch future is `!Send`); the streaming/multipart asset
  paths are cfg-gated off on wasm (whole-buffer bodies are W7).

So no code changes were needed for W5 itself — the gap was **runtime proof**.

## Verification

- `cargo test -p kupferdraht-core --target wasm32-unknown-unknown --test wasm_fetch`
  (headless Firefox): `reqwest::get(<harness origin>)` performs a real
  HTTP round-trip to the `wasm-bindgen-test` server's own origin (localhost —
  no external network, no CORS) and returns a status → the `fetch` backend
  links and runs on wasm. A transport failure would surface as `Err`.
- Native (all-targets) clippy + `time::` unit tests + wasm (`--lib`) clippy green.

## Out of scope / follow-ups

- **W6 — the `zuid` refresh cookie (🚩 the real risk).** `fetch` uses the
  browser cookie jar and JS/wasm **cannot read `Set-Cookie`**, so Kupferdraht's
  rotate-then-persist `zuid` handling (Steps 39/40/41) can't run as-is on web.
  Needs a prototype: `fetch credentials:'include'` (browser jar) vs. Wire
  returning the rotated `zuid` in the body. This is the natural next step — a
  real login exercises `fetch` + the cookie behaviour together.
- **W4 — async runtime.** The live sync/typing loops still use tokio
  `spawn`/`sleep`/`Notify`; wasm needs `spawn_local` + timers.
- **W7 — assets.** Streamed/multipart asset bodies are gated off on wasm;
  needs whole-buffer `Blob`/`Uint8Array` (+ OPFS for large payloads).
