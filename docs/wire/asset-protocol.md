# Wire Asset Protocol — Observation Record

How file attachments ("assets") travel between Wire clients and the
Wire backend, as observed for ADR-0011 and Step 11. The observations
come from Kalium's public source, read as a description of the Wire
protocol (ADR-0009: protocol observation only), and were cross-checked
live against a Wire v14 backend during Step 11 (`docs/observations.md`,
`docs/backend-spec.md`). Where the observation left a question open,
the section says so.

Survey date: 2026-05-05. Kalium revision
[`9ccecabb`](https://github.com/wireapp/kalium/tree/9ccecabb04742ec0afa30babe9925bea7313a17c);
every source link below is a permalink into it. Kalium is GPL-3.0: the
links name the observation point, the text describes the behaviour,
and no code is reproduced.

---

## Q1 — Cipher mode

**Finding.** New assets are encrypted with **AES-256-CBC, PKCS#5/7
padding, no HMAC**. The outbound path selects `AES_CBC` unconditionally.

The `EncryptionAlgorithm` enum of the wire format has two values,
`AES_CBC` (0) and `AES_GCM` (1), so the format is mode-tagged and a
receiver has to be prepared for both. Kalium's non-test code never
selects `AES_GCM` when sending; the value appears only in test
fixtures that exercise the receive path with externally produced
ciphertext.

There is no CBC-plus-HMAC construction: plain CBC ciphertext is
uploaded, and integrity travels out of band in
`Asset.RemoteData.sha256` (Q3). Kupferdraht does the same because
interoperability requires it (ADR-0011 §Crypto construction).

**Observation points.**

- [`AESUtils.kt:172`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/core/cryptography/src/commonJvmAndroid/kotlin/com.wire.kalium.cryptography/utils/AESUtils.kt#L172)
  — the cipher transformation: AES in CBC mode with PKCS5 padding;
  the file wraps nothing in a MAC.
- [`AESUtils.kt:175`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/core/cryptography/src/commonJvmAndroid/kotlin/com.wire.kalium.cryptography/utils/AESUtils.kt#L175)
  — the key size: 256 bits.
- [`UploadAssetMessageMetadata.kt:122`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/feature/asset/upload/UploadAssetMessageMetadata.kt#L122)
  — the outbound metadata hard-codes the CBC mode; no code path
  selects GCM for outbound content.
- [`MessageEncryptionAlgorithm.kt:21`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/core/data/src/commonMain/kotlin/com/wire/kalium/logic/data/message/MessageEncryptionAlgorithm.kt#L21)
  — the local two-valued enum mirroring the schema's.
- [`messages.proto:398`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/tools/protobuf-codegen/src/main/proto/messages.proto#L398)
  — the schema's `EncryptionAlgorithm` enum.

---

## Q2 — Key material layout (`otr_key`, IV)

**Finding.** `Asset.RemoteData.otr_key` is **exactly the 32-byte raw
AES-256 key**. Nothing is concatenated; there is no separate MAC key.
The IV is **16 random bytes prepended to the ciphertext**: the blob
the upload endpoint receives is `iv(16) || aes-cbc-pkcs5(plaintext)`.

The IV travels neither in `otr_key` nor in a field of its own; it is
the head of the encrypted blob. The receiver reads the first 16 bytes
as the IV and decrypts the remainder.

**Observation points.**

- [`AESUtils.kt:103-108`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/core/cryptography/src/commonJvmAndroid/kotlin/com.wire.kalium.cryptography/utils/AESUtils.kt#L103-L108)
  — key generation: a fresh 256-bit AES key whose raw bytes are the
  whole key material.
- [`AESUtils.kt:49-50, 56-58`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/core/cryptography/src/commonJvmAndroid/kotlin/com.wire.kalium.cryptography/utils/AESUtils.kt#L49-L50)
  — a random 16-byte IV, written to the output before the cipher
  stream starts.
- [`AESUtils.kt:99-100`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/core/cryptography/src/commonJvmAndroid/kotlin/com.wire.kalium.cryptography/utils/AESUtils.kt#L99-L100)
  — the in-memory variant returns the IV followed by the ciphertext.
- [`AESUtils.kt:124-129, 161-167`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/core/cryptography/src/commonJvmAndroid/kotlin/com.wire.kalium.cryptography/utils/AESUtils.kt#L124-L129)
  — the decoder takes the leading IV-sized prefix as the IV and
  decrypts the rest.
- [`UploadAssetMessageMetadata.kt:91, 119`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/feature/asset/upload/UploadAssetMessageMetadata.kt#L91)
  — the outbound path puts the generated key's bytes, and nothing
  else, into `otr_key`.
- [`AssetContent.kt:77`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/core/data/src/commonMain/kotlin/com/wire/kalium/logic/data/message/AssetContent.kt#L77)
  — the local model carries the key as a bare byte array.

---

## Q3 — `sha256` covers the ciphertext, IV included

**Finding.** `Asset.RemoteData.sha256` is the SHA-256 of the
**complete uploaded blob**, the 16-byte IV plus the padded CBC
ciphertext, computed after encryption. It is the only integrity check
on an asset: CBC authenticates nothing, so this hash is what lets a
receiver detect a tampered blob. There is no MAC step.

**Observation points.**

- [`AssetRepository.kt:191-200`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/data/asset/AssetRepository.kt#L191-L200)
  — upload: the file is encrypted to a temporary path first, and the
  hash is computed over that encrypted file, not the plaintext.
- [`AssetRepository.kt:417-430`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/data/asset/AssetRepository.kt#L417-L430)
  — download: the hash of the still-encrypted download is compared
  with the value from the message before decryption.
- [`CryptoUtils.kt:55-77`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/core/cryptography/src/commonMain/kotlin/com/wire/kalium/cryptography/utils/CryptoUtils.kt#L55-L77)
  — the hash helpers are plain, unkeyed SHA-256.

---

## Q4 — Schema

Wire publishes the `Asset` message — its nested `Original`,
`Preview`, `ImageMetaData`, `VideoMetaData`, `AudioMetaData` and
`RemoteData` types and the `NotUploaded` enum — together with the
top-level `EncryptionAlgorithm` enum in
[`messages.proto:269-401`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/tools/protobuf-codegen/src/main/proto/messages.proto#L269-L401)
(GPL-3.0; the same schema is published standalone as
[`wireapp/generic-message-proto`](https://github.com/wireapp/generic-message-proto)).
The schema is not reproduced here. The subset Kupferdraht emits or
decodes is a modified copy of these declarations in
[`kupferdraht-core/proto/generic_message.proto`](../../kupferdraht-core/proto/generic_message.proto)
(`message Asset`), with Wire's notice kept in the file's header and
Kupferdraht's own comments; field names and numbers are identical because
interoperability dictates them.

Notes:

- The schema's comment on `Asset.RemoteData.sha256` calls the field
  obsolete and kept for backward compatibility. In practice it is not
  obsolete: the receive path rejects an asset whose downloaded
  ciphertext does not hash to this value (`AssetRepository.kt:417-430`
  above).
- `Asset.preview` (field 5) is the current preview pointer; the earlier
  preview field number is retired.
- The `Asset.NotUploaded` enum exists in the schema but is never
  emitted on the outbound path (Q5).

---

## Q5 — Lifecycle: which phases cross the wire, which stay local

**Finding.** Two parallel state machines:

1. **Wire-visible status (`Asset.status` oneof).** Two variants exist:
   `NotUploaded(CANCELLED|FAILED)` and `RemoteData uploaded`. Kalium
   **only ever sends the uploaded variant.** Its receive code accepts
   `NotUploaded` and maps it to empty remote data, but its outbound
   mapper has a single branch. On the wire you only see complete
   uploads.

2. **Local transfer state.** A nine-value enum drives the UI through
   the upload and download lifecycles (in progress, failed, done, not
   yet fetched, saved, not found, on both sides). None of these cross
   the MLS wire; they live beside the message row in the local
   database. The sender's "still encrypting / uploading" placeholder
   is a local row. Once the upload returns the asset id and the hash,
   the local status becomes uploaded, the full content including
   `RemoteData` is persisted, and only then is the message handed to
   the sender. The MLS message therefore always carries the uploaded
   status; the receiver never sees an in-progress state.

**Observation points.**

- [`messages.proto:312-332`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/tools/protobuf-codegen/src/main/proto/messages.proto#L312-L332)
  — the schema's `NotUploaded` enum and `status` oneof.
- [`AssetMapper.kt:204-247`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/data/asset/AssetMapper.kt#L204-L247)
  — the outbound mapper always produces the uploaded status; no path
  produces `NotUploaded`.
- [`AssetMapper.kt:182-199`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/data/asset/AssetMapper.kt#L182-L199)
  — inbound `NotUploaded` becomes an empty remote-data record,
  treated as preview-only content rather than an error.
- [`AssetMessage.kt:36-83`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/core/data/src/commonMain/kotlin/com/wire/kalium/logic/data/asset/AssetMessage.kt#L36-L83)
  — the local transfer-status enum.
- [`UploadAssetUseCase.kt:67, 80, 83, 108`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/feature/asset/upload/UploadAssetUseCase.kt#L67)
  — the order of operations: mark in progress, upload, mark uploaded,
  persist the full content, and only then send. The MLS dispatch is
  always after the upload has settled.

---

## Q6 — HTTP routes, body, response, asset token

### Endpoints (per API version)

Two route shapes exist in Kalium's tree, selected by the negotiated
Wire API version:

| API version          | Upload                  | Download                          | Delete                             | Source |
|----------------------|-------------------------|-----------------------------------|------------------------------------|--------|
| **v0** (legacy)      | `POST /assets/v3`       | `GET /assets/v3/{id}` (unfederated) **or** `GET /assets/v4/{domain}/{id}` (federated) | same path with `DELETE` | `data/network/.../v0/.../AssetApiV0.kt:113-117, 130, 145, 151-152` |
| **v2 and later** (Wire API v14 is on this shape) | `POST /assets`           | `GET /assets/{domain}/{id}` (federated form only; an absent domain falls back to the caller's own) | same path with `DELETE`     | `data/network/.../v2/.../AssetApiV2.kt:39-46, 50-67, 70` |
| **v12 and later** (audit log) | unchanged path; the metadata JSON gains `convId`, `filename`, `filetype` | unchanged | unchanged | `data/network/.../v12/.../AssetApiV12.kt:31-48` |

The versions between inherit from v2, v11 or v12 without further
endpoint changes; only the upload metadata evolves at v12.

### Upload request body (multipart/mixed)

The body is a hand-built `multipart/mixed` with the boundary
`frontier`, two parts:

```
POST /assets HTTP/1.1
Content-Type: multipart/mixed; boundary=frontier

--frontier
Content-Type: application/json;charset=utf-8
Content-Length: <len(json)>

{
  "public": false,
  "retention": "persistent"
  // v12+ adds: "convId": {"id":"…","domain":"…"}, "filename":"…", "filetype":"…"
}

--frontier
Content-Type: application/octet-stream
Content-Length: <encryptedDataSize>
Content-MD5: <base64(md5(ciphertext))>

<raw ciphertext bytes — IV(16) || AES-CBC-PKCS5(plaintext)>

--frontier--
```

`public` is a boolean; `retention` is one of `eternal`, `persistent`,
`volatile`, `eternal_infrequent_access`, `expiring`. For private
(end-to-end encrypted) assets the values are `public=false`,
`retention=persistent` (`AssetRepository.kt:227`).

The `Content-MD5` header on the second part is the base64 of the MD5
over the ciphertext stream (`CryptoUtils.kt:42-53`). It is a
transport-level check Wire's backend applies; the SHA-256 in the
message is the end-to-end check.

**Observation points.**

- [`AssetApiV0.kt:124-141, 157-194`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/data/network/src/commonMain/kotlin/com/wire/kalium/network/api/v0/authenticated/AssetApiV0.kt#L124-L141)
  — the multipart body streamed by hand with the `frontier` boundary.
- [`AssetApiV0.kt:119-122`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/data/network/src/commonMain/kotlin/com/wire/kalium/network/api/v0/authenticated/AssetApiV0.kt#L119-L122)
  — the v0 metadata JSON: `public` and `retention` only.
- [`AssetApiV12.kt:31-48`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/data/network/src/commonMain/kotlin/com/wire/kalium/network/api/v12/authenticated/AssetApiV12.kt#L31-L48)
  — the v12 metadata JSON adds `convId`, `filename`, `filetype`.
- [`AssetRetentionType.kt:25-40`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/data/network-model/src/commonMain/kotlin/com/wire/kalium/network/api/model/AssetRetentionType.kt#L25-L40)
  — the retention values and their serialised names.

### Upload response (JSON)

```json
{
  "key":     "<asset-id-uuid>",
  "domain":  "<wire-domain>",
  "expires": "<rfc-3339-timestamp-or-null>",
  "token":   "<asset-token-or-null>"
}
```

`token` is the per-asset access token Wire requires on download for
private assets (below). `expires` is set for retentions other than
`persistent`; for the asset-as-content path it is typically `null`.

**Observation point.**

- [`AssetResponse.kt:24-30`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/data/network-model/src/commonMain/kotlin/com/wire/kalium/network/api/authenticated/asset/AssetResponse.kt#L24-L30).

### Download response

Raw bytes, the still-encrypted ciphertext, streamed in the response
body. Beyond the status code the only header Kalium looks at is a
logged `WWW-Authenticate` on `401`. Two authentication mechanisms are
layered:

- the standard `Bearer` access token on the request;
- the optional **`Asset-Token: <token>` header** for private assets,
  sent only when the token is non-empty — an empty string counts as
  "no token".

**Observation points.**

- [`AssetApiV0.kt:62-89, 92-107`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/data/network/src/commonMain/kotlin/com/wire/kalium/network/api/v0/authenticated/AssetApiV0.kt#L62-L89)
  — the download streams the response body into a caller-provided
  sink.
- [`AssetApiV0.kt:69, 153`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/data/network/src/commonMain/kotlin/com/wire/kalium/network/api/v0/authenticated/AssetApiV0.kt#L69)
  — the `Asset-Token` header name and its conditional setter.
- [`AssetRepository.kt:347`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/data/asset/AssetRepository.kt#L347)
  — the caller normalises an empty token to "none" before the call.

---

## Q7 — Size limit (client-side vs server-side)

**Finding, with caveat.** Kalium enforces **no client-side size limit
on assets**: its tree holds no asset-size constant, and its
file-sharing feature model is a three-state value (`Disabled`,
`EnabledAll`, `EnabledSome(types)`;
`logic/.../FileSharingStatus.kt:21-30`) whose allow-list of file
extensions is a build-time setting of Kalium, not a byte cap and not
server data. The server's feature config exposes a boolean
file-sharing flag and no size (`core/data/.../FeatureConfigModel.kt:34`).

*Note (2026-10-20):* Kupferdraht's `FileSharingPolicy` had taken over
that three-state shape, reading an allow-list from a
`config.allowedTypes` field no Wire backend defines. The provenance
audit after counsel's opinion identified this as the one place where a
reading of Kalium shaped a Kupferdraht type rather than a protocol
observation; the enum is `Disabled | Enabled` since ADR-0012 revision
1.3 (`docs/wire/kalium-readings.md`).

The effective cap is therefore **server-side** at the asset endpoint;
its value is not visible from the client source.

**Status at survey time: unverified.** The two ways to settle it: the
backend operator's configured cap, or a live probe with growing assets
that records the failure mode (HTTP status and label).

---

## Q8 — Federation references

**Finding.** Three fields on `Asset.RemoteData` together identify an
asset on any backend:

- `asset_id` (string, optional) — the per-asset UUID the upload
  returned as `key`;
- `asset_domain` (string, optional) — the originating backend's
  fully-qualified domain (`example.com`, a
  federated peer's domain);
- `asset_token` (string, optional) — the per-asset access token the
  upload returned as `token`; required for private assets, `null` for
  public ones.

The download route the client builds is:

```
GET /assets/{asset_domain}/{asset_id}        Authorization: Bearer <jwt>
                                             Asset-Token:   <asset_token>   (when set)
```

If `asset_domain` is empty or absent, Kalium falls back to the
**caller's own domain**, i.e. it assumes the asset is local to the
redeemer. Federation works because `asset_domain` is always carried
in the message and Wire's backend serves cross-backend fetches behind
the same URL shape through its federator.

The legacy v0 fallback (only against a pre-v2 backend) routes
unfederated assets via `GET /assets/v3/{id}` and federated ones via
`GET /assets/v4/{domain}/{id}`; Kupferdraht targets Wire API v14 and
uses the `assets/{domain}/{id}` shape only.

**Observation points.**

- [`messages.proto:317-325`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/tools/protobuf-codegen/src/main/proto/messages.proto#L317-L325)
  — the schema's `RemoteData` with `asset_id`, `asset_token`,
  `asset_domain`.
- [`AssetApiV2.kt:39-46`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/data/network/src/commonMain/kotlin/com/wire/kalium/network/api/v2/authenticated/AssetApiV2.kt#L39-L46)
  — the path builder: assets, domain, id; an absent domain falls
  back to the caller's own.
- [`AssetApiV0.kt:113-117`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/data/network/src/commonMain/kotlin/com/wire/kalium/network/api/v0/authenticated/AssetApiV0.kt#L113-L117)
  — the legacy fork between the v3 and v4 routes on whether a domain
  is present.

---

## Open items / follow-ups

- **AES-GCM receive-path coverage.** Kalium's outbound code hard-codes
  `AES_CBC`. The `AES_GCM` value exists in the schema and Kalium's
  tests exercise the receive path, but the snapshot shows no non-test
  implementation that decrypts GCM asset content. Either the value is
  reserved but unused, or the implementation lives in a client
  repository Kalium has not merged. To be verified against
  `wire-android` and `wire-desktop` if it ever matters.
- **AES-GCM IV and tag layout.** Should Kupferdraht ever need to
  *send* `AES_GCM`, the snapshot does not show what byte layout
  receivers expect (IV position, tag position, key shape). Reading
  the client repositories is the first step before scoping it.
- **Server-side asset size cap.** Per Q7 — the number comes from the
  backend operator or from live testing.
- **Preview asset path.** `Asset.Preview` carries its own `RemoteData`
  and is encrypted under its own key and hash. Kalium runs it through
  the same repository surface with a separate upload and download
  cycle. When to generate previews, mime-type rules and thumbnail
  dimensions were out of Kupferdraht's first scope; if previews ship,
  they deserve their own ADR section.
- **Asset deletion vs retention.** `DELETE /assets/{domain}/{id}` is
  exposed and Kalium calls it (`AssetRepository.kt:448-450`). Whether
  `persistent` assets are reaped on conversation deletion or stay
  forever is server-side policy, not visible from the client.
