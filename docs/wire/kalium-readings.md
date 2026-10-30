# Kalium readings

Every place where Wire's Kalium source was read while Kupferdraht was
built, with its source, what the reading informed and what Kupferdraht
did with it. This is the index the README's provenance rule promises;
the readings themselves are recorded where they happened
(`docs/observations.md`, `docs/wire/api-notes.md`,
`docs/wire/asset-protocol.md`, the ADRs).

**Revision.** All readings are of `wireapp/kalium` at
`9ccecabb04742ec0afa30babe9925bea7313a17c` (2026-04-03), the one
revision that sat in the development tree as a git submodule from
2026-04-08 to 2026-10-07; the submodule was removed before publication.
Every path below links to that revision.

**The rule.** Kupferdraht is written from Wire's v14 OpenAPI document,
RFC 9420 and live observation of wire-server deployments. Kalium is read
to observe protocol behaviour where the spec is silent or an
interoperability failure needs explaining, never as a model for
structure, naming or implementation (ADR-0009). The outcome column says
what each reading led to:

- *protocol* — what was read is dictated by the server, the schema or
  the RFC; Kupferdraht implements the same wire behaviour in its own code.
- *divergent* — Kupferdraht chose a different design after the reading.
- *convention* — a value every Wire client uses, not a Kalium choice.
- *replaced* — something taken over was later replaced (details below).

## Readings

| # | Kalium source | What was read | Recorded in | Outcome |
|---|---|---|---|---|
| 1 | [AccessTokenApiV0.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/data/network/src/commonMain/kotlin/com/wire/kalium/network/api/v0/authenticated/AccessTokenApiV0.kt) (and V3–V15) | `POST /access` shape across API versions | api-notes.md, `POST /access` (v5 → v14 note) | protocol |
| 2 | [UserDetailsApiV0.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/data/network/src/commonMain/kotlin/com/wire/kalium/network/api/v0/authenticated/UserDetailsApiV0.kt) | user-details endpoint shape | api-notes.md, `/users` (v5 → v14 note) | protocol |
| 3 | [ConversationApiV5.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/data/network/src/commonMain/kotlin/com/wire/kalium/network/api/v5/authenticated/ConversationApiV5.kt), [ConversationApiV7.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/data/network/src/commonMain/kotlin/com/wire/kalium/network/api/v7/authenticated/ConversationApiV7.kt) | conversation endpoints; the one-to-one conversation is a single `GET` from v7 (confirmed after the documented `POST` returned 404 live) | api-notes.md, conversations and Step 14a | protocol |
| 4 | [NotificationApiV0.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/data/network/src/commonMain/kotlin/com/wire/kalium/network/api/v0/authenticated/NotificationApiV0.kt), [NotificationApiV9.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/data/network/src/commonMain/kotlin/com/wire/kalium/network/api/v9/authenticated/NotificationApiV9.kt) | notification paging, the legacy `/await` socket, the v9+ `/events` socket with `client` and `sync_marker`, the `/cookies` precondition | observations.md (Step 5 entries), LEARNINGS.md §1, CHANGELOG §30 | protocol; see "Replaced" for the April 2026 prototype |
| 5 | [EventRepository.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/data/event/EventRepository.kt) | `handleEvents`: ack order and the per-connection marker, cross-checked in April 2026 | CHANGELOG §30 | protocol; the shipped socket code is a 2026-08-12 rebuild from `docs/backend-spec.md` §11 |
| 6 | [ClientRequest.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/data/network-model/src/commonMain/kotlin/com/wire/kalium/network/api/authenticated/client/ClientRequest.kt) (`ClientCapabilityDTO`), [UpdateSelfClientCapabilityToConsumableNotificationsUseCase.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/feature/client/UpdateSelfClientCapabilityToConsumableNotificationsUseCase.kt) | how the client capability is serialised and when it is declared | observations.md §22 | protocol |
| 7 | [RegisterClientUseCase.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/feature/client/RegisterClientUseCase.kt) → `RegisterMLSClientUseCase` | register the client first, then `PUT` the MLS public key | api-notes.md, `PUT /clients/{id}`; observations.md §19 | protocol (the server enforces the order) |
| 8 | [MessageSendingInterceptor.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/feature/message/MessageSendingInterceptor.kt), [MessageContentEncoder.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/util/MessageContentEncoder.kt), [MessageContentEncoderTest.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonTest/kotlin/com/wire/kalium/logic/util/MessageContentEncoderTest.kt) | the quote digest other clients require for rendering, its byte format, and the conformance vectors | observations.md §33 | protocol (format); replaced (vectors) |
| 9 | [MessageTextEditHandler.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/sync/receiver/handler/MessageTextEditHandler.kt) | an edit rewrites the stored message id | observations.md, edit-chain entry | divergent (stable key plus edit chain, migration V12) |
| 10 | [DeleteMessageHandler.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/sync/receiver/handler/DeleteMessageHandler.kt) | who may delete whose message | observations.md §91 | divergent (narrowed to ephemeral targets after a black-box test of the webapp) |
| 11 | [DeleteEphemeralMessageForSelfUserAsReceiverUseCase.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/feature/message/ephemeral/DeleteEphemeralMessageForSelfUserAsReceiverUseCase.kt), […AsSenderUseCase.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/feature/message/ephemeral/DeleteEphemeralMessageForSelfUserAsSenderUseCase.kt) | who enforces an ephemeral message's expiry | observations.md §74 | protocol (matches the wire-webapp observation) |
| 12 | [ConversationGroupRepository.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/data/conversation/ConversationGroupRepository.kt), [MLSConversationRepository.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/data/conversation/MLSConversationRepository.kt) | self-leave is a REST `DELETE`; the server issues the Remove | observations.md §31 | protocol |
| 13 | [KeyPackageApiV5.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/data/network/src/commonMain/kotlin/com/wire/kalium/network/api/v5/authenticated/KeyPackageApiV5.kt) | the `ciphersuite` query parameter goes out as a decimal | api-notes.md, key-package claim and count | protocol; see "Attribution" |
| 14 | [KeyPackageLimitsProvider.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/data/keypackage/KeyPackageLimitsProvider.kt), [KeyPackageManager.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/feature/keypackage/KeyPackageManager.kt) | key-package pool size, refill threshold, check interval | api-notes.md, key-package count; planning tables (entry 19) | convention: 100, refill below half, check once per 24 h are also Wire web's (`MLSService.ts`) and Wire iOS's (`MLSService.swift`) values |
| 15 | [FileSharingStatus.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/configuration/FileSharingStatus.kt), [FeatureConfigModel.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/core/data/src/commonMain/kotlin/com/wire/kalium/logic/data/featureConfig/FeatureConfigModel.kt) | Kalium's file-sharing feature model; the server flag is a boolean | asset-protocol.md Q7; ADR-0012 | replaced (see below) |
| 16 | [AESUtils.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/core/cryptography/src/commonJvmAndroid/kotlin/com.wire.kalium.cryptography/utils/AESUtils.kt), [CryptoUtils.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/core/cryptography/src/commonMain/kotlin/com/wire/kalium/cryptography/utils/CryptoUtils.kt), [AssetRepository.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/data/asset/AssetRepository.kt), [AssetMapper.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/data/asset/AssetMapper.kt), [UploadAssetMessageMetadata.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/feature/asset/upload/UploadAssetMessageMetadata.kt), [UploadAssetUseCase.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/feature/asset/upload/UploadAssetUseCase.kt), [AssetMessage.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/core/data/src/commonMain/kotlin/com/wire/kalium/logic/data/asset/AssetMessage.kt), [AssetApiV0.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/data/network/src/commonMain/kotlin/com/wire/kalium/network/api/v0/authenticated/AssetApiV0.kt), [AssetApiV2.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/data/network/src/commonMain/kotlin/com/wire/kalium/network/api/v2/authenticated/AssetApiV2.kt), [AssetApiV12.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/data/network/src/commonMain/kotlin/com/wire/kalium/network/api/v12/authenticated/AssetApiV12.kt), [AssetRetentionType.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/data/network-model/src/commonMain/kotlin/com/wire/kalium/network/api/model/AssetRetentionType.kt), [AssetResponse.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/data/network-model/src/commonMain/kotlin/com/wire/kalium/network/api/authenticated/asset/AssetResponse.kt), [MessageEncryptionAlgorithm.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/core/data/src/commonMain/kotlin/com/wire/kalium/logic/data/message/MessageEncryptionAlgorithm.kt), [messages.proto](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/tools/protobuf-codegen/src/main/proto/messages.proto) | the deployed asset protocol: AES-256-CBC with IV prefix, SHA-256 over the blob, verify before decrypt, the multipart upload, retention, the `Asset` protobuf | asset-protocol.md Q1–Q8; ADR-0011 | protocol; Kupferdraht's streaming (64 KB chunks against Kalium's 8 KB), single-pass hashing, cache and storage are its own; two of the surveyed items (local transfer status, plaintext on disk) were decided the other way |
| 17 | [MessageContent.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/core/data/src/commonMain/kotlin/com/wire/kalium/logic/data/message/MessageContent.kt), [AssetContent.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/core/data/src/commonMain/kotlin/com/wire/kalium/logic/data/message/AssetContent.kt) | the catalogue of message contents a Wire client handles | docs/wire/message-types.md, asset-protocol.md | protocol (the schema dictates the variants) |
| 18 | [ConversationScope.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/feature/conversation/ConversationScope.kt), [MessageScope.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/feature/message/MessageScope.kt), [UserSessionScope.kt](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/feature/UserSessionScope.kt) | scope nesting, the first implicit API model (April 2026) | ADR-0009; LEARNINGS.md §4.1 | divergent (the public API follows matrix-rust-sdk since 2026-04-22) |
| 19 | planning tables, 2026-04-08 to 2026-04-22 | the first plan carried "Kalium prior art" tables with file and line links per step (among them `MLSConversationRepository.kt`, `RefillKeyPackageUseCase.kt`, `KeyPackageManager.kt`, `MLSMessageUnpacker.kt`, `MLSWelcomeEventHandler.kt`) | ADR-0009; LEARNINGS.md | superseded by the clean-slate plan of 2026-04-22; the shipped MLS code does not follow those files' decomposition |
| 20 | Kalium production-readiness review, 2026-09 | a list of defect classes used as a checklist for Kupferdraht's own code | open.md §69; CLAUDE.md | not a code reading; Steps 77–80 |

## Replaced

**Quote-hash conformance vectors.** Step 6-II-b (2026-04-30) took the
two vectors Kalium's `MessageContentEncoderTest.kt` uses — an emoji
body and a YouTube URL, both stamped 2018-10-22 — into Kupferdraht's
unit tests. They are Wire's cross-client conformance vectors (the same
values sit in wire-webapp, wire-ios, wire-web-packages and
wire-apps-jvm-sdk). On 2026-05-28 they were replaced by Kupferdraht's
own body, timestamp and digests; the two-case shape of the tests (one
emoji body for the UTF-16 surrogate-pair path, one plain URL) remains and
is the natural minimal coverage. The digest algorithm itself is Wire's
protocol (observations.md §33).

**File-sharing model.** The Q7 survey (entry 15) described Kalium's
three-state `FileSharingStatus`; Kupferdraht's `FileSharingPolicy`
took over that shape, including an allow-list read from a
`config.allowedTypes` field no Wire backend defines (in Kalium the list
is a build-time flag). The provenance audit of 2026-10 identified this
as the one place where a reading of Kalium shaped a Kupferdraht type;
the enum is `Disabled | Enabled` since ADR-0012 revision 1.3.

**WebSocket prototype.** On 2026-04-26 a first socket DTO file declared
itself a mirror of Kalium's `ConsumableNotificationResponse` and used
its type names; it was deleted the same day ("drop WebSocket prototype,
ship pull-only"). The shipped socket code is a 2026-08-12 rebuild from
`docs/backend-spec.md` §11, whose frame shapes come from live capture.

## Attribution

- **Decimal ciphersuite (entry 13).** The parameter's decimal form was
  first learned from `KeyPackageApiV5` (Step 6b, 2026-04) and later
  confirmed by live smoke against other Wire clients; the code comment
  was reworded to the live observation on 2026-05-28. Both are true; the
  first source is recorded here.
- **WebSocket authentication.** LEARNINGS.md once credited "Kalium's v0
  behavior" for passing the access token as a query parameter on the
  socket. Kalium authenticates its socket with a Bearer header and never
  does that; the attribution was wrong and is corrected in place
  (2026-10-20).
- **Comment sweep of 2026-05-28.** "drop Kalium-source citations from
  shipped artifacts" removed file-level Kalium citations from shipped
  code comments and restated the facts in terms of the live observation
  or Kupferdraht's own constraint. The observations the comments pointed
  to stayed in `docs/observations.md` and `api-notes.md`; this index
  restores the file-level sources.

## Not read

No Kotlin was transcribed. Identifiers, comments, string literals,
numeric constants, the SQL schema, tests and fixtures were compared
against the whole Kalium tree in October 2026
(`docs/wire/provenance-audit-2026-10.md`); Kupferdraht's keystore,
recovery, task and case subsystems, its FFI, wasm and CLI layers have no
Kalium counterpart.
