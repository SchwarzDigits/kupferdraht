# Architecture Decision Records

Index of decisions for this component. Each ADR has its own file, and
each ADR has a same-named condensed rule file in `rules/` (the binding
short form — see `rules/README.md`); the two change together in the
same commit (CLAUDE.md freshness rule).

**Status lifecycle:** Proposed → Accepted → optionally
Superseded by ADR-NNNN or Deprecated. A **new choice** still requires
a superseding ADR — do not silently rewrite an accepted decision.
The Status line and Revision History may always be edited.

**Amendments (same decision, current truth):** Decision,
Consequences, Implementation Notes, and References may be amended
in place when leaving the original wording would force a later ADR
to contradict this one. Every such edit is a dated Revision History
entry that names what changed and why it is not a supersession. The
condensed `rules/` sibling updates in the same commit.

See `_template.md` for the format.

| Nr | Title | Status | Date | Link |
|----|-------|--------|------|------|
| 0001 | Use openmls and proteus directly instead of CoreCrypto | Accepted | 2026-04-20 | [→](0001-use-openmls-and-proteus-directly.md) |
| 0002 | No separation between cryptographic primitives and application logic | Accepted | 2026-04-21 | [→](0002-no-crypto-logic-split.md) |
| 0003 | Per-field AES-GCM encryption with DEK instead of SQLCipher | Superseded by ADR-0013 | 2026-04-21 | [→](0003-per-field-encryption-instead-of-sqlcipher.md) |
| 0004 | Integrate Wire's External Senders Extension | Accepted | 2026-04-21 | [→](0004-integrate-external-senders-extension.md) |
| 0005 | Web delivery via WebAssembly only | Superseded by ADR-0015 | 2026-04-21 | [→](0005-web-delivery-via-wasm-only.md) |
| 0006 | Implement Kupferdraht in Rust instead of Kotlin Multiplatform | Accepted | 2026-04-21 | [→](0006-rust-instead-of-kmp.md) |
| 0007 | Implement the test CLI in Rust, not on the JVM | Accepted | 2026-04-21 | [→](0007-cli-in-rust-not-jvm.md) |
| 0008 | Target Wire API v14 directly | Accepted | 2026-04-22 | [→](0008-target-wire-api-v14.md) |
| 0009 | matrix-rust-sdk as Kupferdraht's conceptual API model | Accepted | 2026-04-22 | [→](0009-matrix-rust-sdk-as-api-model.md) |
| 0010 | Decouple Kupferdraht from GPL dependency stack | Accepted | 2026-04-23 | [→](0010-decouple-from-gpl-dependencies.md) |
| 0011 | Asset content protocol — AES-256-CBC over Wire's `/assets` endpoint | Accepted | 2026-05-05 | [→](0011-asset-content-protocol.md) |
| 0012 | Server feature configs and user properties — fetch all, persist all, expose generically | Accepted | 2026-05-05 | [→](0012-server-feature-configs.md) |
| 0013 | SQLCipher as the whole-DB encryption engine | Superseded by ADR-0021 | 2026-06-10 | [→](0013-sqlcipher-engine.md) |
| 0014 | Tasks as event-sourced GenericMessage content | Accepted | 2026-07-16 | [→](0014-tasks-as-event-sourced-message-content.md) |
| 0015 | Browser architecture — WASM-only delivery, IndexedDB per-record persistence, hand-written wasm-bindgen surface | Accepted (Part 2 superseded by ADR-0022) | 2026-07-17 | [→](0015-browser-architecture-wasm-indexeddb-bindgen.md) |
| 0016 | License Kupferdraht under GPL-3.0-only | Accepted | 2026-07-23 | [→](0016-license-kupferdraht-gpl-3.0-only.md) |
| 0017 | Reinstate the /v14/events WebSocket push transport (native, opt-in) | Accepted — rule only: the ADR file was never committed; the binding form is the rule, the rationale ADR-0019 §Context (polling default superseded by ADR-0033, reconnect backoff by ADR-0039) | 2026-08-12 | [→](../../rules/0017-reinstate-websocket-push-transport.md) |
| 0018 | Represent room case state as event-sourced snapshot messages | Accepted | 2026-08-17 | [→](0018-case-state-as-event-sourced-snapshots.md) |
| 0019 | Auto-negotiate the push transport across /v14/events and legacy /await | Accepted (items 2–3 superseded by ADR-0030; reconnect backoff by ADR-0039) | 2026-08-21 | [→](0019-auto-negotiate-push-transport.md) |
| 0020 | Mark task-list rooms and re-broadcast full state on invite | Accepted | 2026-08-24 | [→](0020-task-list-marker-and-invite-state-rebroadcast.md) |
| 0021 | Adopt SQLite3MC with ChaCha20-Poly1305 as the whole-DB encryption engine | Accepted (build glue replaced by ADR-0042) | 2026-09-16 | [→](0021-sqlite3mc-engine-chacha20.md) |
| 0022 | Persist browser data in SQLite3MC over OPFS instead of IndexedDB per-record encryption | Accepted (item 7 superseded by ADR-0023; item 3 by ADR-0035; the stand-in's wasm arm lives in the fork since ADR-0042) | 2026-09-24 | [→](0022-browser-persistence-sqlite3mc-over-opfs.md) |
| 0023 | Enforce one active Kupferdraht instance per browser origin with a Web Locks lease and explicit takeover | Accepted | 2026-09-24 | [→](0023-single-active-kupferdraht-instance-per-origin.md) |
| 0024 | Guard the native critical sections with a per-account advisory file lock, fail-fast | Accepted | 2026-09-24 | [→](0024-native-cross-process-lock-per-account.md) |
| 0025 | Scope keystore ids and the login cookie label per home with a persisted home id | Accepted | 2026-09-25 | [→](0025-scope-keystore-ids-and-cookie-label-per-home.md) |
| 0026 | Serve every tab of an origin from the one active instance — a leader tab elected by the lease queue, thin-client tabs over BroadcastChannel | Accepted | 2026-09-25 | [→](0026-leader-tab-serves-thin-client-tabs.md) |
| 0027 | Attribute inbound messages to the MLS-authenticated leaf credential | Accepted | 2026-09-26 | [→](0027-attribute-inbound-messages-to-mls-leaf-credential.md) |
| 0028 | Authorize inbound edits and deletes by the original sender, with Wire's delete exceptions | Accepted | 2026-09-26 | [→](0028-authorize-edits-and-deletes-by-original-sender.md) |
| 0029 | Create a database key once — never replace a missing one | Accepted | 2026-09-27 | [→](0029-create-database-key-once-never-replace.md) |
| 0030 | Treat the legacy /await push socket as a doorbell for the REST drain | Accepted ("no periodic drain" superseded by ADR-0039) | 2026-09-28 | [→](0030-legacy-push-socket-is-a-doorbell.md) |
| 0031 | Serialise the drain per account and defer, not heal, around own pending commits | Accepted | 2026-09-28 | [→](0031-serialise-drain-and-defer-around-own-commits.md) |
| 0032 | Commit an inbound notification's MLS state only after its data is persisted | Accepted (item 1 superseded by ADR-0037) | 2026-09-28 | [→](0032-commit-mls-state-after-persisting-notification.md) |
| 0033 | Make push the default receive transport wherever a target has one | Accepted | 2026-09-28 | [→](0033-push-default-where-a-push-transport-exists.md) |
| 0034 | Heal a room only on a confirmed gap, and leave a group only on the backend's word | Accepted | 2026-09-29 | [→](0034-heal-only-on-confirmed-gap.md) |
| 0035 | Store an account's browser databases on sqlite-remote-vfs, with keys from a per-account seed that Kupferdraht seals with a PIN | Accepted (in part superseded by ADR-0040) | 2026-09-29 | [→](0035-store-browser-databases-on-sqlite-remote-vfs.md) |
| 0036 | Admit browser storage connections with short-lived, key-bound tokens from a token exchange | Accepted | 2026-09-29 | [→](0036-admit-storage-connections-with-exchanged-tokens.md) |
| 0037 | Commit a drained page, not each notification | Accepted | 2026-09-29 | [→](0037-commit-a-drained-page.md) |
| 0038 | Heal the browser's storage after the storage server was unreachable | Accepted | 2026-09-30 | [→](0038-heal-browser-storage-after-server-outage.md) |
| 0039 | Drive sync by triggers, with timers only as safety nets | Accepted | 2026-10-02 | [→](0039-drive-sync-by-triggers-timers-only-as-safety-nets.md) |
| 0040 | Keep one remote device per user, held as a slot on the storage server | Accepted | 2026-10-02 | [→](0040-one-remote-device-per-user-storage-slot.md) |
| 0041 | Rename Sodium to Kupferdraht as a clean cut | Accepted | 2026-10-07 | [→](0041-rename-sodium-to-kupferdraht-as-a-clean-cut.md) |
| 0042 | Bundle SQLite3MC through forked rusqlite, refinery and openmls_sqlite_storage, not through a root `[patch]` | Accepted | 2026-10-31 | [→](0042-sqlite3mc-via-forked-rusqlite-not-a-root-patch.md) |
