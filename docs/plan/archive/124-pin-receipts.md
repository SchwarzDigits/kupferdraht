# Step 124 — PIN receipts: a successful export or restore costs no attempt

**Status: MERGED 2026-10-06** — commissioned 2026-10-06 (Markus, with
the storage/PIN side: natrium-pin-service#8, ECDSA P-256 per BSI
TR-02102-1). `0addf56`, smoke record `25583f0`. Merged to `main` as PR
#115 (`c70d94a`); CI green. Smoke 2026-10-06 against the PIN service 0.2.0
on the test stage. Scope shipped: the seed's receipt key, recovery data
version 3, `refundKey` / `attemptId` / `POST /v1/refund`; a successful
export or restore costs no attempt. Retrospective: `docs/LEARNINGS.md`
§5.65. Archived with this header.

## Problem

The PIN service counts every allowed evaluation against 5 per hour and 12
per day — the right PIN too: the service cannot tell a right guess from a
wrong one. A user who exports recovery data and restores it a few times
locks themselves out for the day.

## The service's contract (natrium-pin-service, `docs/protocol.md`)

- Receipt key pair from the seed: `refundSeed = HKDF-SHA256(ikm = seed,
  salt = empty, info = "natrium-pin-refund-v1|<domain>|<userId>")`, 32
  bytes; private key `d = DeriveKeyPair(refundSeed,
  "natrium-pin-refund-v1")` (RFC 9497 §3.2.1, P256-SHA256, OPRF mode);
  `refundKey = d·G`, compressed SEC1, 33 bytes, standard base64.
- `POST /v1/evaluate` requires `refundKey`; the answer adds `attemptId`
  (16 bytes, base64, valid 15 minutes). The service's OPRF key is derived
  per refund key (`natrium-recovery-v2|…|<refundKey>`): data sealed before
  cannot be opened (no migration).
- `POST /v1/refund {"attemptId", "signature"}` with the PIN token:
  ECDSA P-256/SHA-256 over `natrium-pin-refund-v1|<domain>|<userId>|<attemptId>`,
  `r||s` 64 bytes base64 → `{"attemptsRemaining"}`; `404 unknown_attempt`,
  `403 invalid_signature`.

## Design

- **D1 — `recovery/receipt.rs`:** the receipt key from the seed (HKDF,
  then `voprf::OprfServer::new_from_seed(…).serialize()` as the P-256
  scalar — `DeriveKeyPair` without voprf's `danger` feature), the
  compressed public key, the message, the signature (RFC 6979). p256 gains
  its `ecdsa` feature (already compiled in through openmls).
- **D2 — recovery data v3:** the header gains the 33-byte refund key
  (header 138, total 186 bytes); v2 is `unsupported` (no migration —
  the service cannot open it any more).
- **D3 — PIN service client:** `evaluate` sends `refundKey` and reads
  `attemptId`; new `refund`.
- **D4 — export:** refund key from the seed → evaluate → seal (key in the
  header) → refund. **Restore:** the header's refund key → evaluate → open
  → the key from the opened seed must equal the header's (else
  `InvalidRecoveryData`), then refund. A wrong PIN sends no receipt (it
  stays counted). A refund is best effort: its failure is logged, never
  the export's or the restore's.
- No API change; BREAKING for users: recovery data made before must be
  made again (`changePin`).

## Tests (red first)

- **KAT, `recovery/receipt.rs`** (the service's `interop.json`, both
  vectors): seed → `refundSeed` (HKDF), → the private scalar
  (`DeriveKeyPair`), → `refundKey` (compressed); the message; the
  service's (random, not low-S) signatures verify with `refundKey`; our
  own receipts verify; an account id without a domain is refused.
- **Format, `recovery/data.rs`:** version 3 — header 138, total 186 bytes,
  the refund key at 105..138, round trip; versions 1 and 2 `unsupported`.
- **Client, `recovery/pin_service.rs`:** `evaluate` sends `refundKey` and
  reads `attemptId`; `refund` posts `{attemptId, signature}` with the PIN
  token and returns the attempts left; `404`/`403` are `BackendRejected`,
  `401` `Unauthorized`.
- **Flow, `recovery/flow.rs`:** the header carries the seed's refund key;
  sealing and opening with the right PIN each send one receipt whose
  signature verifies (checked independently in the mock); a wrong PIN sends
  none; a refused receipt fails nothing; data whose header names another
  receipt key than its seed's is `notRecoveryData`, without a receipt.
- `recovery/oprf.rs`'s vectors follow the service's new file (its key is
  derived per refund key now). `manager::recovery_tests`' mock service
  answers `attemptId` and accepts receipts.
- **Smoke** (test stage, after the joint rollout with the service's
  v0.2.0): export → `attemptsRemaining` unchanged after the receipt;
  restore with the right PIN → unchanged; a wrong PIN → one less.

## As built (2026-10-06)

- `recovery/receipt.rs`: `ReceiptKey::derive` (HKDF through
  `storage::account_keys::derive_from_seed`, then
  `voprf::OprfServer::<NistP256>::new_from_seed(…).serialize()` as the
  scalar — the serialized scalar is zeroized after use), `public_key`
  (compressed), `sign` (RFC 6979, `r ‖ s`); the known-answer tests
  reproduce the service's vectors byte for byte and verify its random
  signatures.
- `recovery/data.rs`: format 3, the refund key after `created_at_ms`.
- `recovery/pin_service.rs`: `refundKey` in, `attemptId` out; `refund`;
  one `service_endpoint` for both paths.
- `recovery/flow.rs`: `evaluate` returns an `Evaluated` (output, answer,
  PIN token); `give_back` sends the receipt and logs a failure; opening
  checks the derived key against the header (`notRecoveryData`) before the
  receipt; the header checks moved into `check_header`.
- `Cargo.toml`: p256 gains `ecdsa` (already compiled in through openmls).
- No public API change; no CLI command (browser only).

## Smoke (2026-10-06, test stage — PIN service 0.2.0, release wasm)

Account 2, server mode, four fresh browser profiles. The PIN service
counts 5 attempts an hour; A's export opened a fresh window.

| step | result | without receipts |
|---|---|---|
| A: login, `enable` (export) | `ENABLE-OK`, 186 bytes (format 3) | |
| B: `recover` with a wrong PIN | `WrongPin`, **4** left | 3 |
| B: the right PIN; C: a wrong PIN | **3** left | 1 |
| C: the right PIN, `changePin` (export); D: a wrong PIN | **2** left | 0 — locked out |

Only the three wrong PINs were counted; the export, both restores and the
second export gave their attempts back. (The right PIN met `HeldElsewhere`
— the previous browser held the databases — after the seed was opened and
the receipt sent; the scenario then takes over.)

**The `SessionExpired` of `open.md` §75 explained:** in the browser the
refresh cookie lives in the browser's own cookie store, Wire rotates it on
every `/access`, and Chrome writes its cookie store to disk late. The
smoke stopped Chrome three seconds after a login, so the next start had the
rotated-away cookie: `POST /access` 403. The first run of this smoke met it
again; with the browser kept 40 s after each login the same flow passed.
A harness artifact — §75 is closed (`docs/LEARNINGS.md` §5.65).
