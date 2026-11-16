# Step 159 — Compressed answers: gzip on the native transport, the decoded bytes bounded

**Status: commissioned 2026-11-16** (Volker), from a comparison with Kalium: Kalium
asks Wire for gzip-compressed answers, Kupferdraht's native builds never do
(observations §103, kalium-readings entry 26). Both parts belong in one step: gzip
makes an oversized answer cheap for a hostile server to send, so the bound comes
with it. The bound is 16 MiB (Volker, the same day): the bound `wire-server-rs` sets
on a decoded request body at galley and gundeck. Wire's Haskell services set none
on a decoded body (as `wire-server-rs` read their code, not measured); nginz bounds requests before decoding (512 KiB by default, 40 MiB
for Proteus messages, 70 MiB for MLS commit bundles).

## Problem

**The native builds download every answer uncompressed.** Wire's nginz compresses
`application/json` answers of 1 KiB and more when the request carries
`Accept-Encoding: gzip`; live, Wire's production backend sent its 602 KB OpenAPI
document as 83 KB. Kupferdraht's native reqwest (0.12.28, `kupferdraht-core/Cargo.toml`,
the `cfg(not(wasm32))` block) is built without reqwest's `gzip` feature, so it sends
no `Accept-Encoding` and nginz answers uncompressed. Nothing fails; the bytes are
only more. That reaches the CLI and the Kotlin and Swift bindings, whose traffic goes
through Kupferdraht's reqwest, not through OkHttp or URLSession, which would ask on
their own. The browser build is not affected: `fetch` asks and decodes by itself.

What is gained, estimated (synthetic answers of Wire's shape, gzip level 6 as nginz;
phase 1 measures real ones): a `GET /notifications` page of 100 MLS messages shrinks
by about 40–60 % (the ciphertext barely compresses, its Base64 and the JSON around it
do), lists of users, rooms and team members by more. Polling, the default
transport, reads such pages on every pass, and a device catching up after a long
time offline reads many.

**Almost no answer is bounded today.** `decode_response` reads a JSON answer whole
(`resp.json::<T>()`), `send_keeping_answer` and `reject_from_response` read their
text whole and bound only what they keep (`redact::bounded_detail`, Step 137). Only
the avatar download reads through a bound (`read_bounded`, Step 144). A hostile
server can already send an oversized answer; with gzip on, a few KB of it inflate to
GB, a thousandfold cheaper for the sender.

## Scope

1. **gzip on.** reqwest's `gzip` feature in the native block of
   `kupferdraht-core/Cargo.toml`: every native client sends `Accept-Encoding: gzip` and
   decodes. Phase 1 decides whether the asset `transfer` client keeps asking: an
   encrypted blob is `application/octet-stream`, which nginz does not compress, and
   Kalium's asset client asks for nothing.
2. **The decoded bytes bounded.** Every answer read into memory is read through a
   bound over the decoded stream, as `read_bounded` reads an avatar: the JSON answers
   (`decode_response`), the kept texts (`send_keeping_answer`, the rejection detail)
   and the dozen direct `json` / `text` reads beside them in `wire/http/` (phase 1
   lists them). Past the bound the read stops with a recoverable error, never a panic
   (CLAUDE.md, Assertions). One constant, 16 MiB. A notifications page holds 100
   events, so it fits 160 KiB per event on average; the largest single answers
   (a large room's group info, a full member list) are estimated well below it.
3. **The browser build.** `fetch` decodes by itself, and reqwest's wasm response has no
   chunk reader without the `stream` feature (Step 144). Phase 1 decides whether the
   bound is checked after the read there, as `read_bounded` does on wasm, or reqwest's
   `stream` feature is turned on for wasm too. The browser-only token exchange
   (`storage/remote/token_exchange.rs`) gets the same bound.
4. **Dependencies.** `flate2` is in the graph already, through `png` (Step 144); what
   reqwest's `gzip` adds (`async-compression` and its codecs) is measured with
   `cargo tree` and passes `cargo deny`; `THIRD-PARTY-NOTICES` and
   `docs/THIRD_PARTY_LICENSES.html` name it.
5. **Docs.** `docs/wire/api-notes.md` (Kupferdraht asks for gzip), `docs/THREAT_MODEL.md`
   (a hostile server's oversized answer and its bound), `CHANGELOG.md`.

### Phase 1

- The error a bound answer becomes (`Network`, `Encoding` or a variant of its own) and
  what a sync pass does with it.
- The `transfer` client's `Accept-Encoding` (Scope 1) and the browser's bound
  (Scope 3).
- The real saving: a notifications page and the room list of the local stack, with and
  without gzip.

## Out of scope

- Compressed request bodies: nginz decodes none, Kalium sends none.
- The WebSocket (`permessage-deflate`): cannon's source configures no compression,
  Kalium installs no deflate extension (observations §103).
- Brotli and zstd: nginz offers gzip only.
- The public API: nothing changes there, so no FFI or binding change and no CLI
  command; the existing `sync` exercises the transport.

## TDD sketch

- A request to a JSON endpoint carries `Accept-Encoding` naming `gzip` (httpmock).
- A notifications page served with `Content-Encoding: gzip` yields the same events as
  the same page uncompressed.
- A gzip answer of a few KB that inflates past the bound fails with the phase-1 error,
  without reading the rest.
- An uncompressed answer past the bound fails the same way: the bound is on the
  decoded bytes, whatever the encoding.
- A rejection with an oversized body still classifies (`BackendRejected`, bounded).
- The browser build: what phase 1 decides for Scope 3, in the browser suite.
- Smoke: the local stack (its nginz compresses like Wire's); the run is the usual one,
  and phase 1 names how a compressed answer is seen in it.
