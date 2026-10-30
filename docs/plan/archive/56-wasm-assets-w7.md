# Step 56 — Browser assets/files (W7) + remaining W10 bindings

> **Status:** MERGED to `main` — `c85f244`, PR #44 (merge `b8dd044`,
> 2026-07-07). Scope: **W7** in-memory (buffer-based) browser asset
> crypto + transfer (`sendFile`/`downloadAsset`) + the remaining W10
> binding gaps (invite/kick/leave/logout). Native (all-targets) clippy +
> 368 tests green, wasm build/clippy green, prototype `vue-tsc` + Vite
> build green. Live verification pending (as of 2026-07-16) — the file's
> own Verification section: "Pending (browser): attach + send an image
> between two accounts; receive + view inline; SHA-256 verified on
> download."
>
> **Branch / commit prefix:** committed on `wasm`; `step/56-*`.

## Goal

File sharing in the browser (**W7**) plus the last W10 binding gaps
(invite/kick/leave/logout). The native asset pipeline is filesystem +
streaming-multipart based and stubbed on wasm; this adds an **in-memory,
buffer-based** path (small files as `Vec<u8>`/`Uint8Array`).

## W7 — in-memory asset crypto + transfer

Everything the native path does to/from tempfiles + streams, done in memory.
Same wire format (`iv(16) || aes-cbc-pkcs7`, SHA-256 over the whole blob):

- **Crypto** (`encryption/asset_crypto.rs`, wasm): `encrypt_bytes(&[u8]) ->
  {key, sha256, ciphertext, plaintext_size}` and `decrypt_bytes(ct, key, sha) ->
  Vec<u8>`, reusing the already-`Read`/`Write`-generic `stream_encrypt`/
  `stream_decrypt` cores (`Cursor`/`Vec` in place of `File`).
- **HTTP** (`wire/http/assets.rs`, wasm): `upload_asset_bytes` assembles the
  same `multipart/mixed; boundary=frontier` body as **one `Vec<u8>`** and POSTs
  via `.body(Vec<u8>)` (core reqwest — no `multipart`/`stream` feature needed on
  the fetch backend); `download_asset_bytes` buffers the whole body via
  `resp.bytes()`.
- **Send** (`timeline/asset_send.rs`, wasm `Client::send_bytes`): policy gate →
  `resolve_room_group_id` → `encrypt_bytes` → Content-MD5 over the buffer →
  `upload_asset_bytes` → `build_asset_generic_message` → `encrypt_and_upload`
  (MLS) → persist `timeline_events` + `attachments` (`cache_path = None`) →
  `persist_mls()`. No tempfile staging, no cache move.
- **Receive** (`timeline/asset_receive.rs`, wasm `AssetSummary::download`):
  `download_asset_bytes` → SHA verify → `decrypt_bytes`. No cache file; drops the
  `base_dir` requirement.
- Forwarders in `client/http_calls.rs` (`http_upload_asset_bytes`/
  `http_download_asset_bytes`).

## W10 — remaining bindings (`src/wasm/manager.rs`)

`invite(roomId, userIds)`, `kick(roomId, userIds)`, `leave(roomId)`,
`logout()` (server-side `zuid` revocation), plus the W7 surface:
`sendFile(roomId, bytes, filename, mime)` and `downloadAsset(roomId, itemId) ->
Uint8Array`. The timeline projection now carries `asset: { filename, mime, size }`
for `kind == "asset"` items.

## Prototype

Composer gains a 📎 attach button (reads the file → `Uint8Array` → `sendFile`);
asset messages render a download button, and images display inline via a
`Blob` object-URL from `downloadAsset`.

## Verification

- Native (all-targets) clippy + 368 tests green; wasm (`--lib`) clippy green;
  `wasm-pack` emits the typed `sendFile`/`downloadAsset` API; prototype
  `vue-tsc` + Vite build green. Native asset paths unchanged.
- **Pending (browser):** attach + send an image between two accounts; receive +
  view inline; SHA-256 verified on download.

## Out of scope / follow-ups

- **Large files / OPFS.** The buffer path holds the whole asset in memory
  (fine for images/small files); streaming to OPFS for large assets is a later
  step. The 64 MB decrypt cap still applies conceptually.
- **W2 Web Worker keystore** (Step 57) remains; plus the W4 `sync_stream`/typing
  variants.
