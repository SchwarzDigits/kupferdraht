# Step 160 — The asset download bounded: the sender's size, a ceiling in the browser

**Status: commissioned 2026-11-16** (Volker), from what Step 159 found: the browser's
whole-asset download reads an encrypted file into memory without any bound, and Step
159's answer bound does not fit it, since an asset may be larger than 16 MiB
(`THREAT_MODEL.md` 0.16).

## Problem

**The browser buffers a whole asset without a bound.** `AssetSummary::download` (wasm,
W7) fetches the ciphertext through `download_asset_bytes`, which reads the body whole,
and decrypts it in memory: ciphertext and plaintext sit in the wasm instance at once.
A hostile server can make a browser hold an arbitrarily large blob when an attachment
is opened; the SHA-256 check (L6) runs only after the whole body is in memory.

**Natively the download streams to disk without a bound.** `download_asset` writes
the body to the cache through `stream_response_to_path`, which bounds a stall (Step
98) but not the size: a hostile server can fill the disk.

**The right size is known before the download.** The sender declares the plaintext
size in `Asset.Original.size` (`size_bytes` on the handle), inside the MLS-encrypted,
MLS-authenticated `GenericMessage`, which the server cannot change. AES-256-CBC with
the IV prefix (ADR-0011) makes the ciphertext at most 32 bytes longer: 16 bytes IV and
at most 16 bytes padding. Wire's cargohold counts the same way: its upload limit is
`maxTotalBytes: 104857632 # limit to 100 MiB + 32 bytes` for team users and
`maxTotalBytesStrict` (25 MiB + 32 bytes) for others, both configurable per
deployment (wire-server's Helm chart, `CargoHold/API/Public.hs`).

## Scope

1. **Per asset: the sender's size + 32 bytes.** Natively the stream stops at the
   first chunk past `size_bytes + 32`, and the partial file is removed; in the browser
   the read is bounded the same way. Past it a recoverable error.
2. **In the browser a ceiling: 100 MiB + 32 bytes**, Wire's default upload limit for
   team users. An asset whose declared size exceeds it is refused before any request:
   the in-memory path holds the ciphertext and the plaintext, so a sender declaring an
   enormous size would otherwise still be admitted. No fixed ceiling natively: the
   download streams to disk, and an operator may raise `maxTotalBytes`.
3. **The browser's read.** reqwest's wasm response has no chunk reader without the
   `stream` feature (Step 159): a declared `Content-Length` past the bound is refused
   before reading, the read length after it. Phase 1 decides whether that suffices
   here or the download reads the body as a stream.
4. **Docs.** `THREAT_MODEL.md` (the download bounded), `docs/library.md` and
   `docs/bindings.md` (the refusals and their errors), `docs/wire/asset-protocol.md`
   (the size rule), `CHANGELOG.md`. Step 159's pin test names `download_asset_bytes` as
   its one exception; it stays one (an asset is no answer), with its own bound now.

### Phase 1

- The errors: more bytes than the sender declared (`IntegrityCheck`, as a substituted
  blob, or `BackendRejected`, as Step 159's bound), and a declared size over the
  browser's ceiling (`InvalidInput`, or a variant of its own, which would be BREAKING).
- Whether Wire's own clients set `Original.size` to the file's size: a file sent from
  the webapp and from Kalium, received by Kupferdraht. A client that declares less
  than it uploads would have its files refused, which the SHA-256 check alone would
  have let through.
- The browser's read (Scope 3).

## Out of scope

- Upload limits: cargohold enforces them, and a refused upload already surfaces.
- Streaming the browser download to OPFS instead of memory, which would lift the
  ceiling: a step of its own if large files in the browser are needed.
- Images sent as files and avatars: an avatar has its own bound (Step 144).

## TDD sketch

- Native: a body longer than `size_bytes + 32` stops with the phase-1 error, and no
  file is left in the cache; a body of the right length downloads and decrypts.
- Browser path (`download_asset_bytes` is compiled for native tests): the same bound,
  with httpmock.
- A declared size over the ceiling is refused in the browser path before any request
  (the mock counts none).
- Step 159's pin test still finds every body read; `download_asset_bytes` reads
  through its bound.
- Smoke: the local stack's `send-file` / `download-asset` stay green; phase 1 names
  how a file sent from the webapp is received.
