# Step 43 — Custom backend URL (`--api-url`)

> **Status:** MERGED to `main` — `80b27f6`, PR #40 (merge `6973024`,
> 2026-07-05). Scope: CLI-only — `--api-url`/`--ws-url` on `login` /
> `register` / `activate` as the alternative to the two hardcoded
> presets; no `kupferdraht-core` / FFI / Wire-layer change.
> Live smoke 2026-07-05: login via `--api-url` against a self-hosted
> Wire v14 instance → clean `invalid-credentials` from the target
> host; derived `nginz-ssl` WebSocket host verified reachable.
>
> **Branch / commit prefix:** `step/43-*`, `feat(step-43): …`.

---

## Goal

`kupferdraht-core` is already URL-agnostic — `ServerConfig { api_base_url,
websocket_url, title }` flows through `ClientBuilder` and is persisted
per account. But the CLI's `Backend` clap `ValueEnum` hardcoded exactly
two instances (`staging` → zinfra.io, `production` → wire.com), so a
self-hosted Wire-compatible deployment was unreachable — even though
`docs/backend-spec.md` §20 defines conformance as "an unmodified Kupferdraht
client works by changing only its base URL".

Step 43 adds that missing base-URL path to the CLI:

- `--api-url <https://…>` on `login` / `register` / `activate`, as the
  alternative to `--backend staging|production` (exactly one of the
  two; clap enforces via `conflicts_with` / `required_unless_present`,
  `resolve_server` guards again).
- `--ws-url <wss://…>` optional override; when omitted the WebSocket
  URL is derived by Wire's deployment naming convention
  (`https://nginz-https.<domain>` → `wss://nginz-ssl.<domain>`, other
  hosts keep their host). The WebSocket is not consumed yet
  (backend-spec §17) but `ServerConfig` persists it, so it must be
  right from day one.
- Validation: `--api-url` must be a bare `https://` origin (no path),
  `--ws-url` must be `wss://` — TLS mandatory, backend-spec §18.1.
- `title` of a custom `ServerConfig` = the API host.

Design: new module `kupferdraht-cli/src/backend.rs` (`BackendPreset`,
`resolve_server`, `custom_server`, `derive_websocket_url`) replaces the
`Backend` enum + `to_server()` in `main.rs`; the three handlers take a
resolved `ServerConfig`.

## What this is not

- No change to `kupferdraht-core` — no public API, FFI, or Wire-layer
  change (hence no FFI mirror, no api-mapping entry).
- No committed instance URLs beyond Wire's own two presets — private
  deployment URLs stay in flags/env, not in the repo.
- No config file / env-var resolution layer; flags only (a
  `KUPFERDRAHT_API_URL` convenience can come later if wanted).

## Tests (phase 3, red first)

13 unit tests in `kupferdraht-cli/src/backend.rs::tests`: the two presets
pin their exact URLs/titles; custom URL round-trip, trailing-slash
trim, nginz `wss` derivation + fallback + explicit override, title =
host; rejections (non-https API URL, non-wss WS URL, preset+URL,
neither, `--ws-url` without `--api-url`) as
`KupferdrahtError::InvalidInput`.

## Smoke

`kupferdraht-cli login --api-url` against a live self-hosted
Wire-compatible instance (v14 in `GET /api-version` verified); an
invalid-credentials rejection from the target host proves URL
resolution, TLS, and the v14 handshake path end to end without
creating state.
