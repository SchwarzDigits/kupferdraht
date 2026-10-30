# Step 20c — full FFI surface via a dedicated wrapper layer (approach B)

> **Status:** planned (2026-06-10), pending review.
> **Depends on:** 20a (proc-macro scaffolding + thin slice), 20b (Swift XCFramework/SwiftPM,
> Kotlin/JVM JAR, Android AAR) — all shipped on `feature/uniffi-bindings`.
> **Branch / commit prefix:** `step/20c-*`, `feat(ffi): …`.
> **Reference:** matrix-rust-sdk-ffi (a dedicated FFI crate wrapping matrix-sdk; ADR-0009).
> **Supersedes:** the brief "20c — full API surface" sketch in `docs/plan/20-uniffi-bindings.md`,
> which assumed approach A (annotate core types in place).

---

## Context

20a/20b shipped a **thin vertical slice** and packaged it for all three native targets
(Swift XCFramework + SwiftPM, Kotlin/JVM JAR, Android AAR), each proven by a binding-level
test. The exports so far are **approach A** — UniFFI annotations live directly on / around the
core types (`#[derive(uniffi::Object)]` on `kupferdraht::Client`, an `#[uniffi::export] impl Client`,
free fns in `lib.rs` / `builder.rs` / `identity`).

20c exports the **rest of the consumer API**, which is the bulk of the work. At that scale,
approach A frays `kupferdraht-core` with FFI concerns and forces a parallel FFI-shaped method (with
a name clash) next to every borrowing getter. So we switch to **approach B**.

## Measured surface (what "full" means)

- **123 `pub async fn`** across the crate — most consumer methods are async.
- `Client` ~40+ public methods; `Room` 26; `Timeline` ~15; `Account` 6 getters.
- ~15 record types (`TimelineItem`, `Message`, `AssetSummary`, `DeviceInfo`, `RoomMember`,
  `JoinSummary`, `SyncSummary`, `FeatureConfigEntry`, `UserPropertyEntry`, …).
- ~8 enums (`TimelineItemKind`, `TimelineDiff`, `MessageBody`, `RoomType`, `RoomAccess`,
  `ReceiptMode`, `FileSharingPolicy`, `KupferdrahtError` ✓).
- 2 observer traits beyond `SecureKeyStore`: `EventHandler` (received messages),
  `TimelineListener` (diffs).

## Decision — approach B: a dedicated `ffi` layer (resolved with Markus)

A new in-crate module `kupferdraht-core/src/ffi/` holds the FFI-shaped surface. The native API
(`kupferdraht::Client`, `Room`, `Timeline`, …) stays **pure Rust** — `&UserId` returns, `Arc<Room>`,
generics, `impl Stream`, `&Path` — and the `ffi` layer wraps it:

- **Stateful types → wrapper objects.** `ffi::Client { inner: Arc<kupferdraht::Client> }`,
  `ffi::Room`, `ffi::Timeline`, … carry `#[derive(uniffi::Object)]` and expose FFI-shaped
  methods (owned returns, `async_runtime = "tokio"`, String paths) that delegate to `inner`.
  Foreign consumers see `Client` / `Room` / `Timeline` (the wrappers).
- **Value types → plain records/enums.** Data carriers (`ServerConfig`, `QualifiedUserId`,
  `TimelineItem`, `Message`, enums) are `#[derive(uniffi::Record/Enum)]`. These need no
  wrapper — they are already FFI-shaped data. **Open question D2** decides whether their derive
  lives on the core type (shared) or as an `ffi`-local mirror.
- **Callbacks stay on the core trait.** `SecureKeyStore` is a genuine core input (used by
  `load_or_create_key`); it keeps its `#[uniffi::export(with_foreign)]`. `EventHandler` /
  `TimelineListener` are likewise core traits the `ffi` layer re-exports as callback interfaces.

The cdylib is still `kupferdraht-core` (the `ffi` module lives inside it) — no separate crate, but
the same separation matrix-rust-sdk-ffi gets from one. FFI churn is isolated in `ffi/`; the
core stays idiomatic.

### Why B over A
A's parallel-method-per-getter + name clashes (`user_id() -> &UserId` vs an FFI
`qualified_user_id()`) multiply across ~80 methods. B concentrates the owned-return / async /
no-generics / no-stream reshaping in one module and keeps it out of the core types.

## Rework the existing 20a/20b exports into B (do this first — 20c-0)

The shipped slice is approach A and must be migrated so the surface is consistent:

| Current export (approach A) | Where | Under B |
|---|---|---|
| `#[derive(uniffi::Object)] Client` + `#[uniffi::export] impl Client { qualified_user_id }` | `client/runtime.rs` | **Move out.** Core `Client` loses the derive + export impl; `ffi::Client` wraps `Arc<kupferdraht::Client>` and exposes `qualified_user_id`. |
| `login(...)` free fn | `client/builder.rs` | Move to `ffi/`; returns `Arc<ffi::Client>`. |
| `parse_user_id(&str)`, `QualifiedUserId` | `identity/mod.rs` | Move to `ffi/` (FFI convenience + FFI value form of `UserId`). |
| `kupferdraht_version()` | `lib.rs` | Move to `ffi/`. |
| `KupferdrahtError` `#[derive(uniffi::Error)]` | `error.rs` | **D2:** likely stays on the core enum (already clean, structured, naturally shared). |
| `ServerConfig` `#[derive(uniffi::Record)]` | `client/builder.rs` | **D2:** stays on core, or an `ffi` mirror. |
| `SecureKeyStore` `#[uniffi::export(with_foreign)]` | `keystore/mod.rs` | Stays on the core trait (core input). |

**Exit gate for 20c-0:** all three durchstiche (Swift `swift test`, Kotlin `./gradlew test`,
Android AAR assemble) still green against the relocated surface — a pure refactor, zero
behaviour change.

## Design decisions to settle (review these)

- **D1 — wrapper granularity.** Wrap only stateful objects (`Client`/`Room`/`Timeline`/
  `Kupferdraht`/`PendingClient`); keep value types as direct records/enums. *(Recommended.)*
- **D2 — value types: core-derive vs `ffi` mirror.** Keep `uniffi::Record/Enum/Error` on the
  core types (shared, less duplication) vs. define `ffi`-local mirrors with `From` conversions
  (cleaner boundary, more boilerplate). *Recommendation: derive on core for pure data + error;
  mirror only where the FFI shape genuinely differs.*
- **D3 — streams & closures.** `sync_stream` (`BoxStream`) and `rooms_filtered<F>` /
  `sync_with_callback<F>` can't cross UniFFI. Replace with callback interfaces + a `TaskHandle`
  (cancellation) — the matrix-rust-sdk-ffi pattern. *(This is the design-heavy part, lands in
  20c-5.)*
- **D4 — `StorageProvider` / `Store` / `GlobalStore`: out of the FFI surface.** They are a
  Rust integration seam (cloud-bot backend), not a mobile-binding concern; exporting `Store`
  as a foreign-implementable interface would be huge and pointless. *(Recommended: out.)*
- **D5 — foreign-facing names.** Wrappers are named `Client` / `Room` / `Timeline` (matrix
  convention), not `KupferdrahtClient`. Confirm.
- **D6 — async runtime lifecycle.** One global tokio runtime via `async_runtime = "tokio"`;
  confirm no per-call runtime churn.
- **D7 — id flattening.** `UserId` crosses as the `QualifiedUserId` record (already decided in
  20a); apply the same to other borrowing getters (owned String / ByteArray).

## Sub-steps (each: TDD binding test → gate → regenerate bindings green)

- **20c-0 — `ffi` module + migrate the slice** (the rework above). Refactor, durchstiche stay green.
- **20c-1 — identity / session + Account.** `ffi::Client` getters (device_id, access_token,
  homeserver_url, …) flattened; `ffi::Account`. Establishes the getter pattern at scale.
- **20c-2 — Rooms.** `ffi::Room` + `RoomMember` / `RoomType` / `RoomAccess`; room list / lookup
  / `join_room` / `create_dm` / `dm_with` / `leave` / `rename`.
- **20c-3 — Timeline read.** `TimelineItem` / `TimelineItemKind` / `TimelineDiff` records+enums;
  `messages` / `items`.
- **20c-4 — Timeline write.** send_text / reply / edit / redact / toggle_reaction /
  mark_as_read / mentions / assets (`&Path` → String).
- **20c-5 — Sync + observers (hard).** `EventHandler` / `TimelineListener` as callback
  interfaces; `sync_once` / `sync_with_callback`; `Subscription` + a `TaskHandle` replacing
  `sync_stream`.
- **20c-6 — Manager + register/activate.** `ffi::Kupferdraht` (multi-account) + `KupferdrahtBuilder`,
  `PendingClient` (consuming `activate`) wrapped as free fns / object methods.

## 20c-5 design — sync + observers (resolved with Markus)

All per recommendation:

- **Continuous sync = `Client.start_sync(settings, listener) -> TaskHandle`** (plus
  `sync_once(settings) -> SyncSummary` for one-shot). The SDK spawns the poll loop on the
  tokio runtime; the foreign `SyncListener` receives a `SyncSummary` per iteration (and may
  return `false` to stop); `TaskHandle.cancel()` and dropping the handle both abort the loop.
  This replaces the non-FFI-able `sync` / `sync_with_callback<F>` / `sync_stream`. Chosen over
  exposing `sync` as a cancellable suspend fn (which would lean on uncertain UniFFI
  async-cancellation semantics).
- **Observers = foreign callback trait + Rust adapter.** `ffi::EventHandler` /
  `ffi::TimelineListener` / `ffi::SyncListener` are `#[uniffi::export(with_foreign)]`; a private
  Rust adapter implements the matching core trait and forwards core→ffi-converted values. The
  core traits can't be exported directly — they carry `Box<AssetSummary>`-bearing types.
- **Mirror types**: `Message`, `MessageBody` (`Text` / `Asset`), `TimelineDiff`
  (Append/UpdateAt/RemoveAt/Reset). `MessageBody::Asset` / `TimelineItemKind::Asset` carry the
  read-only `AssetMeta` record (download via `Timeline.asset(id)`), consistent with the
  timeline-read mirror — object refs can't sit in enum variants.
- **Registration**: `Client.add_event_handler(handler)` is handle-less (Client lifetime, as in
  core); `Timeline.subscribe(listener) -> Subscription` and
  `Client.add_timeline_listener(listener) -> Subscription`. `ffi::Subscription` is a
  `uniffi::Object` with `unsubscribe()` + unsubscribe-on-Drop; `ffi::TaskHandle` aborts on
  `cancel()` + Drop.
- **Value records**: `SyncSettings { poll_interval: Option<Duration> }`,
  `SyncSummary { rooms_updated, events_dispatched }` — FFI-friendly, derived on the core type.
- **Threading**: callbacks fire from the tokio sync task; foreign impls must be `Send + Sync`
  (UniFFI enforces this for `with_foreign`).

## Out of scope
- `StorageProvider` / `Store` / `GlobalStore` FFI (D4), internal `Store::*` methods.
- WASM (separate `wasm-bindgen` track, ADR-0015).
- The `keyring`-on-mobile cfg cleanup (separate; keyring is being removed anyway).
- Android emulator instrumented test, additional Android ABIs (20d/20e).

## Risks
- **Surface size** — mechanical but pervasive; the wrapper layer adds boilerplate.
- **Observers / streams (20c-5)** — the one genuinely uncertain design (threading, lifetime,
  cancellation). Prototype it early.
- **Migration regression (20c-0)** — the three native packages must stay green through the
  relocation; bind a regression check into each sub-step's gate.

## Verification
- Per sub-step: `cargo fmt`/`build`/`clippy -D warnings`/`test`/`deny` green; regenerate
  Swift + Kotlin; at least one binding-level test (`swift test` / `./gradlew test`) covers the
  new surface; the Android AAR still assembles.
- `docs/bindings.md` / `docs/library.md` / `docs/api-mapping-matrix-rust-sdk.md` updated per
  the freshness rule.

## Branch / Commit
Sub-step work on `step/20c-*` (or continued on `feature/uniffi-bindings`). No `git push`
(Markus). `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
