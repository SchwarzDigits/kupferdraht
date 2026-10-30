# Step 11 — Asset content (file attachments)

> **Status:** merged — `78577f1` (live 2026-05-11, send-file round-trip byte-identical). ADR-0011. Sub-steps: 11a `056e864` · 11b `d53d0b8` · 11c `5355419` · 11d `7bbc23e` · 11e `6edf132` (docs-only carryover) · 11f `8bbfe6d` · 11g `64cc73c` · 11h `89a5f7d` + live-smoke fix `34b72b4` (live-smoke 2026-05-11). Archived from PLAN.md.
>
> *Editorial note (2026-10-20):* this archived plan names Kalium as the template for the multipart layout and boundary and proposes known-answer vectors from Kalium's tests. The layout and the `frontier` boundary are those of Wire's own `POST /assets` documentation; the vector item was never executed and was replaced by an independent generator (ADR-0011 revision 1.2). Kept as written for history; see `docs/wire/kalium-readings.md`.
>
> **Sub-steps:** 11a, 11b, 11c, 11d, 11e, 11f, 11g, 11h.
>
> **Historical names:**
> - "Step 11 — Asset content" — 2026-05-09 onward (current).
>
> **Sub-step renaming history:**
> - 2026-05-11: new `11e — Step-10 learnings carryover` inserted
>   after 11d. Original 11e–11g shifted to 11f–11h:
>   - `11e` (old: `Timeline::send_file` + policy gate) → `11f`.
>   - `11f` (old: inbound asset dispatch) → `11g`.
>   - `11g` (old: CLI + live-smoke + scope amendments) → `11h`.
>   Prior commit messages (11a `056e864`, 11b `d53d0b8`, 11c
>   `5355419`, 11d `7bbc23e`) reference the OLD letters in their
>   forward-looking notes — those are historical record, not
>   updated retroactively.
>
> **Branch / commit prefix:** `step/11-*`, `feat(step-11): …`,
> `chore(step-11): …`. No prior commits use this number — the step
> is newly introduced after ADR-0011.
>
> **Note on storage version:** ADR-0011 §Module layout references
> `V17__step_10_attachments.sql`. That label was correct on
> 2026-05-05 (ADR write date) but Step 7b minted V17 for
> `meta.schema_version` and Step 10b minted V18 for
> `feature_configs` + `user_properties`. The actual migration ships
> as **V19** — see §Store additions.

---

## Step 11 — Asset content support

Wire-compatible file attachments. AES-256-CBC encrypted blob
shipped through Wire's `/v14/assets` REST endpoint; reference
(decryption key + content hash + asset locator) carried inside an
MLS-protected `GenericMessage`. Mirrors what every Wire client
does today byte-for-byte.

The crypto construction, byte layout, streaming model, and
plaintext-persistence policy are **fixed by ADR-0011** — this file
is the implementation roadmap. The Kalium reference survey in
[`docs/wire/asset-protocol.md`](../wire/asset-protocol.md) is the
source-of-truth for protocol details (multipart envelope, JSON
metadata field names, retention semantics).

### matrix-rust-sdk analog

`Timeline::send_attachment` is the matrix-rust-sdk neighbour for
Kupferdraht's `Timeline::send_file`. Wire's two-channel pattern (file
content via REST, reference via MLS) is the same External Content
shape that `draft-ietf-mimi-content` generalises across
MLS-based messengers.

The drift entry for Kupferdraht's `Timeline::send_file` /
`AssetSummary::download` lands in
`docs/api-mapping-matrix-rust-sdk.md` once 11h merges.

### Wire API v14 endpoints

- **`POST /v14/assets`** — upload encrypted asset. Body is
  `multipart/mixed` with boundary `frontier`, two parts:
  1. `application/json; charset=utf-8` part with metadata
     (`public`, `retention`, `convId`, `filename`, `filetype`).
  2. `application/octet-stream` part with the raw ciphertext
     (`iv(16) || aes-cbc-pkcs7(plaintext)`).
  Response: `{key, token, domain, expires?}` triple — the
  reference Kupferdraht then ships through MLS.
- **`GET /v14/assets/{domain}/{key}`** — stream-download the
  encrypted blob. `Asset-Token: <token>` header required for
  private assets.
- **`DELETE /v14/assets/{domain}/{key}`** — sender-driven removal.
  Stretch goal in V1; HTTP layer ships the wrapper, but no
  `Timeline` / CLI surface exposes it.

### Wire Message Types

`Asset` content variant added to `GenericMessage.oneof content`
(field 11 — matches Kalium's deployed schema; ADR-0011 §Module
layout's "field-5" reference was a draft-stage error and is
corrected here):

```protobuf
message Asset {
    optional Original    original     = 1;
    // field 2 (Preview) deprecated in Wire's schema; replaced
    // by field 5 below.
    oneof status {
        NotUploaded   not_uploaded = 3;
        RemoteData    uploaded     = 4;
    }
    // field 5 (Preview) — declared in Kalium's schema, not
    // consumed by Kupferdraht's V1 (deferred to a follow-up).
    optional bool          expects_read_confirmation = 6 [default = false];
    // field 7 (legal_hold_status) — not consumed.
}

message Asset.Original {
    string mime_type = 1;
    uint64 size      = 2;
    optional string name        = 3;   // filename
    optional ImageMetaData image = 4;  // optional image dimensions
    optional VideoMetaData video = 5;  // not consumed in V1
    optional AudioMetaData audio = 6;  // not consumed in V1
    optional string source        = 7; // not consumed
    optional string caption       = 8; // not consumed
}

message Asset.RemoteData {
    bytes  otr_key   = 1;   // 32-byte raw AES-256 key
    bytes  sha256    = 2;   // SHA-256 over (iv || ciphertext)
    string asset_id  = 3;
    optional bytes  asset_token = 4;
    optional string asset_domain = 5;
    optional EncryptionAlgorithm encryption = 6;  // AES_CBC = 0
}

enum EncryptionAlgorithm {
    AES_CBC = 0;
    AES_GCM = 1;   // tolerated on receive (warn-and-skip), not emitted
}
```

The exact field numbers and submessage shapes are constrained by
Wire's deployed schema — see `reference/wire.swagger.json` for the
authoritative definition. Kupferdraht does not invent any new fields.

### Public API

```rust
impl Timeline {
    /// Upload-and-send a file as an `Asset` content variant.
    /// Encrypts with a fresh AES-256-CBC key, uploads to
    /// `/v14/assets`, then sends the MLS reference via the same
    /// pipeline `send_text` uses.
    ///
    /// Consults `Client::file_sharing()` (Step 10e) before
    /// touching `/assets`:
    /// - `FileSharingPolicy::Disabled` → InvalidInput, no Wire call.
    /// - `FileSharingPolicy::EnabledSome(allowed)` → check
    ///   `mime_type` against the allow-list; reject with
    ///   InvalidInput on mismatch, no Wire call.
    /// - `FileSharingPolicy::EnabledAll` → proceed.
    pub async fn send_file(
        &self,
        path: &Path,
        mime_type: &str,
    ) -> Result<MessageHandle, KupferdrahtError>;
}

pub struct AssetSummary {
    pub message_id: String,
    pub mime_type: String,
    pub filename: Option<String>,
    pub size_bytes: u64,
    pub sender_user_id: String,
}

impl AssetSummary {
    /// Stream the decrypted plaintext to a buffer. Verifies SHA-256
    /// before decrypting; rejects mismatched ciphertext.
    pub async fn download(&self) -> Result<Vec<u8>, KupferdrahtError>;

    /// Decrypt + write directly to a filesystem path.
    pub async fn save_to(&self, path: &Path) -> Result<(), KupferdrahtError>;
}

pub enum MessageBody {
    Text(String),
    Asset(AssetSummary),  // new in 11g
}

pub enum TimelineItemKind {
    Text { ... },
    Asset(AssetSummary),  // new in 11g
}
```

UDL additions land in 11h (alongside the other Step 10/11 typed
surfaces deferred to a workspace-wide UDL pass).

### Sub-steps

#### 11a — Protobuf schema additions

Add `Asset`, `Asset.Original`, `Asset.RemoteData`, `Asset.NotUploaded`
submessages and the `EncryptionAlgorithm` enum to
`kupferdraht-core/proto/generic_message.proto`. Add the
`Asset asset = 5;` arm to `GenericMessage.oneof content`. The
`Preview` submessage is declared as a placeholder (Kupferdraht decodes
but does not consume).

Field numbers and submessage shapes mirror Wire's deployed schema
verbatim — these are externally constrained. Kupferdraht does not
invent fields.

`build.rs` re-runs `prost-build`; the regenerated
`generic_message.rs` lands as a normal source file (CLAUDE.md does
not exempt generated code from review).

TDD checklist:
- Unit test: round-trip an `Asset` message through `prost::Message`
  encode + decode, asserting field preservation.
- Unit test: decode a known-good Wire-emitted `GenericMessage`
  envelope (captured from `docs/wire/asset-protocol.md` reference
  fixtures) and assert the inner `Asset` fields match.
- Unit test: a `GenericMessage` whose `oneof content` carries
  `Asset` is serialized to the expected wire format.

#### 11b — Crypto module (`encryption/asset_crypto.rs`)

Pure Rust AES-256-CBC + SHA-256 module. Three public functions:

```rust
pub struct EncryptedAsset {
    pub key: [u8; 32],          // AES-256 key (kept in-memory, never persisted)
    pub iv: [u8; 16],           // prepended to the ciphertext
    pub sha256: [u8; 32],       // over (iv || ciphertext)
    pub ciphertext_path: PathBuf, // tempfile holding (iv || ciphertext)
}

/// Encrypt a plaintext file via AES-256-CBC + PKCS7 padding.
/// Generates a fresh key + IV via OsRng. Streams plaintext through
/// the cipher in 64 KB chunks, writing (iv(16) || ciphertext) to
/// a tempfile. Computes SHA-256 over the same byte stream during
/// the write.
pub fn encrypt_file(plaintext_path: &Path) -> Result<EncryptedAsset, KupferdrahtError>;

/// Decrypt an encrypted-asset tempfile to a plaintext sink. Verifies
/// the supplied SHA-256 against the actual byte stream before
/// decrypting; returns `IntegrityCheck` on mismatch.
pub fn decrypt_to_path(
    ciphertext_path: &Path,
    key: &[u8; 32],
    expected_sha256: &[u8; 32],
    plaintext_path: &Path,
) -> Result<u64, KupferdrahtError>;  // returns plaintext byte count

/// Same as `decrypt_to_path`, but returns the plaintext as a
/// `Vec<u8>` for in-memory consumers. Caps at 64 MB to prevent
/// OOM on large assets — callers above that should use the
/// path variant.
pub fn decrypt_to_memory(
    ciphertext_path: &Path,
    key: &[u8; 32],
    expected_sha256: &[u8; 32],
) -> Result<Vec<u8>, KupferdrahtError>;
```

The IV is prepended to the ciphertext stream (per Wire protocol)
and is recoverable from the first 16 bytes of the encrypted blob;
the `EncryptedAsset.iv` field is informational only.

TDD checklist:
- Known-Answer Vectors extracted from
  Kalium's `AESUtilsTest` — for fixed
  `(plaintext, key, iv)` triples, Kupferdraht's encrypt produces
  byte-identical ciphertext to Kalium's.
- Round-trip: encrypt a 1 MB plaintext, decrypt the resulting
  tempfile, assert byte equality.
- Round-trip: 50 MB streamed (memory profile stays under ~200 KB
  at peak — verified via a coarse RSS check on the test process).
- SHA-256 mismatch: tamper with one byte of the ciphertext post-
  encrypt, decrypt rejects with `IntegrityCheck`.
- Wrong key: decrypt with a different 32-byte key, padding-verify
  fails — rejected with `Encoding` (CBC's PKCS7 unpadding catches
  most wrong-key cases statistically).

#### 11c — HTTP layer (`wire/http/assets.rs`)

Three methods on `HttpClient`:

```rust
pub async fn upload_asset(
    &self,
    api_version: u32,
    access_token: &str,
    convert_id: Option<&str>,    // populates JSON metadata's convId
    filename: Option<&str>,
    mime_type: &str,
    ciphertext_path: &Path,
    md5_b64: &str,               // Content-MD5 of the ciphertext
) -> Result<AssetUploadResponse, KupferdrahtError>;

pub async fn download_asset(
    &self,
    api_version: u32,
    access_token: &str,
    asset_domain: &str,
    asset_id: &str,
    asset_token: Option<&str>,
    sink_path: &Path,
) -> Result<u64, KupferdrahtError>;       // returns ciphertext byte count

pub async fn delete_asset(
    &self,
    api_version: u32,
    access_token: &str,
    asset_domain: &str,
    asset_id: &str,
) -> Result<(), KupferdrahtError>;
```

`AssetUploadResponse { key, token, domain, expires? }` is the
direct projection of Wire's response.

The upload body is **manually constructed** as a `multipart/mixed`
stream with the exact byte layout Kalium emits — not via reqwest's
multipart helper, because reqwest emits `multipart/form-data` and
Wire's backend expects `multipart/mixed`. The boundary string
`frontier` is hardcoded (matching Kalium); the JSON metadata part
is the first part, the ciphertext part is the second.

TDD checklist (against `httpmock`):
- `upload_asset` POSTs the expected `multipart/mixed` body.
  Mock captures the body, test asserts the boundary, the
  `application/json` content-type, the metadata JSON shape, the
  `application/octet-stream` content-type for the ciphertext part,
  and the `Content-MD5` header.
- `download_asset` GETs with `Asset-Token` header, streams the
  body to `sink_path`, returns the byte count.
- 4xx/5xx surfaces as `BackendRejected` with the response body
  in the error message (consistent with every other Wire endpoint).

#### 11d — Storage migration V19 + `attachments` table

```sql
CREATE TABLE attachments (
    message_id        TEXT NOT NULL,
    room_id           TEXT NOT NULL,
    mime_type         TEXT NOT NULL,
    filename          TEXT,
    size_bytes        INTEGER NOT NULL,
    sha256            BLOB NOT NULL,                   -- 32 bytes
    otr_key_ct        BLOB NOT NULL,                   -- AES-256-GCM ciphertext (per-field DEK, ADR-0003)
    asset_id          TEXT NOT NULL,
    asset_token       TEXT,
    asset_domain      TEXT NOT NULL,
    encryption_algo   TEXT NOT NULL,                   -- 'AES_CBC' | 'AES_GCM'
    cache_path        TEXT,                            -- absolute path to encrypted-cache tempfile, NULL when not cached
    created_at_ms     INTEGER NOT NULL,
    PRIMARY KEY (message_id, room_id),
    FOREIGN KEY (message_id, room_id) REFERENCES timeline_events(message_id, room_id)
);

CREATE INDEX idx_attachments_room ON attachments(room_id);
```

`otr_key_ct` is encrypted via the per-field DEK exactly like every
other `_ct` column post-Step-7c. The plaintext key never lives on
disk — the `AccountKeystore`-backed unwrap happens on-demand
during `download` / `save_to`.

`Store` trait additions:

```rust
async fn insert_attachment(
    &self,
    record: &AttachmentRecord,
) -> Result<(), KupferdrahtError>;

async fn get_attachment(
    &self,
    message_id: &str,
    room_id: &str,
) -> Result<Option<AttachmentRecord>, KupferdrahtError>;

async fn list_attachments_for_room(
    &self,
    room_id: &str,
) -> Result<Vec<AttachmentRecord>, KupferdrahtError>;

async fn update_attachment_cache_path(
    &self,
    message_id: &str,
    room_id: &str,
    path: Option<&str>,
) -> Result<(), KupferdrahtError>;

async fn delete_attachment(
    &self,
    message_id: &str,
    room_id: &str,
) -> Result<(), KupferdrahtError>;
```

Plus a new `AttachmentRecord` struct in
`storage/records.rs` carrying every column.

TDD checklist:
- Round-trip: insert → get → list → delete.
- `otr_key_ct` is properly DEK-encrypted at rest (raw column read
  is ciphertext, not the supplied plaintext key).
- `update_attachment_cache_path` sets and clears the path.
- FK cascade: deleting the parent timeline_events row blocks the
  attachments delete (ON DELETE NO ACTION default; live behavior
  matches the existing 6-II pattern).

#### 11e — Step-10 learnings carryover (pre-implementation absorption)

Pure docs sub-step inserted between 11d and the orchestration
work (11f/11g/11h) so live-learnings from Step 10 surface in the
right places before they get re-discovered as live-smoke bugs.

Step 10 (server feature configs, ADR-0012) shipped five
observations that ADR-0011 did not (or only partially) anticipate.
Each carryover item below pins where the learning gets absorbed
in the renumbered 11f/11g/11h sub-steps. The absorption happens
inline in those sections (as bullet additions); this section is
the index so a reviewer landing on a single learning can find
its application point without scanning the whole plan-doc.

**A. CLI room-resolution sugar — reuse for `download-asset`.**
ADR-0011 §CLI commands shows `download-asset --message <id>`
without specifying whether `--id <room>` is required. The
`chore/cli-room-resolution-message-commands` cross-cut (merged
`859aafb`) makes `--id` optional on the five
message-scoped commands (`mark-read`, `react`, `delete-message`,
`edit`, `reply`) by auto-resolving from `--message` against
the local `timeline_events` table.
→ 11h reuses `resolve_room_for_message` for `download-asset`.

**B. UDL surface deferred — same posture as 10e.**
ADR-0011 §Public API ships a full UDL block. `kupferdraht.udl` is
projectwide empty (`namespace kupferdraht {};`) — Step 10e
documented this as "workspace-wide UDL pass after the Rust
public API stabilises", and Step 11 inherits that posture.
→ 11f/11g/11h add only Rust surface; no UDL changes.

**C. `EnabledAll` is the dominant `fileSharing` case in
production.** ADR-0011 §Public API examples show
`FileSharingPolicy::EnabledSome(["image/*", ...])`. Live-smoke
2026-05-08 against Volker's Wire production team observed
`fileSharing: { status: "enabled", config: {} }` — empty
config → `EnabledAll`. Step 10e's parser handles this branch
correctly; Step 11's policy gate must treat `EnabledAll` as the
common path, not the edge.
→ 11f's `file_sharing_check` tests put `EnabledAll` first,
`Disabled` and `EnabledSome` as edge cases.

**D. Inbound live-smoke needs a Wire-Web counterpart sender.**
Step 10f's off-path gate (toggle `WIRE_RECEIPT_MODE` via
Wire-Web, observe no receipt emission) was skipped during
live-smoke and covered only by integration test — accepted
trade-off because the gate is one line of code. Step 11g's
inbound asset dispatch CANNOT be skipped the same way:
download + SHA verify + AES-CBC decrypt is multi-stage and
needs a real Wire-Web-emitted asset to validate end-to-end.
→ 11h live-smoke plan flags "counterpart sender required"
as a pre-flight task, gating the live-smoke run.

**E. Pre-smoke check: Wire-Web still emits `AES_CBC`.**
ADR-0011 §Crypto construction pins AES-CBC as Wire's current
deployed format. Step 10c's "10–15 entries" was off by 3x
against live production (33 entries observed) — ADR claims
about external systems decay quietly. Before 11h's outbound
live-smoke, a quick check that Wire-Web still ships
`Asset.RemoteData.encryption = AES_CBC` (DevTools → Network
→ inspect a Wire-Web upload) catches the scenario where
Wire silently switched to AES_GCM during the implementation
window.
→ 11h pre-smoke checklist includes the DevTools verification
step.

This sub-step ships as a single docs-only commit on
`step/11e-step10-learnings-carryover`. No code changes; PLAN.md
row goes `committed` immediately after merge (no live-smoke
needed for docs).

#### 11f — `Timeline::send_file` outbound

(Renamed from `11e` on 2026-05-11 — see §Sub-step renaming
history at the top.)

**Step-10 carryover (§11e item C):** `EnabledAll` is the dominant
`fileSharing` case in Wire production, not `EnabledSome`. Test
fixtures for the policy gate put `EnabledAll` first as the
common path; `Disabled` and `EnabledSome(allowed)` are edge cases.

Orchestrator that wires 11a + 11b + 11c + 11d into a single async
call. Layout:

```
send_file(path, mime_type):
  1. file_sharing_check(mime_type)   // 10e gate
  2. encrypt_file(path)              // 11b → EncryptedAsset
  3. compute_md5_b64(ciphertext)     // for Content-MD5 header
  4. http.upload_asset(...)          // 11c → AssetUploadResponse
  5. build GenericMessage::Asset { Original + RemoteData }
  6. mls.encrypt_text_message(generic_msg)  // existing path
  7. http.send_mls_message(ciphertext)
  8. store.insert_attachment(record) + insert_timeline_event(...)
  9. tempfile cleanup (cache_path, _ if persistence is enabled_)
  10. return MessageHandle
```

Cache policy: the encrypted ciphertext tempfile is moved into
`~/.kupferdraht/<account_id>/assets/<message_id>.enc` after a successful
send so a future `download` from the SAME device hits the local
cache. The `cache_path` column tracks this; eviction is LRU-
bounded by the cap defined in §Implementation notes.

`Client::file_sharing_check(mime_type)` is a private helper that
projects `Client::file_sharing()` against the requested mime:

```rust
fn file_sharing_check(&self, mime_type: &str) -> Result<(), KupferdrahtError> {
    match self.file_sharing().await? {
        FileSharingPolicy::Disabled => Err(KupferdrahtError::InvalidInput(
            "file_sharing disabled by team policy".into(),
        )),
        FileSharingPolicy::EnabledSome(allowed) if !mime_matches(&allowed, mime_type) => {
            Err(KupferdrahtError::InvalidInput(format!(
                "mime_type {mime_type} not allowed by team policy (allowed: {})",
                allowed.join(", "),
            )))
        }
        _ => Ok(()),
    }
}
```

`mime_matches` handles glob patterns (`image/*` matches `image/png`).

TDD checklist (httpmock-driven):
- Happy path: send a 64 KB file → MLS message lands, attachment
  row persisted, cache_path populated.
- `Disabled` policy: `send_file` errors before any Wire call.
- `EnabledSome(["image/*"])` + `application/pdf` mime: rejected
  pre-flight, no Wire call.
- `EnabledSome(["image/*"])` + `image/png` mime: accepted.
- Upload 5xx: error surfaces, no attachment row persisted.

#### 11g — Inbound asset dispatch

(Renamed from `11f` on 2026-05-11 — see §Sub-step renaming
history at the top.)

**Step-10 carryover (§11e item B):** UDL surface deferred. The
new `MessageBody::Asset(AssetSummary)` and
`TimelineItemKind::Asset(AssetSummary)` ship only on the Rust
surface; no `kupferdraht.udl` changes.

Add `dispatch_inbound_asset` to `sync/dispatch.rs`. Wire's
`GenericMessage::Content::Asset` arrives via the same MLS
decrypt path as `Text` — what changes is the variant handling.

```rust
pub(super) async fn dispatch_inbound_asset(
    &self,
    client: &Arc<Client>,
    room_id: &str,
    sender_user_id: &str,
    sender_device_id: Option<&str>,
    message_id: &str,
    asset: crate::wire::proto::Asset,
) -> Result<(), KupferdrahtError> {
    // 1. Reject AES_GCM (warn-and-skip per ADR-0011).
    // 2. Extract Original + RemoteData fields; persist
    //    `attachments` row + `timeline_events` row.
    // 3. Emit `MessageBody::Asset(AssetSummary)` to handlers.
    // 4. Emit `TimelineDiff::Append { kind: Asset(...) }`.
}
```

`MessageBody::Asset(AssetSummary)` and
`TimelineItemKind::Asset(AssetSummary)` are the new public-facing
shapes consumers see. The inbound row's `cache_path` is `NULL` by
default — the encrypted ciphertext is downloaded on-demand by
`AssetSummary::download` / `save_to`, populating the cache then.

`Asset.NotUploaded` events are discarded silently (the sender's
state never resolved). `expects_read_confirmation = true` is
honoured by the existing 6-II-f read-receipt machinery — no new
code path.

TDD checklist (synthetic events):
- Inbound `Asset` with `AES_CBC` → row persisted, summary emitted.
- Inbound `Asset` with `AES_GCM` → "unsupported asset" placeholder
  in timeline, NO attachment row persisted (decrypt would fail
  later anyway), drain continues.
- Inbound `Asset.NotUploaded` → drain continues silently, no row
  persisted, no callback.

#### 11h — CLI + live-smoke + scope amendments

(Renamed from `11g` on 2026-05-11 — see §Sub-step renaming
history at the top.)

**Step-10 carryover:**
- **§11e item A** — `download-asset` reuses the
  `resolve_room_for_message` helper from
  `chore/cli-room-resolution-message-commands` (merged
  `859aafb`). `--id <room>` is optional; auto-resolved from
  `--message <id>` against `timeline_events`.
- **§11e item D** — inbound live-smoke gate requires a real
  Wire-Web counterpart sender (not skippable). Pre-flight
  task before the live-smoke run starts.
- **§11e item E** — pre-smoke check: open Wire-Web DevTools →
  Network → inspect a Wire-Web upload's MLS payload to verify
  `Asset.RemoteData.encryption = AES_CBC` is still emitted.
  Catches the scenario where Wire silently switched to AES_GCM
  during Step 11's implementation window.

CLI subcommands:

| Command            | Purpose |
|---|---|
| `send-file --id <room>@<domain> --path <p> --mime <m>` | Encrypt + upload + MLS-send a file. Future room-resolution sugar could auto-resolve `--id` from the active conversation context, but V1 keeps it required (no parent message id to look up against). |
| `download-asset --message <id> [--id <room>] [--save-to <p>]` | Download + decrypt an inbound asset. `--id` optional — see §11e item A. Without `--save-to`, prints metadata only. |

**Pre-smoke checklist (per §11e items D + E):**
1. Confirm counterpart sender available — second Wire account
   that can emit an asset from Wire-Web into a shared room.
2. Wire-Web upload-format check: send any file from Wire-Web,
   open DevTools → Network → MLS message body → confirm
   `Asset.RemoteData.encryption` is `0` (AES_CBC). If it shows
   `1` (AES_GCM), Wire's deployed format has changed; stop and
   open a follow-up ADR before proceeding.

Live-smoke against Wire production:
1. **Outbound:** Send a 1 MB image to a counterpart account; verify
   it renders in Wire-Web (mime preserved, filename preserved,
   image displays). The MLS message is identical-shape to existing
   text messages — just with `Asset` content.
2. **Inbound:** Have the counterpart (Wire-Web) send a small file
   back; `sync-fetch` drains the event; `download-asset` decrypts
   and saves to disk; `sha256` matches; plaintext byte-identical
   to the original.
3. **Policy gate:** Toggle `fileSharing` to `disabled` in Wire-Web's
   team settings (admin-only — skip if the test account isn't admin),
   re-run `send-file`, verify rejection without Wire call.
4. **AES-GCM tolerance:** synthetic test only — no Wire client
   emits GCM today.

Scope amendments (CLAUDE.md + PLAN.md):

```diff
-**In scope: Wire's full text-messaging feature set, plus
- server-administered configuration awareness.**
+**In scope: Wire's full messaging feature set (text + assets),
+ exclusive of calling and previews.**

-- File transfers, image / audio / video attachments
+- (removed; assets are in scope as of Step 11)
+- Voice / video / calling — no symbol from any calling library
+- Asset previews (`Asset.Preview` upload cycle, deferred follow-up)
```

`docs/THREAT_MODEL.md` gains the "Asset content authenticity"
addendum from ADR-0011 §Threat model addendum, verbatim.

### Store additions

V19 migration creates `attachments`. `AttachmentRecord` struct in
`storage/records.rs`. Five new `Store` trait methods (insert /
get / list / update_cache_path / delete) — see 11d.

### Implementation notes

- **Cache directory.** Default
  `~/.kupferdraht/<account_id>/assets/`. Permissions `0700` on Unix
  (matches the existing `~/.kupferdraht/` posture). `cache_path` is an
  absolute path so a `KUPFERDRAHT_HOME` change (rare) doesn't strand
  cached blobs.

- **Cache cap (LRU).** V1 ships a soft 1 GB cap per account; the
  encrypt path's tempfile-move-into-cache step checks the
  directory size and evicts the oldest file when the new one
  would exceed the cap. Cap is hard-coded; making it configurable
  is a follow-up. The cache holds only *encrypted* ciphertext —
  re-decryption on every consumer access. Plaintext is never
  cached.

- **Tempfile location during encrypt.** `tempfile::tempfile_in()`
  rooted at the OS-level user cache directory (`dirs::cache_dir`),
  not `/tmp`. Reason: `/tmp` is often `tmpfs` (RAM-backed); a
  100 MB file would inflate process RSS. The user cache is
  disk-backed on every supported platform.

- **`Content-MD5` header.** Wire's backend validates the upload
  body's MD5; the header is required, not optional. `md-5` 0.10
  computes it streamingly over the ciphertext tempfile in the same
  pass as the SHA-256 (one disk read for both digests).

- **Mime-glob matching for `EnabledSome`.** Kupferdraht implements a
  minimal glob matcher: `image/*` matches `image/<anything>`,
  `*` matches anything, exact strings match exactly. No regex,
  no full RFC 6838 parameter parsing — the full grammar is
  out-of-scope for the policy gate.

- **Asset preview deferral.** `Asset.Preview` (a separate
  upload cycle for thumbnails) is decoded into the `Asset`
  protobuf but its `RemoteData` is ignored. UI consumers see only
  the `Original` metadata. The follow-up step ("Step 11-II" if it
  materialises) wires preview download.

- **AES-GCM emission deferral.** Kupferdraht emits only `AES_CBC` until
  Wire's clients standardise GCM byte layout. Receive-side
  warn-and-skip is documented in 11g's TDD checklist.

- **Federation.** Kupferdraht assumes `asset_domain` resolves through
  the same Wire federation graph as `conversation_domain`. Wire's
  federator handles cross-domain GETs server-side; no client work
  required.

- **Storage migration ordering.** The `attachments` table's
  foreign key references `timeline_events(message_id, room_id)`
  — V19 must run after V6 (timeline_events creation) and V9 (FK
  cascade fix). The migration sequence is monotonic so this
  ordering is automatic.

### Dependencies

Three new direct dependencies, all permissive-licensed:

- `aes` 0.8 (Apache-2.0 / MIT) — AES-256 block cipher.
- `cbc` 0.1 (Apache-2.0 / MIT) — CBC mode of operation.
- `md-5` 0.10 (Apache-2.0 / MIT) — Content-MD5 transport header.

`sha2` is reused from the existing graph (Step 6-II-b's quote
digest). `cipher` and `block-padding` come transitively from
`cbc`. No new git-source dependencies; all from crates.io.

`cargo deny check` passes without `deny.toml` changes — every
new crate falls under the existing permissive allow-list.

### TDD starter tests

Cross-cutting integration tests:

- `kupferdraht-core/tests/step_11_kalium_kat.rs` — Known-Answer Vectors
  from Kalium's `AESUtilsTest.kt` round-trip Kupferdraht's encrypt to
  byte-identical ciphertext.
- `kupferdraht-core/tests/step_11_asset_round_trip.rs` —
  encrypt → upload (mock) → download (mock) → decrypt → assert
  plaintext equality, for sizes [1 byte, 64 KB, 1 MB, 50 MB].
- `kupferdraht-core/tests/step_11_inbound_dispatch.rs` — synthetic
  inbound `Asset` events round-trip into `attachments` +
  `timeline_events` rows; AES-GCM-flagged events become
  "unsupported asset" placeholders.
- `kupferdraht-core/tests/step_11_send_file_policy_gate.rs` —
  `Client::file_sharing()` projection rejects mime types
  outside the team's allow-list before any Wire call.

### Dependencies (predecessor steps)

- All earlier Steps (1–10) merged.
- Step 7's per-field DEK pattern for `otr_key_ct`.
- Step 10e's typed `Client::file_sharing()` getter for the
  policy gate.
- Step 6-II-a's Timeline read API for `TimelineItemKind::Asset`.
- Step 5d's polling drain for inbound `Asset` event delivery.

### Validation gates

This step is "complete" when:

1. All sub-step TDD lists are green.
2. Live-smoke 1 + 2 above pass against Wire production
   (outbound 1 MB image renders in Wire-Web; inbound Wire-Web file
   decrypts byte-identical).
3. Live-smoke 3 (policy gate) passes (Wire-Web admin toggle
   skipped if test account isn't admin — synthetic integration
   test substitutes).
4. `docs/library.md` carries the `Timeline::send_file` /
   `AssetSummary` surface.
5. `docs/cli.md` carries `send-file` / `download-asset` entries.
6. `docs/THREAT_MODEL.md` carries the asset-content-authenticity
   addendum.
7. CLAUDE.md §Functional Scope is amended per 11h.
8. `docs/api-mapping-matrix-rust-sdk.md` carries the
   `Timeline::send_file` drift entry.

### CLI commands

| Command | Options |
|---|---|
| `send-file` | `--id <room>` (optional, auto-resolved via Step 10c sugar — for `send-file` the resolution path is `--id` only since no parent message id is available); `--path <p>`; `--mime <m>` |
| `download-asset` | `--message <id>`; `--save-to <p>` (optional — without it, prints metadata only) |
