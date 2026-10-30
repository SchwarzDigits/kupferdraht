# Kupferdraht

Kupferdraht is a project of Schwarz Digits KG, originally developed by Volker Zöpfel: a Rust library for MLS-based end-to-end encrypted messaging against the Wire backend, exposed via UniFFI bindings for Kotlin (Android, JVM), Swift (iOS, macOS), and WebAssembly.

Always-on charter: binding invariants and prohibitions only. Reference detail lives in the on-demand homes named below, pulled in when a task needs it.

## Rules

`rules/` is the binding condensed form of every `docs/adrs/` decision — one file per ADR, same filename, each a one/two-line Do/Don't. Consult the relevant `rules/` file(s) before writing code; treat as authoritative. Read the ADR only for rationale. If a rule and its ADR disagree, the ADR wins (the rule is the defect). Freshness: change an ADR → update its same-named rule file in the same commit.

## Goal

A Wire-compatible MLS client in Rust that:

- uses `openmls` directly (Proteus prekeys come from an own GPL-free module, ADR-0010); crypto + app logic in a single crate — no internal FFI boundary;
- is written from the Swagger spec, live observation and RFC 9420 and transcribes **no Wire program code**. The one Wire-authored artifact in the tree is the Protobuf schema `kupferdraht-core/proto/generic_message.proto`: a modified subset of Wire's `GenericMessage` declarations (GPL-3.0-or-later) that keeps Wire's notice and states the modifications in its header. Wire's open-source repositories (Kalium, proteus, wire-server, wire-webapp, wire-android; GPL-3.0 / AGPL-3.0) are consulted to observe protocol behaviour where the spec is silent or an interoperability failure needs explaining (every reading is indexed in `docs/wire/kalium-readings.md`), and once (2026-09) a review of Kalium's production readiness served as a checklist of defect classes for Kupferdraht's own code (Steps 77–80); never as a model for structure, naming or implementation (ADR-0009); every such observation is recorded in `docs/` with its source;
- exposes Kotlin / Swift / WASM via UniFFI 0.29;
- enforces compiler-checked zeroization of sensitive material (THREAT_MODEL.md G4);
- targets Wire API v14 exclusively;
- shapes its public API after matrix-rust-sdk.

Client layer only — a Wire-compatible client, not a Wire replacement. Kalium (GPL-3.0) is not a foundation (see "What NOT to do").

## Stack

Rust 2024 · UniFFI 0.29 (tokio feature) · openmls 0.9 (crates.io; no proteus — ADR-0010) · SQLite3MC (vendored, ADR-0021/0022) · reqwest 0.12 (rustls, no native-tls) · tokio 1, serde 1, thiserror 2, tracing 0.1 · cargo-ndk (Android) · wasm-pack (WASM).

## Architecture (ADR-0009)

Three layers. Diagram, module layout, and the matrix-rust-sdk ↔ Kupferdraht symbol mapping: `docs/ARCHITECTURE.md` + `docs/api-mapping-matrix-rust-sdk.md`. Binding invariants:

- Public API is matrix-rust-sdk-shaped (`Client`, `Room`, `Timeline`, `SyncService`, `EventHandler`); backend speaks Wire v14. Replacing the backend changes only the bottom layer.
- `kupferdraht-core/src/wire/`: Wire field names verbatim, transport only, no business logic.
- `kupferdraht-core` outside `wire/`: matrix-rust-sdk naming, no Wire names.
- Public API surface (UDL + bindings): matrix-rust-sdk concepts, never Wire terminology.
- Crypto (openmls) and transport (HTTP) must not share a function — `timeline/` calls into `wire/`, never makes HTTP calls directly.

## Wire API: v14 Only (ADR-0008)

`CLIENT_SUPPORTED = &[14]`. Swagger: `reference/wire.swagger.json` (fetched by `scripts/fetch-wire-swagger.sh`, not committed); live notes: `docs/wire/api-notes.md`. Sync transports: polling (`GET /v14/notifications?since=`, the default) or push (`SyncTransport::WebSocket`), which auto-negotiates per connection — `GET /v14/events` (`ConsumableNotifications`, per-event ack, sync-marker cursor) where the server enables it, else the legacy `/await` socket (ADR-0019; current wire-server ships the feature hard-disabled). Proteus prekeys required at `POST /clients` even MLS-only — uploaded, never used to encrypt.

## Wire Message Types

All Wire content is serialized as `GenericMessage` Protobuf (`kupferdraht-core/proto/generic_message.proto`; variants in `docs/wire/message-types.md`). No custom message format. Unknown types: deserialize the envelope, log, discard — never skip at the envelope layer.

## MLS Protocol: RFC 9420 Only

MLS (RFC 9420) exclusively — a hard architectural boundary.

Forbidden: Olm / Megolm; Signal Protocol (Double Ratchet, X3DH); custom crypto primitives; direct `ring` / `rustls` / `openssl` outside what openmls/proteus pull in transitively.

No partial protocol, no security-disabling flags: PFS and PCS are not optional; Welcome / Commit / Proposal are fully processed; no `#[cfg(feature = "insecure")]` or similar.

Check: `grep -rn 'feature.*insecure\|cfg.*disable\|cfg.*skip' src/` → zero hits.

## Functional Scope

In scope: Wire's full text set (`Text` + `quote` + `mentions`, `MessageEdit`, `MessageDelete`, `Reaction`, `Confirmation`/READ), ephemeral text (`Ephemeral(Text)`, Step 89 — send with TTL, receive with a local self-redaction timer), `Asset` (Step 11, ADR-0011), read-only server-administered config (Step 10, ADR-0012), typing indicators (Step 21), tasks/todos (`TaskPayload`, Step 60, ADR-0014 — event-sourced, no server-side task state) — full round-trip each. Authoritative list: `README.md` "What it does" + `PLAN.md` (a scope change updates README in the same commit).

Hard out-of-scope (prohibitions):

- Voice / video / calling — no symbol from any calling library.
- e2ei certificate enrollment — MLS BasicCredentials path only.
- Outbound AES-GCM — receive-side toleration only (ADR-0011).
- JavaScript port — WASM only (ADR-0015). JVM CLI path — CLI is Rust (ADR-0007).
- Matrix-sense "threads" (Wire's `Text.quote` is in scope).

## Step Procedure

Every implementation step runs these 12 phases in order — do not reorder or skip. The **hard stops** are non-negotiable; standing law (the Conventions) binds throughout without being restated per phase.

1. **Orient** — read `docs/plan/NN-*.md`, the topic-relevant `rules/`, and `docs/ARCHITECTURE.md`. Gap or ambiguity → ask ("No Assumptions").
2. **Branch** — `step/NN-slug` off `main` (sub-steps `step/NNa-slug`); never work on `main`.
3. **Tests first (red)** — the tests only, no implementation; they compile but fail. Never implement before the failing tests exist.
4. **Approval — HARD STOP** — show the red tests, wait for approval; only then implement.
5. **Implement (green)** — until `cargo test` is green.
6. **CLI command** — add the `kupferdraht-cli` command exercising the feature, same change; the CLI is the API's first consumer, before bindings (ADR-0007).
7. **Parity + docs** — in this commit: FFI-mirror every changed `pub` consumer-facing symbol; update every doc home the change touched (freshness map below); a new `pub` symbol → an `api-mapping-matrix-rust-sdk.md` entry; a surfaced decision → run `new-adr`.
8. **Gate — HARD STOP** — all five green, no subset: `cargo fmt --check`, 0 build warnings, `clippy -D warnings`, `cargo test --no-fail-fast`, `cargo deny check`. fmt → `cargo fmt --all` + re-stage; a build/clippy warning is a defect (fix in code, never `#[allow]`); a failing test or a `deny` hit (GPL pull / open CVE) is a ship-stopper. Never commit on red. A step touching `kupferdraht-core/src/wasm/` or `bindings/web/` adds the wasm32 commands — the five never compile that code, so they cannot go red on it (Conventions → "Warnings are Errors").
9. **Smoke** — live against a self-hosted wire-server where the step has backend surface; a pure refactor/CLI/docs step skips it, noting "no smoke (refactor)".
10. **Land** — one commit bundling library + CLI. Stage only the coherent change (no blind `git add -A`); Conventional-Commits message `type(scope): subject`, step in scope. Merge to `main` only on green gate + smoke. Offer the commit; **push only when the user explicitly asks, never force-push** (the force variants stay denied in `.claude/settings.json`). `CHANGELOG.md` stays SHA-free.
11. **Retrospective — don't skip** — self-critical, scaled to the step: what was hacked/simplified/left `TODO`, what surprised us, what to do better. Learnings → `docs/LEARNINGS.md`, open questions → `open.md`, debt → the matching `docs/v1-*-audit.md`, proposed steps → `PLAN.md` (only with the user's OK). A "not done" verdict → back to the phase; don't archive yet.
12. **Cleanup / archive — don't skip** — remove the merged step's `PLAN.md` row (fold its SHA + smoke date + scope into the `docs/plan/NN-*.md` header first); add the change to `CHANGELOG.md` `[Unreleased]`, consumer-facing (`BREAKING:` when it breaks); move any resolved `open.md` item into the changelog; `git mv` the prose to `docs/plan/archive/`; strip scaffolding + dead code; clean the branch.

Test conventions (phase 3): integration tests → `kupferdraht-core/tests/<concern>.rs`; unit → `#[cfg(test)]` modules; HTTP → `httpmock`; crypto → known-answer vectors; `unwrap()` / `expect()` in tests only; stub missing signatures with `todo!()` to compile-but-fail.

## No Assumptions

Context missing or ambiguous → ask, don't guess.

## Conventions

Non-negotiable — a rule broken in review is a defect, not a style question.

### Rust

Edition 2024, idiomatic. `Result<T, E>` with `?`; no `unwrap()` in library code (tests only). Own error enums via `thiserror`; no `anyhow` in library code.

### Warnings are Errors

In `lib.rs`, before imports:

```rust
#![deny(warnings)]
#![deny(clippy::all)]
#![deny(clippy::pedantic)]
#![deny(unused_must_use)]
#![allow(clippy::module_name_repetitions)]
```

Mandatory pre-commit gate — all five, no subset (Step Procedure phase 8 — full commands here):

```bash
cargo fmt --all -- --check                            # format drift = 0
cargo build --workspace 2>&1 | grep -c "warning"      # must be 0
cargo clippy --workspace --all-targets -- -D warnings # clippy as gate
cargo test --workspace --no-fail-fast                 # tests green
cargo deny check                                      # licenses + advisories + sources
```

`cargo deny` enforces `deny.toml` + the RustSec feed — a GPL pull or open CVE is a ship-stopper. Install: `cargo install --locked cargo-deny`.

**A step that touches `kupferdraht-core/src/wasm/` or `bindings/web/` adds these, because the five above never compile that code.** `pub mod wasm` is `#[cfg(target_arch = "wasm32")]`, so `build`, `clippy --all-targets` and `test --workspace` walk past the browser surface entirely — all five go green on a module that does not type-check. Measured twice: gap-B.28 (a non-exhaustive match, "red for weeks because nothing here built wasm32", as the CI job's own comment records) and Step 125, where the five were green, the parity test was green and the generated `.d.ts` was green while the `kupferdraht-tabs` module was dead on arrival.

```bash
cargo check  -p kupferdraht-core --target wasm32-unknown-unknown              # it compiles at all
cargo clippy -p kupferdraht-core --target wasm32-unknown-unknown --lib -- -D warnings
./scripts/wasm-test.sh                       # the browser tests; needs Firefox + geckodriver
```

The first two are cheap and belong in the gate. `wasm-test.sh` runs the wasm integration tests in headless Firefox; a change a native test cannot reach should not wait for CI to find out — Step 125's rename was confirmed by `step_94_leader_tab`, not by anything native. All three need a wasm-capable clang (`CC_wasm32_unknown_unknown` / `AR_wasm32_unknown_unknown`; Apple clang has no WebAssembly backend) — see [`docs/bindings.md`](docs/bindings.md).

### Design

- SRP at function level: one responsibility, describable in one sentence without "and".
- ≤ 40 lines per function (60 only for a justified large `match`); longer is a review defect.
- SRP at module level: one module per responsibility, not per type. Split a file > 200 code lines (`#[cfg(test)]` excluded) that hosts > 1 responsibility. Different types → different files.
- Naming describes effect: `upload_key_package`, `encrypt_message`, `derive_key` — never `process` / `handle` / `do_work`. matrix-rust-sdk naming in core outside `wire/`; verbatim Wire names inside `wire/`.
- SoC: a concern crossing a layer boundary is a design defect.

### Assertions

Preconditions and invariants belong in code, not comments. Every function that does I/O, calls the network, or mutates state needs ≥ 1 assertion. Preconditions at entry; postconditions before return on mutation. `assert!()` for must-never invariants; `debug_assert!()` in hot paths.

Assertions guard CODE invariants — programmer errors a correct build makes impossible. Data that can be wrong at runtime (persisted rows, server responses, user input) is never asserted: it fails as a recoverable error (`SessionExpired`, not a panic) — a consumer's runtime is not a test harness. Live-proven 2026-08-13: a `debug_assert!` on a poisoned persisted token killed a tokio worker inside kupferdraht-tui (its open.md §15).

### No `unsafe`

Forbidden in hand-written code: `unsafe fn`, `unsafe { … }`, `std::mem::transmute`, `std::ptr::*`. Exception: UniFFI-generated `#[unsafe(no_mangle)]`.

Check: `grep -rn 'unsafe' src/` → only UniFFI-generated hits.

### No Custom Macros

Forbidden: `macro_rules!`, proc-macros, custom derives. Allowed: std macros, std derives, `#[derive(uniffi::...)]`.

Check: `grep -n 'macro_rules!' src/` → zero hits.

### Logging

`kupferdraht-core/` uses `tracing` only — never `println!` / `eprintln!`. Both fine in `kupferdraht-cli/`.

Check: `grep -rn 'println!' kupferdraht-core/src/` → zero hits.

### Language

Committed content is English — code, comments, commit messages, docs, CLI output (personal notes outside the repo: any language).

### Comments in Shipped Code

A comment describes the code as it is and names the decision behind it (`ADR-NNNN`); a step or sub-step code (`Step 89`, `6-II-d`) may stay as a pointer to its `docs/plan/` file, whose sub-step names are local to that file. No process narrative: no dates, no "pre-merge" / "this commit" / "V1", no review or ticket references, and no enumeration of Wire schema parts Kupferdraht does not implement — the scope catalogue is `docs/wire/message-types.md`. A live observation is recorded with its date in `docs/observations.md`; the comment says "live-confirmed" and points there.

Check: `grep -rnE '^\s*(//|///|//!).*20[0-9]{2}-[0-9]{2}-[0-9]{2}' kupferdraht-core/src/` → only constants whose value is a date.

### License Headers

Every source file Kupferdraht authors opens with two SPDX lines, `SPDX-FileCopyrightText: 2026 Schwarz Digits KG` and `SPDX-License-Identifier: GPL-3.0-only`, in the file type's comment syntax, after a shebang, `swift-tools-version` or `<!DOCTYPE>` line where one exists. Not tagged: `vendor/`, the Gradle wrappers, the SHA-pinned SQL migrations, the Protobuf schema (it carries Wire's notice and Kupferdraht's own), docs and configuration files. A new file gets the two lines by hand.

Check: `cargo test -p kupferdraht-core --test license_headers` — it names every tracked source file without the notice.

### Sensitive Material

Zeroize explicitly via `Zeroizing` / `ZeroizeOnDrop`; never rely on memory reuse. Compiler-enforced (ADR-0002, ADR-0006, THREAT_MODEL.md G4).

### README and Documentation

Living docs updated in the same commit as the code — out-of-date docs are a defect. The change → doc-home map (Step Procedure phase 7 applies it):

- CLI → `docs/cli.md` · Rust API → `docs/library.md` · UDL/bindings → `docs/bindings.md` · build/invocation or capability/scope → `README.md`
- layer/module/data-flow → `docs/ARCHITECTURE.md` (storage schema → §7)
- Wire endpoint → `docs/wire/api-notes.md` · Wire MLS deviation → `docs/wire/mls-deviations.md` · backend contract → `docs/backend-spec.md`
- decision → ADR in `docs/adrs/` + same-named `rules/` file · Wire/MLS gotcha → `docs/observations.md` · resolved change → `CHANGELOG.md` · open question / backlog item / non-goal → `open.md`

Consult the relevant doc before re-deciding; write observations to `docs/observations.md` and continue — don't autonomously rewrite docs or add ADRs.

### Public API ↔ FFI parity

A change to a `pub` consumer-facing symbol in `kupferdraht-core` is mirrored in the FFI layer (`kupferdraht-core/src/ffi/`) in the same commit — a missing mirror is a review defect. Mechanics (annotate directly vs. wrap in `ffi/`): `docs/bindings.md`. A new `pub` symbol gets an entry in `docs/api-mapping-matrix-rust-sdk.md` (ADR-0009).

### Branches and Commits

One branch per Step/sub-step: `step/{N}-{slug}`, sub-steps `step/{N}{letter}-{slug}`. Never multiple sub-steps per branch; no work on `main`. One commit per Step/sub-step, bundling library + the CLI command that validates it (small fix-ups inside a branch are fine). Merge to `main` only on a green gate + smoke.

## Implementation References (priority order)

1. matrix-rust-sdk (Apache-2.0) — architecture, structure, naming, UniFFI / observer patterns; adaptable directly. `matrix-sdk-ffi/`, `matrix-sdk/src/`.
2. RFC 9420 — MLS semantics.
3. OpenMLS (Apache-2.0) — MLS implementation details.
4. Wire v14 Swagger (`reference/wire.swagger.json`) — authoritative HTTP reference; cross-ref `docs/wire/api-notes.md`.
5. Wire's own repositories (Kalium, proteus, wire-server, wire-webapp, wire-android; GPL-3.0 / AGPL-3.0) — protocol observation only: what a Wire endpoint expects, an event means or a wire format looks like, where the Swagger is ambiguous; every observation is recorded with its source in `docs/wire/api-notes.md` or `docs/observations.md`. Never a structural/naming/implementation reference.

`reference/` is read-only. Per-source detail (matrix-rust-sdk / Kalium / natrium usage rules): `reference/README.md`.

## What NOT to do

- Don't copy Kalium's API names, scope nesting, or module structure — protocol observation only (ADR-0009).
- Don't target Wire API < v14 — v5 event stream retired (ADR-0008).
- Whole-DB encryption is SQLite3MC with ChaCha20-Poly1305 (ADR-0021, supersedes ADR-0013; landed with Step 90) — don't reintroduce SQLCipher/vendored OpenSSL, libsqlite3-sys's `SQLITE3_LIB_DIR` route, or per-field DEK (ADR-0003 superseded).
- Don't write a JavaScript port — WASM only (ADR-0015). Don't add a JVM CLI path — CLI is Rust (ADR-0007).
- Don't implement Proteus encryption/decryption — prekeys generated at registration, uploaded, forgotten.
- Don't add ADRs or rewrite docs autonomously — observations to `docs/observations.md`; doc changes go through discussion.
- Don't add feature flags that disable MLS security properties.
- Don't implement anything not required by the current and next Step — keep the public API minimal.
