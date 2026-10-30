# Kupferdraht Implementation Plan — Second Pass

This file is the **open backlog** — the steps still in progress, pending, or blocked. A step that merges leaves this list: its prose is archived self-contained under [`docs/plan/archive/`](docs/plan/archive/) and its change is recorded in [`CHANGELOG.md`](CHANGELOG.md). The per-step **prose** (sub-steps, Wire endpoints, public API, TDD lists, implementation notes) lives under [`docs/plan/`](docs/plan/) — one file per step. The **conventions** (Rust idioms, TDD workflow, file structure rules, language policy, Wire Message Types, MLS Protocol scope, Functional Scope) live in [`CLAUDE.md`](CLAUDE.md) at the repo root. **Architectural decisions** are recorded as ADRs under [`docs/adrs/`](docs/adrs/). For Wire-API protocol details and live-discovered behaviour, see [`docs/wire/api-notes.md`](docs/wire/api-notes.md), [`docs/wire/mls-deviations.md`](docs/wire/mls-deviations.md), and [`docs/observations.md`](docs/observations.md). For the first-pass retrospective, see [`docs/LEARNINGS.md`](docs/LEARNINGS.md). Threat model: [`docs/THREAT_MODEL.md`](docs/THREAT_MODEL.md).

## Status overview

Last update: **2026-10-06** — Steps 123 (sqlite-remote-vfs 0.5.0, PR #114) and 124 (PIN receipts: a successful export or restore costs no attempt; recovery data version 3 — BREAKING, PR #115) merged + archived. Before: **2026-10-05** — Steps 121 (a login that meets a retired device forgets its keys, PR #111) and 122 (small gaps of the browser surface: an own send wakes the sync loop, `pushState`, `email`, storage location, `join_via_link` by the link — BREAKING, PR #112) merged + archived. Before: **2026-10-02** — Step 120 (one remote device per user — the storage slot, ADR-0040, PR #109) merged + archived. Before: Step 119 (a browser learns at once that its databases were taken over — sqlite-remote-vfs 0.3.0, PR #107) merged + archived. Before: Steps 117 (Kupferdraht's logs reach the app through a listener, PR #104) and 118 (sync only what changed — the room list on demand, no polling beside a live socket, push as the native default; ADR-0039, PR #105) merged + archived. Previously: **2026-09-30** — Step 116 (kupferdraht-tabs: an evicted leader's late message moves no tab back) merged + archived. Steps 107–115 merged + archived, with ADRs 0035–0038 and the Rust 1.98 toolchain pin (PRs #78–#99): the browser's databases on sqlite-remote-vfs with keys from a per-account seed (110, 111a–d), recovery data sealed with a PIN (112a/b), self-healing storage (113), leaving a browser (114), the browser push doorbell (115, proposed as "112", renumbered), page commits (109), the Step-97a review follow-ups (107) and the remote-VFS spike (108). Previously: **2026-09-29** — Steps 96–106 merged + archived: the production-readiness review's blockers B5–B9 (96, 97a–97e, 105) and findings H1–H4, H6, H7 part 1, H8 (98, 99, 102, 106, 101, 104, 103), the stable leader takeover (100) and the wasm surface parity (the second Step 98). 2026-09-25: Step 94 (leader tab + thin-client tabs, `bindings/web/kupferdraht-tabs/`, ADR-0026 extends ADR-0023; smoked scripted + live) merged + archived. Step 93 (home-scoped keystore ids + cookie label, ADR-0025) merged + archived. Step 94 was commissioned the same day; Step 93 in progress (worktree instance). Step 85 archived (phase 12). Step 93 (home-scoped keystore ids + cookie label, follow-up of ADR-0024) commissioned; Step 85 (cross-process lock, ADR-0024 accepted; gate + smoke green 2026-09-25) merged from `step/85-cross-process-lock`. Step 79 (failed-send recovery: `Timeline::resend` / `resend_failed`, `Room::failed_sends`, V28 `ephemeral_ttl_ms`, reconnect contract written down; smoked 2026-09-24) merged + archived. 2026-09-24: Steps 77 (dev-keystore hardening), 83 (docs hygiene) and 92 (one active tab, ADR-0023) merged + archived; Step 86 (self-conversation: establish at epoch 0, `Client::self_room`, CLI `self-room`; live-probed + smoked 2026-09-24) merged + archived. Earlier today: Step 87 (atomic migrations) merged + archived; Step 88 (SQLite3MC verification, verdict go → ADR-0021) archived; Step 90 (engine swap, ADR-0021 executed) merged + archived; Step 91 (SQLite3MC in the browser over OPFS, ADR-0022) merged + archived. Step 92 (one active tab — Web Locks lease, ADR-0023) merged + archived; the wasm/web consumer can start. Prior: Step 89 (ephemeral messages incl. V1b receiver-side delete, OTP driver) merged + archived. Open besides: 78 + 82 + 84–85, 12/13 (Wire-gated), 20 (packaging). Step 82 (THREAT_MODEL 0.4 + charter refresh, incl. the Dehydrated-Device retirement) merged + archived; Step 13 (Dehydrated device) DROPPED by Volker (no longer needed; prose archived). 2026-09-24: Steps 77 (dev-keystore hardening), 83 (docs hygiene) and 92 (one active tab, ADR-0023) merged + archived; Step 86 (self-conversation: establish at epoch 0, `Client::self_room`, CLI `self-room`; live-probed + smoked 2026-09-24) merged + archived. Earlier today: Step 87 (atomic migrations) merged + archived; Step 88 (SQLite3MC verification, verdict go → ADR-0021) archived; Step 90 (engine swap, ADR-0021 executed) merged + archived; Step 91 (SQLite3MC in the browser over OPFS, ADR-0022) merged + archived. Step 92 (one active tab — Web Locks lease, ADR-0023) merged + archived; the wasm/web consumer can start. Prior: Step 89 (ephemeral messages incl. V1b receiver-side delete, OTP driver) merged + archived. Open besides: 78–79 + 82 + 84–85, 12/13 (Wire-gated), 20 (packaging).

Only **open** work lives here. The **Reference / next action** cell says
what's blocking or what's next, kept terse; all prose lives in the step's
[`docs/plan/`](docs/plan/) file. Once a step merges, its row leaves this
table — the step is archived under `docs/plan/archive/` (self-contained)
and recorded in `CHANGELOG.md`.

| Sub-step / Step | Status | Reference / next action |
|---|---|---|
| [12 — Key backup (mnemonic)](docs/plan/12-key-backup.md) | pending | No backup endpoint in Wire API v14 |
| [78 — Local account purge (destroy_store)](docs/plan/78-local-account-purge.md) | commissioned | Kalium-audit follow-up 2026-09-10: logout leaves ciphertext DBs + the KEYSTORE ENTRIES behind; key ids are kupferdraht-internal so consumers cannot clean up (kupferdraht-tui §10). Phase-1 decisions: API home (manager vs Client), delete order, idempotence. |
| [84 — Asset deletion + cache orphan cleanup](docs/plan/84-asset-lifecycle.md) | commissioned | Transfer from open.md gap-B.3 + gap-B.5 (2026-09-10). Phase 1 writes the def-D.2 mini-ADR; lands after Step 80 (cache-dir hardening). gap-B.4 (LRU) stays open. |
| [125 — The browser `Client` answers the account's handle](docs/plan/125-the-client-answers-its-handle.md) | in progress | Proposed by an embedder of the browser surface, which spends a `GET /self` of its own on one string the engine already holds (`Account::handle`). One wasm getter — the read side of Step 2g's `setHandle` — its `kupferdraht-tabs` mirror (ADR-0026: `GETTERS` row, proxy getter, `index.d.ts`) and the two doc homes. No ADR (no decision surfaced), no CLI command (browser-only surface, as in Step 121). Branch `step/125-the-client-answers-its-handle`; gate green incl. CI's wasm32 job, which the gate itself never runs (`pub mod wasm` is `cfg(wasm32)`). Smoked through an embedder's showcase app on the dev stage: the getter and the `GET /self` it replaces answered the same handle in one session. The smoke earned its keep — it found that the mirrored getter shadowed `HandleProxy`'s own routing id (`this.handle`, now `$handle`) and broke every tab-proxied session while gate, parity test and generated `.d.ts` were all green. Next: review. |
| [20 — UniFFI bindings + packaging (Kotlin/JVM, Android, Swift iOS/mac) (20a–20e)](docs/plan/20-uniffi-bindings.md) | partial | **20a–20c merged** (`f88dd06`, #6): proc-macros (UDL removed), full FFI surface (Client/Room/Timeline/Sync/Manager/Admin, approach B), Swift XCFramework+SwiftPM + Kotlin/JVM JAR with green round-trip tests. **20d partial**: Android AAR arm64-v8a + macOS via XCFramework. Pending: **20d** (armeabi-v7a/x86_64 + on-device test), **20e** (CI/Maven/SwiftPM publish + release — **precondition: a support-tier ADR** naming the platform matrix, per-tier promises, the CI OS matrix (today: ubuntu-x64 only; macOS-arm64 coverage is the dev machine, Windows never runs, and the wasm target has ZERO CI execution — the indexeddb_* suites run as 0 tests natively, so the W2/W3 restore paths are hand-tested only; the core-crypto #2576 class), and an FFI ABI/semver guard (today: manual "bindings generate clean" checks only); the JVM JAR bundles only the build host's dylib and nothing states support levels — the Kalium audit 2026-09-10 found the identical gap there (its B5/M10/M12 + ABI-baseline minor)). FFI surface re-validated 2026-06-15: Kotlin+Swift generate clean against current main (incl. `change_password`). |

### How to resume

1. `git checkout` the branch in the topmost **in progress** row above.
   If no row is in progress, the next branch to open is named in the
   topmost **next** row, branched off `main`.
2. Verify a clean baseline:
   ```bash
   cargo build --workspace 2>&1 | grep -c "warning"     # must print 0
   cargo test --workspace --no-fail-fast                # all tests must pass
   cargo clippy --workspace --all-targets -- -D warnings # no diagnostics
   ```
3. Read the corresponding [`docs/plan/`](docs/plan/) step file for the
   failing-tests checklist and the specific Wire endpoints / RFC
   sections / store additions / TDD assertions for the cycle you are
   entering.
4. Per CLAUDE.md TDD rule: write failing tests first, get them
   reviewed, only then implement.

### Update protocol for this table

- The first commit on a new step / sub-step branch updates the row to
  **in progress** and writes the branch name into the Reference column.
- On merge to `main`, the step **leaves this table**: fold its SHA +
  live-smoke date + scope into its `docs/plan/` file's status header,
  `git mv` the file to `docs/plan/archive/`, and record the change in
  `CHANGELOG.md` (the Step Procedure's cleanup phase does this, CLAUDE.md). This table holds
  open work only.
- **Keep the Reference / next-action cell terse** — what's blocking or
  what's next; all prose belongs in the step's `docs/plan/` file. A cell
  that grows into a paragraph is a review defect.
- Cross-cutting work on `chore/*` branches that lands on `main` is
  recorded in `CHANGELOG.md` (it is not tied to a step).
- Out-of-date status rows are a review defect, same class as
  out-of-date `docs/library.md` (CLAUDE.md Freshness rule).
- **Renumbering a step adds an entry to that step's file's
  Historical Names block.** The entry includes the date the
  rename took effect and the reason (typically "after ADR-NNNN
  inserted/removed a step"). The entry never edits or deletes
  prior history bullets — it appends a new one and updates the
  prior "current" entry to a closed date range.
- **Inserting a new step renumbers every later step file
  inclusive.** Each renamed file is `git mv`'d to its new name
  and gets a new Historical Names entry. The old filename
  remains in git history; readers chasing a stale filename
  reach the new file via git's rename detection.
- **Sub-step naming inside a step file is local.** Adding 6-II-h
  does not require renaming the file; the new sub-step lands as a
  new H3 inside the step's `docs/plan/NN-*.md` file and the
  Sub-steps line in the metadata block is extended.
