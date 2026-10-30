# ADR-0011: Asset content protocol — AES-256-CBC over Wire's `/assets` endpoint

**Status:** Accepted
**Date:** 2026-05-05
**Deciders:** Volker Zöpfel

---

## Context

Kupferdraht's first-pass scope was deliberately restricted to "Wire's full text-messaging feature set" (CLAUDE.md §Functional Scope). File transfers, image / audio / video attachments were explicitly out of scope and `GenericMessage.Asset` variants were ignored gracefully on receive (PLAN.md §Explicitly out of scope, §3105).

With Step 9 complete, the codebase has reached the natural inflection point at which file attachments become the next addition: text messaging is end-to-end live-validated against a live Wire v14 backend, the dependency graph is GPL-free, security baseline (DEK/KEK + per-field encryption) is in place, and the `MlsClient` encrypt / decrypt path is already content-agnostic. Adding `Asset` does not require a second crypto plane — it requires extending `GenericMessage` with a new content variant whose payload happens to point at an off-MLS encrypted blob.

This ADR fixes the **content-layer protocol** that Kupferdraht's asset implementation must mirror: how the file is encrypted, what bytes go into MLS, what bytes go to Wire's asset endpoint, and how the receiver puts the two halves back together. Without this fixed, the future PLAN-step is a wager on protocol details. With it fixed, the step becomes mechanical.

### How file attachments work in Wire (the two-channel pattern)

Files do not travel through MLS. They travel through a separate Wire REST endpoint (`/assets`), encrypted with a one-time symmetric key. The MLS message carries only a small reference: the encryption key, a content hash, and the asset locator. This is the **External Content** pattern — same shape as `draft-ietf-mimi-content`, generalised across MLS-based messengers.

Concretely:

1. **Sender** generates a fresh AES-256 key, encrypts the file, hashes the resulting ciphertext, uploads the ciphertext to Wire's `/assets` endpoint, receives an `(asset_id, asset_token, asset_domain)` triple from the response.
2. **Sender** wraps `(asset_id, asset_token, asset_domain, otr_key, sha256, mime_type, size, …)` into an `Asset` content variant inside a `GenericMessage`, encrypts that as an MLS application message, and posts it to `/v14/mls/messages` — exactly the same MLS pipeline Kupferdraht already uses for text.
3. **Receiver** decrypts the MLS message, sees the `Asset` variant, downloads the ciphertext from `/assets/{asset_domain}/{asset_id}`, verifies SHA-256, decrypts with `otr_key`, and surfaces the plaintext to the consumer.

The Wire backend's Delivery Service sees the encrypted blob and its size, but cannot link it to a recipient (delivery is anonymous via the Asset-Token header) and cannot read the contents (the AES key never leaves MLS-protected territory). The MLS layer authenticates the *sender* of the reference; the SHA-256 binds the reference to the file. Together, MLS and the hash function as an authenticated-encryption construction at the message-protocol level.

### The crypto choice is constrained, not chosen

The survey of Kalium (`docs/wire/asset-protocol.md`, 2026-05-05) established what Wire's clients actually do today:

- **AES-256 in CBC mode with PKCS5 padding**, no HMAC. This was a surprise — the conventional construction would be CBC + HMAC-SHA256 ("encrypt-then-MAC"). Wire deliberately uses unauthenticated CBC.
- **Integrity is delivered out-of-band** via `Asset.RemoteData.sha256`, a SHA-256 hash over the full ciphertext (including the IV prefix).
- **The 32-byte AES key alone** is what travels in `Asset.RemoteData.otr_key` — no key concatenation, no separate MAC key, no IV.
- **The 16-byte IV is prepended to the ciphertext stream** before upload, so the encrypted blob is `iv(16) || aes-cbc-pkcs5(plaintext)`.

This means the integrity guarantee on assets has two parts:

- The SHA-256 prevents a network attacker from substituting or modifying the encrypted blob undetected (the receiver verifies the hash before decrypting).
- The MLS layer prevents an attacker from substituting the SHA-256 itself, because the `Asset.RemoteData.sha256` field is inside an MLS-encrypted, MLS-authenticated `GenericMessage`.

Kupferdraht cannot choose differently here. Choosing CBC + HMAC, or AES-GCM, would produce assets that no other Wire client can decrypt. Interoperability with the existing Wire ecosystem is the entire point — a non-interoperable Kupferdraht is, for this feature, useless. The crypto construction is therefore not a Kupferdraht decision but a Wire-protocol given.

### Why this needs an ADR despite the constraint

Three things are still genuinely Kupferdraht decisions, and each deserves explicit acknowledgment:

1. **Streaming model.** Kalium streams encrypt-then-upload via a tempfile; it never holds a full asset plaintext in memory. Kupferdraht has the choice between mirroring this (memory-constant, slightly more code) or buffering small assets in memory (simpler, RAM-bound). With files plausibly reaching 100 MB or more on Wire's production backends, the buffering path is a footgun.
2. **Plaintext persistence.** Wire's mobile clients decrypt on demand and never persist plaintext blobs. Wire's web/desktop clients persist plaintext blobs for repeat access. This is a threat-model choice, not a protocol choice — and Kupferdraht's existing per-field encryption posture (Step 7) implies the conservative path.
3. **Receive-side AES-GCM toleration.** The `EncryptionAlgorithm` enum in `Asset.RemoteData` lists `AES_GCM = 1` alongside `AES_CBC = 0`. Kalium does not emit GCM and does not have non-test code to receive GCM. Kupferdraht needs an explicit decision about what to do when an inbound asset is GCM-flagged: hard error, silent skip, or eventual implementation.

This ADR fixes all three.

---

## Options Considered

### Option A — Reuse the MLS application-message channel for the file content itself

Stuff the encrypted file directly into a `GenericMessage` and let MLS encrypt it as an application message. No `/assets` endpoint, no separate AES key — MLS does everything.

- **Con:** Breaks Wire interop. No other Wire client expects file bytes inside MLS application messages.
- **Con:** The Wire backend's `/v14/mls/messages` endpoint enforces a per-message size cap (kilobytes, not megabytes). Files would not even fit.
- **Con:** MLS application messages live forever in the Delivery Service's outbound queue per recipient device until acked. A 50 MB file fanned out to 8 devices is 400 MB queued.
- **Con:** MLS Forward Secrecy provides per-message ratcheting; storing large files inside this stream complicates the receive-side ratchet bookkeeping for no security gain.

Rejected on interop grounds alone.

### Option B — Implement Wire's deployed asset protocol exactly (AES-256-CBC + SHA-256, send-side)

Implement what `docs/wire/asset-protocol.md` documents, byte-for-byte. Send only `Status.Uploaded` with `EncryptionAlgorithm::AES_CBC`. Tolerate AES_GCM-flagged inbound content via warn-and-skip.

- **Pro:** Interoperable with every Wire client in production today.
- **Pro:** Crypto primitives are all in the RustCrypto stack under permissive licenses (`aes` 0.8, `cbc` 0.1, `cipher`, `block-padding`); no GPL re-introduction.
- **Pro:** SHA-2 (`sha2` crate) is already a Kupferdraht dependency from Step 6-II-b.
- **Pro:** Validatable with known-answer vectors from an independent AES-256-CBC implementation (`openssl enc`), showing Kupferdraht's ciphertext byte-identical for the same `(plaintext, key, iv)` triple.
- **Con:** CBC without authentication is academically suboptimal. Kupferdraht inherits Wire's design choice to delegate authentication to the MLS-layer wrapper. This needs to be explicit in the threat model.
- **Con:** AES_GCM-flagged inbound assets are a future ADR's concern. Kupferdraht temporarily surfaces them as "unsupported asset" to the consumer rather than rendering them.

### Option C — Implement AES-GCM only

Skip CBC entirely. Send only AES-GCM encrypted blobs. Treat CBC inbound as legacy.

- **Pro:** Authenticated encryption built-in; cleaner threat model.
- **Pro:** `aes-gcm` crate is already in Kupferdraht's dependency graph (Step 7a, AES-256-GCM for column encryption).
- **Con:** **Catastrophic interop break.** Kalium does not have non-test GCM-receive code in production. A Kupferdraht-sent asset would not decrypt in any current Wire client. Kupferdraht becomes a write-only file sender.
- **Con:** GCM IV / authenticated-tag layout for Wire is undocumented and Kalium's source does not pin it. We would be inventing a layout other clients have not agreed on.

Rejected — interop break is total.

### Option D — Wait until Wire ships a successor format

Defer asset support until Wire publishes a v2 asset protocol with authenticated encryption.

- **Pro:** Avoids implementing a construction we wouldn't choose from scratch.
- **Con:** Wire has no public roadmap for an asset protocol revision. The current shape is six years old and shows no signs of evolution.
- **Con:** Files are a baseline messenger feature. Indefinite deferral makes Kupferdraht incomplete in a way no downstream consumer will accept.
- **Con:** Even if Wire ships a v2, backward compatibility with v1 will be required for years; we'd need to implement v1 anyway.

Rejected — indefinite deferral is not implementation.

---

## Decision

**Kupferdraht implements asset send and receive in compliance with Wire's existing protocol (Option B), with the three Kupferdraht-specific design decisions resolved as follows.**

### Crypto construction (fixed by Wire's deployed asset protocol)

- Symmetric algorithm: **AES-256-CBC with PKCS7 padding**. (Kalium's source labels this "PKCS5"; PKCS5 and PKCS7 produce identical bytes for 16-byte block ciphers — the label is a JCE historical artifact.)
- Key material: 32 raw AES-256 key bytes, generated once per asset via the OS CSPRNG (`rand::rngs::OsRng`). Travels in `Asset.RemoteData.otr_key` as raw bytes, not concatenated with anything else.
- IV: 16 random bytes generated once per asset, prepended to the ciphertext stream. Does not appear in the protobuf.
- Encrypted blob layout on the wire: `iv(16) || aes-cbc-pkcs7(plaintext)`. This is the byte stream Wire's `/assets` endpoint stores and serves.
- Integrity: SHA-256 over the full encrypted blob (including the IV prefix), carried in `Asset.RemoteData.sha256`. The receiver verifies the hash before decrypting; mismatched ciphertext is rejected.

### Streaming model (Kupferdraht decision)

- Encrypt and upload via **file-streaming**, not in-memory buffering. The encrypt step writes ciphertext to a tempfile inside the OS-level user cache directory; the SHA-256 is computed over the tempfile via streaming reads; the upload then streams the same tempfile as the multipart body.
- Decrypt and download mirror the inverse: download streams to a tempfile, SHA-256 is verified over the downloaded ciphertext, decrypt streams the tempfile to the consumer's chosen sink (`Read`-shaped public API for in-memory consumers, `save_to(path)` for filesystem consumers).
- Memory profile is constant (~64 KB per-buffer regardless of asset size).

### Plaintext persistence (Kupferdraht decision)

- **Plaintext bytes are never persisted by Kupferdraht.** The encrypted ciphertext blob may be cached on disk per-account under `~/.kupferdraht/<account_id>/assets/<message_id>.enc` for repeated access; the consumer requests decryption explicitly and receives a stream / writes to a path the consumer chose.
- The `attachments` SQLite table stores asset metadata (the `otr_key` ciphertext-encrypted via Step 7's per-field DEK, the `sha256`, the `asset_id` triple, the mime type, the size, the local cache path if any). It does not store plaintext.
- This matches Kupferdraht's existing posture from Step 7: nothing user-readable lives on disk in plaintext when the user is logged out.

### AES-GCM toleration policy (Kupferdraht decision)

- **Outbound: never `AES_GCM`.** Kupferdraht emits only `EncryptionAlgorithm::AES_CBC` until Wire's clients demonstrate consistent GCM-receive support and the GCM byte layout is pinned in a successor ADR.
- **Inbound: warn-and-skip for `AES_GCM`.** An AES_GCM-flagged asset is decoded into the timeline as an "unsupported asset" item with the metadata visible (mime, size, filename) but the body inaccessible. The MLS message itself is acknowledged so the sync stream advances; the consumer can render an explanatory placeholder.
- This matches the existing pattern for unknown `GenericMessage` content variants (Step 5d): graceful degradation, no protocol-level rejection, future-extensible without breaking deployed clients.

### Wire endpoint selection

- Upload: **`POST /v14/assets`** (Wire API v14 inherits the V2-shaped path, no `/v3` suffix).
- Download: **`GET /v14/assets/{asset_domain}/{asset_id}`** with the `Asset-Token: <token>` header for private assets.
- Delete: **`DELETE /v14/assets/{asset_domain}/{asset_id}`** (optional in V1 of this step; sender-only feature, surfaced via a future Kupferdraht API but not blocking).
- Multipart body uses a manually-built `multipart/mixed` stream with boundary `frontier`, two parts (JSON metadata + raw ciphertext), the shape Wire's `POST /assets` documents and its clients send (`docs/wire/asset-protocol.md`). V12+ metadata fields (`convId`, `filename`, `filetype`) are populated from V14 onward.

### Cryptographic dependencies (new to Kupferdraht)

Three new direct dependencies, all permissive-licensed and already widely used in the Rust crypto ecosystem:

- `aes` 0.8 (Apache-2.0/MIT) — AES-256 block cipher.
- `cbc` 0.1 (Apache-2.0/MIT) — CBC mode of operation, generic over the block cipher.
- `md-5` 0.10 (Apache-2.0/MIT) — for the `Content-MD5` transport-level header on the upload multipart body (Wire's backend validates this; not a security primitive).

`sha2` is reused from the existing graph. `cipher` and `block-padding` come transitively from `cbc`. No new git-source dependencies; all from crates.io. `cargo deny check` continues to pass.

### Functional scope expansion

CLAUDE.md §Functional Scope and PLAN.md §Explicitly out of scope are amended:

- **Removed from out-of-scope:** "File transfers and media messages."
- **New in-scope item:** "Asset send + receive — file attachments via Wire's `/assets` endpoint, AES-256-CBC encryption, MLS-protected reference."
- **Still out of scope:** Voice / video / calling, asset previews (separate `Asset.Preview` upload cycle, deferred to a follow-up step), AES-GCM emission.

The scope statement evolves from "Wire's full text-messaging feature set" to "Wire's full messaging feature set, exclusive of calling and previews."

---

## Consequences

### Positive

- Kupferdraht becomes feature-complete for the everyday messenger use case: send a document, receive a photo, drag-and-drop a video. Without this, Kupferdraht is a text-only product with limited applicability for any consumer beyond chat.
- The two-channel pattern (file content via REST, reference via MLS) is reusable. Future Wire content variants that follow the same pattern (location, calling-cards, e2ei attestations) can extend the protobuf schema and the storage layer without touching the MLS encrypt path.
- The streaming model means Kupferdraht can handle large files (audio, video, archives) without blowing up the host process. Memory is constant regardless of asset size.
- The encrypted-cache-on-disk + decrypt-on-demand posture is cleaner than Kalium's plaintext-on-disk model from a threat-modeling perspective. A Kupferdraht-using consumer can claim "no plaintext attachments at rest" without further qualification.
- The `Asset` content variant integrates into the existing Step 6-II Timeline read API. No new presentation-layer plumbing — `TimelineItemKind` gains an `Asset` variant alongside `Text`, and the consumer renders accordingly.

### Negative

- Kupferdraht inherits Wire's CBC-without-HMAC choice. The threat model section of `docs/THREAT_MODEL.md` needs an explicit entry acknowledging that asset integrity depends on the MLS-layer wrapper rather than a cipher-mode MAC. This is correct in principle but warrants explicit documentation.
- The `attachments` table is the first table where the size of one row can be megabytes (the encrypted blob path's metadata is small, but adjacent tables grow with asset references). The V17 migration must consider per-account storage caps for the cache directory; an explicit cap is recommended (e.g. 1 GB ring-buffer with LRU eviction).
- AES-GCM-flagged inbound assets become a visible product gap. Until a successor ADR resolves the GCM byte-layout question, Kupferdraht's UI must communicate "this asset was sent in a format Kupferdraht doesn't yet support" rather than failing silently.
- New transitive dependencies enter the graph: `aes`, `cbc`, `md-5`, plus their internal helpers (`cipher`, `inout`, `block-padding`). All permissive, all already common in Rust crypto deployments, but the `cargo about` regenerated `THIRD_PARTY_LICENSES.html` will visibly grow.

### Neutral

- The MLS layer is unchanged. `MlsClient::encrypt_text_message` is the same code path (its docs already say "future content variants … reuse this exact code path unchanged").
- Wire's backend cannot distinguish a Kalium-sent asset from a Kupferdraht-sent asset, by design. Live-validation against a live Wire v14 backend verifies this property.
- Public API expansion is small: one new method on `Timeline` (`send_file`), one new variant on `TimelineItemKind`, one new struct (`Asset` with metadata fields and an async `download` method). The matrix-rust-sdk drift register documents this against the (slightly different) `Timeline::send_attachment` shape — the names diverge, the semantics align.

---

## Implementation Notes

### Module layout

New files:

- `kupferdraht-core/src/encryption/asset_crypto.rs` — AES-256-CBC encrypt + decrypt, IV layout, SHA-256 computation. Pure functions over `Read` / `Write` streams. ~150 lines, well-bounded.
- `kupferdraht-core/src/wire/http/assets.rs` — `POST /assets` (multipart/mixed upload), `GET /assets/{domain}/{id}` (streamed download), `DELETE /assets/{domain}/{id}` (stretch goal). ~250 lines.
- `kupferdraht-core/src/timeline/asset_send.rs` — orchestrator that wires encrypt + upload + persist + MLS-send into a single `Timeline::send_file` call.
- `kupferdraht-core/src/timeline/asset_receive.rs` — inbound dispatcher for `GenericMessage::Content::Asset`, called from `sync/dispatch.rs::dispatch_mls_message`.
- `kupferdraht-core/src/storage/migrations/V17__step_10_attachments.sql` — new `attachments` table.

Extended files:

- `kupferdraht-core/proto/generic_message.proto` — adds `Asset`, `Asset.Original`, `Asset.RemoteData`, `Asset.NotUploaded`, `EncryptionAlgorithm`, plus the field-5 `Asset asset` entry in the `oneof content`.
- `kupferdraht-core/src/storage/records.rs` — new `AssetRecord` struct.
- `kupferdraht-core/src/sync/dispatch.rs` — new `dispatch_inbound_asset` arm.
- `kupferdraht-core/src/sync/event_handler.rs` — new `MessageBody::Asset(AssetSummary)` variant.
- `kupferdraht-core/src/timeline/items.rs` — new `TimelineItemKind::Asset` variant.
- `kupferdraht-core/src/lib.rs` — re-export `Asset`, `AssetSummary`.
- `kupferdraht-cli/src/main.rs` — new `send-file` and `timeline-download` subcommands.
- `Cargo.toml` (kupferdraht-core) — `aes`, `cbc`, `md-5` added.
- `deny.toml` — no new entries; all three new crates fall under the existing permissive allow-list.

### Public API shape

```rust
// In timeline/asset_send.rs (re-exported from lib.rs):

pub struct AssetSummary {
    pub message_id: String,
    pub mime_type: String,
    pub filename: Option<String>,
    pub size_bytes: u64,
    pub sender_user_id: String,
}

impl Timeline {
    /// Upload-and-send a file as an Asset content variant.
    /// Encrypts with a fresh AES-256 key, uploads to Wire's /assets,
    /// then sends the reference through MLS as a GenericMessage.
    /// Returns once the MLS message is acknowledged.
    pub async fn send_file(
        &self,
        path: &Path,
        mime_type: &str,
    ) -> Result<MessageHandle, KupferdrahtError>;
}

impl AssetSummary {
    /// Stream the decrypted plaintext to the caller.
    /// Verifies SHA-256 before decrypting; rejects mismatched ciphertext.
    pub async fn download(&self) -> Result<impl AsyncRead, KupferdrahtError>;

    /// Convenience: decrypt + write to a filesystem path.
    pub async fn save_to(&self, path: &Path) -> Result<(), KupferdrahtError>;
}
```

UDL additions:

```idl
interface Timeline {
    // existing methods...
    [Throws=KupferdrahtError, Async]
    MessageHandle send_file(string path, string mime_type);
};

dictionary AssetSummary {
    string message_id;
    string mime_type;
    string? filename;
    u64 size_bytes;
    string sender_user_id;
};

interface AssetSummary {
    [Throws=KupferdrahtError, Async]
    sequence<u8> download();   // for UniFFI; byte buffer rather than stream
    [Throws=KupferdrahtError, Async]
    void save_to(string path);
};
```

Note: UniFFI does not yet expose `AsyncRead`-shaped streaming back to Kotlin / Swift cleanly. The bindings layer surfaces `download()` as a buffered `Vec<u8>` for now; the streaming path is reachable from native Rust consumers (kupferdraht-cli, future Rust integrations). This is documented as a known drift in `docs/api-mapping-matrix-rust-sdk.md` once the implementation lands.

### Validation strategy

- **Known-Answer Vectors.** Fixed `(plaintext, key, iv)` triples whose ciphertext and SHA-256 are produced by an independent implementation (`openssl enc -aes-256-cbc -K … -iv …` and `sha256sum`), asserted byte-identical against Kupferdraht's encrypt path. Not taken from any Wire repository (revision 1.2).
- **Round-trip tests** in `kupferdraht-core/tests/step_10_asset_round_trip.rs`: encrypt → upload (mock) → download (mock) → decrypt → assert plaintext equality, for sizes ranging from 1 byte to 50 MB.
- **Live-smoke against a live Wire v14 backend**: send a 1 MB image to a known counterpart account, verify it renders in Wire web; receive a Wire-web-sent file, verify decryption and SHA-256 match. Smoke is the gating criterion for "Step 10 complete".
- **AES-GCM tolerance**: synthetic test asserting an inbound Asset with `EncryptionAlgorithm::AES_GCM` is decoded into an "unsupported asset" timeline item without panic and without dropping the MLS sync stream.

### Threat model addendum

`docs/THREAT_MODEL.md` gains a new section "Asset content authenticity":

> Asset ciphertext is encrypted with AES-256-CBC, an unauthenticated mode. The integrity of an asset relies on the SHA-256 hash carried inside `Asset.RemoteData.sha256`, which itself is delivered through an MLS-encrypted, MLS-authenticated `GenericMessage`. An attacker who can substitute an asset blob in transit cannot do so undetected: the receiver computes SHA-256 over the downloaded ciphertext and compares it against the hash. An attacker who can substitute the entire `GenericMessage` would need to forge MLS sender authentication, which is the same threat model that already protects every text message.
>
> The construction is cryptographically sound at the protocol level, but it does inherit the MLS-layer's properties rather than achieving authenticated encryption locally. A future Wire protocol revision to AES-GCM (or AES-CBC + HMAC-SHA256 in encrypt-then-MAC framing) would strengthen this further; Kupferdraht will adopt such a revision when Wire's clients standardise it.

### Out of scope for this ADR

- Asset **preview** generation and rendering. `Asset.Preview` is a separate upload/download cycle with its own `RemoteData` block. Deferred to a follow-up step (call it Step 10-II if it materialises). Kupferdraht's V1 of asset support shows the `Asset.Original` metadata (mime, size, filename, dimensions) but does not generate or display preview thumbnails.
- AES-GCM **emission**. Receive-side toleration is in scope; sending GCM is not.
- Asset **deletion** UI. The wire layer (`DELETE /assets/{domain}/{id}`) is implemented as a stretch goal but not surfaced in `Timeline` / `Room`. Owner-driven asset cleanup is a future user-facing feature.
- Per-asset **expiry** policy (`retention=expiring`). Kupferdraht uploads everything as `retention=persistent`, matching Kalium's E2EE asset path.
- **Federation across non-Wire backends.** Asset URLs that point at non-Wire-federation domains are out of scope; Kupferdraht assumes asset_domain resolves through the same federation graph as conversation_domain, which Wire's federator handles server-side.

---

## References

- ADR-0001 — `openmls` + `proteus` directly. Asset support uses the same MLS path; no new MLS-layer code.
- ADR-0003 — per-field encryption. The `attachments` table follows the same DEK-encrypted column pattern for `otr_key_ct`.
- ADR-0009 — matrix-rust-sdk-shaped public API. `Timeline::send_file` is a Kupferdraht-specific drift entry; `Timeline::send_attachment` is the matrix-rust-sdk neighbour.
- ADR-0010 — GPL-free dependency graph. The new `aes` / `cbc` / `md-5` crates are all permissive and do not regress this property.
- `docs/wire/asset-protocol.md` (2026-05-05) — Kalium reference survey. The source-of-truth document for every protocol detail this ADR fixes.
- `docs/THREAT_MODEL.md` — gains the "Asset content authenticity" addendum after Step 10 lands.
- `reference/wire.swagger.json` — `POST /assets`, `GET /assets/{domain}/{id}`, `DELETE /assets/{domain}/{id}` definitions.
- IETF draft-ietf-mimi-content — generalised External Content pattern for MLS messengers, the same shape Wire's asset protocol implements.

---

## Revision History

| Version | Date | Change |
|---|---|---|
| 1.0 | 2026-05-05 | Initial ADR. Decides AES-256-CBC + SHA-256 mirror of Kalium's protocol; file-streaming model; no plaintext on disk; AES-GCM receive-side warn-and-skip; functional scope expansion to "messaging exclusive of calling and previews". |
| 1.1 | 2026-10-08 | Wording: validation targets named as a live Wire v14 backend, not Wire's hosted service. Decision unchanged. |
| 1.2 | 2026-10-09 | The known-answer vectors are generated with an independent tool, not extracted from Kalium's tests: the earlier instruction contradicted ADR-0009's protocol-observation-only rule and was never carried out. The construction is described as Wire's deployed protocol rather than as a mirror of Kalium. Decision unchanged. |
| 1.3 | 2026-10-20 | Option B's title and its remaining "vectors extracted from a Kalium-encrypted asset" pro brought in line with revision 1.2 (independent generator; Wire's deployed protocol, not Kalium, is the reference). Nothing else changed. |
