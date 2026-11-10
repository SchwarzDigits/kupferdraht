# Step 147 — Check recovery data without a PIN

**Status: MERGED 2026-11-10** — `63a273b` on `step/147-check-recovery-data`, merged to
`main` as PR #68 (`876d6a2`); gate green on 1.98 (fmt, 0 build warnings, clippy, 1628
tests, deny; wasm32 check, clippy `--lib` and CI's wasm clippy line); the browser suite
green locally (18 suites, 82 tests, headless Firefox) and in CI. No smoke: recovery is
browser-only and needs a PIN service and a storage server, which the local stack lacks.
Scope: `PendingLogin.checkRecoveryData(data)` gives the verdict on recovery data before
any PIN — `"ok"` or `InvalidRecoveryData`'s `reason` — with one check shared with
`recover` (`check_before_pin`): the bytes alone with no request, then the storage slot;
never the PIN service. Red tests approved by Volker on 2026-11-10, with these
deviations: the browser types name the verdict `RecoveryDataVerdict`; `recover` already
reported a file with several faults by its local one, so what changes is only that no
request precedes the local verdicts; the recovery tests' token exchange, slot query and
PIN service share one mock server, since parallel tests that each took three servers one
by one emptied httpmock's pool of 25 and waited for each other for good.

**Commissioned 2026-11-05** (from the browser embedder's
requirements for a work-queue client, R9, measured there against
`4f30f02` and re-checked against `main` `9e1a11c`). Taken with Volker on
commissioning:

- `oldDevice` needs the storage slot, so the check makes up to two
  read-only requests — a PIN token from the token exchange and
  `GET /v1/slot` on the storage server — and never one to the PIN
  service; the local verdicts come first and need no request;
- the preconditions of `recover` (`keys` `"missing"`, a
  `pinServiceUrl`), else `InvalidInput`; a failed slot query throws
  instead of answering;
- `PendingLogin.checkRecoveryData(data)` → `"ok" | "notRecoveryData" |
  "unsupported" | "otherAccount" | "oldDevice"` — the reasons
  `InvalidRecoveryData` carries today —, `RecoveryDataVerdict` in the
  core; browser-only like `recover` (ADR-0035), so no FFI mirror and no
  CLI command; matrix-rust-sdk has no analog (Kupferdraht-specific);
- after Step 152, before 129a.

## Problem

1. **The verdict comes only with the PIN.** `recover(data, pin)`
   (`manager/pending_login/first_setup.rs`) decides format, account and
   device before it asks the PIN service, but nothing exposes that
   decision alone. A sign-in screen learns that the picked file is
   another account's, an outdated format or a replaced device's only
   after the user typed the PIN.
2. **The embedder's workaround reads an opaque format.** It reads magic
   `SDRD`, the format version and the account hash at their offsets
   (`recovery/data.rs`) and compares the hash with `PendingLogin.userId`;
   the next format change breaks it silently, and `oldDevice` stays out
   of reach: it needs the storage slot (Step 120, ADR-0040).
3. **`recover` asks the slot first.** `slot_label_for_recovery` runs
   before `RecoveryData::decode` — a token purchase and a slot query —
   so a file that is not recovery data at all costs two requests
   before its verdict.

## Scope

1. **One check before the PIN, shared by both calls** (`recovery/flow.rs`):
   - local, no request: `RecoveryData::decode` (`notRecoveryData`,
     `unsupported`), the account hash against the login's account
     (`otherAccount`), the Argon2id cost a browser can give
     (`unsupported`);
   - then the slot: `account_slot` (PIN token, `GET /v1/slot`); no slot,
     or one being set up → `oldDevice`; the data's client id against the
     slot's label → `oldDevice`.
   `recover` runs it, then goes on to the PIN service as today; its
   errors for a single fault stay what they are, a file with several
   faults now reports the first local one.
2. **`PendingLogin::check_recovery_data(&self, data: &[u8]) ->
   Result<RecoveryDataVerdict, KupferdrahtError>`** (core, `cfg` like
   `recover`): `RecoveryDataVerdict::{Ok, NotRecoveryData, Unsupported,
   OtherAccount, OldDevice}`. It asks no PIN service, counts no attempt,
   installs nothing, and leaves the pending login as it was.
   `InvalidInput` with the seed present or without a PIN service; the
   token exchange's and the slot query's errors (`ServiceUnavailable`,
   `StorageAccessDenied`) as errors, not as a verdict.
3. **Browser.** `PendingLogin.checkRecoveryData(data: Uint8Array)` →
   `Promise<"ok" | "notRecoveryData" | "unsupported" | "otherAccount" |
   "oldDevice">`. `kupferdraht-tabs`: `["PendingLogin",
   "checkRecoveryData", "idempotent", null]` in `METHODS`, `proxy.js`,
   `index.d.ts`.
4. **No CLI command, no FFI mirror.** Recovery exists on the browser
   surface only (ADR-0035); the CLI cannot recover (Phase 6 noted, not
   skipped).
5. **Docs:** `docs/bindings.md` (the call, its requests, that it is no
   PIN attempt), `bindings/web/kupferdraht-tabs/README.md`, a note in
   ADR-0035 (the check without a PIN; `rules/0035-…` if its wording
   changes), `docs/api-mapping-matrix-rust-sdk.md` (Kupferdraht-specific),
   `CHANGELOG.md` `[Unreleased]` under Added.

## Out of scope

- Caching the slot between the check and a later `recover`: the slot can
  change in between (a takeover elsewhere), and `recover` asks it again.
- A check of the PIN itself — only the PIN service can say it, and every
  question costs an attempt.
- The native platforms: no recovery there.

## TDD sketch

`cfg(test)` like `recover`'s tests, mocks for the token exchange, the
slot and the PIN service:

- `"ok"` for the account's current data and the slot's device;
- `"otherAccount"`, `"notRecoveryData"` (foreign bytes, a truncated
  file), `"unsupported"` (a version-2 file, an Argon2id cost over the
  browser's limit) — each with no request at all;
- `"oldDevice"` for another client id, no slot and a slot being set up;
- the PIN-service mock sees no request in any case; the exchange and
  the slot see one each only after the local checks passed;
- `InvalidInput` with the seed present and without a PIN service; a slot
  query answering `503` → `ServiceUnavailable`, no verdict;
- `recover` unchanged for valid data, and a non-recovery file now fails
  before any request;
- tabs: `step_94_tab_proxy_parity` sees the call in `METHODS` and
  `index.d.ts`; gate with `cargo check` / `cargo clippy` for
  `wasm32-unknown-unknown` and the `ci.yml` browser targets.
- No smoke: recovery is browser-only and needs a PIN service and a
  storage server, which the local stack lacks; the browser tests run in
  CI.
