# Step 112 — Recovery data: the account's seed sealed with a PIN

**Status: MERGED 2026-09-30** — commissioned 2026-09-30 (Markus —
ADR-0035). 112a `26046ae` (PR #91, replaced by #97: `7189c88`), 112b
`7dac2f6` (PR #92, replaced by #98: `828f925`) — restacked on Step 113
before the merge; the replacements carry the same content with corrected
commit authors. CI green. Smoke 2026-09-30 against the test stage's PIN
service (enable; wrong PIN; restore in a second browser with a takeover).
Retrospectives: `docs/LEARNINGS.md` §5.54, §5.55. Archived with this
header.

- **112a** (`step/112a-recovery-data`, stacked on 111d) — the core, tested
  natively: the recovery-data format, sealing and opening the seed, the
  OPRF client, the PIN rules, the local PIN check, the PIN service's
  client and the PIN token from the token exchange.
- **112b** — the browser surface: `PendingLogin.recover`,
  `Client.recovery()` (`state`, `enable`, `changePin`, `verifyPin`),
  `Kupferdraht.validateNewPin`, `pinServiceUrl`, the recovery state in
  `kupferdraht.db`, the embedder guide, the smoke against the PIN service.

## The scheme (ADR-0035; the PIN service's protocol)

1. The PIN: Unicode NFC, then UTF-8.
2. A PIN token from the token exchange (`{"audience": "pin"}`) with the
   Wire access token.
3. OPRF (RFC 9497, mode OPRF, `P256-SHA256`): blind the PIN,
   `POST <pin service>/v1/evaluate` (`keyVersion` omitted when sealing, the
   header's when opening), finalize → `o` (32 bytes). The service limits
   the attempts per user (5 per hour, 12 per day by default) and never sees
   the PIN.
4. `a = Argon2id(pin, salt, m = 64 MiB, t = 3, p = 1)`, a random 16-byte
   salt — a leaked service key still leaves Argon2id per guess.
5. `key = HKDF-SHA256(ikm = o ‖ a, info = "kupferdraht/v1/recovery-data")`.
6. AES-256-GCM seals the 32-byte seed; the header is the associated data.

## Decisions (2026-09-30, Markus)

- **Two sub-steps**, 112a and 112b as above.
- **The header carries a hash of the account id** (SHA-256 of the
  qualified id), not the id (ADR-0035): data of another account is refused
  before the PIN service is asked — no attempt spent — and a found file
  does not name its owner. The PIN service never sees the file; its format
  is Kupferdraht's alone.
- **The format:** compact, versioned bytes the embedder never parses:
  magic `SDRD`, format version `1`, the PIN service's key version, the
  Argon2id parameters, salt, nonce, the account hash — the header — then
  the sealed seed with its tag. About 130 bytes.
- **The PIN is checked locally** for `verifyPin` and `changePin`: a
  verifier `HMAC-SHA256(HKDF(seed, "kupferdraht/v1/pin-verifier"), NFC(pin))`
  kept with the recovery state. It costs no attempt at the PIN service;
  breaking it needs the seed, and whoever holds the seed needs no PIN.
- **The recovery state** (enabled since, the verifier) lives in
  `kupferdraht.db` — in the server mode it travels with the databases, so a
  restored account knows its recovery data exists (112b).
- **Libraries, no own cryptography:** `voprf` 0.5 (RFC 9497) with the
  `p256` crate's `voprf` feature, `argon2`, `unicode-normalization`;
  `aes-gcm`, `hkdf`, `p256` are in the tree already.

## 112a — design

- `recovery` module (tested natively in 112a; 112b wires it into the
  browser):
  - `pin` — `normalize_pin`, `validate_new_pin` → `Ok` / `TooShort` /
    `TooCommon` (at least 6 characters after NFC, counted as characters; a
    short list of trivial PINs refused).
  - `oprf` — blind, finalize (client side of RFC 9497, `P256-SHA256`).
  - `data` — the format: encode, decode (`InvalidRecoveryData`:
    `notRecoveryData`, `unsupported`), the account hash.
  - `seal` — derive the key from `o` and the PIN, seal and open the seed
    (a failed open is `WrongPin`).
  - `verifier` — the local PIN check.
  - `pin_service` — `POST /v1/evaluate`: `401` → `Unauthorized`, `410`
    → `InvalidRecoveryData(unsupported)`, `429` → `TooManyAttempts`
    (`Retry-After`), `503` / unreachable → `ServiceUnavailable`.
  - `flow` — seal a seed for an account and open recovery data for an
    account against the exchange and the PIN service; another account's
    data is `InvalidRecoveryData(otherAccount)` before any request; a
    failed open is `WrongPin` with the service's `attemptsRemaining`.
- `token_exchange` gains the PIN token (`audience: "pin"`, no key).
- `KupferdrahtError` gains `WrongPin { attempts_remaining }`,
  `TooManyAttempts { retry_after_secs }`, `InvalidRecoveryData { reason }`.

## 112a — tests (red first)

- The OPRF against the PIN service's own interop values (both of its
  vectors: the digits and the Unicode PIN).
- The format: round trip; foreign bytes, a wrong magic, a truncated file
  → `notRecoveryData`; another version → `unsupported`; the account hash
  as a known answer.
- Sealing: round trip; another OPRF output (a wrong PIN) → `WrongPin`; a
  changed header byte refuses; the production Argon2id parameters.
- The PIN rules: NFC equivalence, characters not bytes, too short, too
  common, fine.
- The verifier: a known answer computed independently; another PIN,
  another seed differ.
- The PIN service's client and the PIN token (httpmock): request shape
  and every answer.
- The flow against a mock PIN service that evaluates with a known key:
  seal then open; a wrong PIN → `WrongPin` with the attempts left;
  another account's data → `otherAccount` without a request; the header's
  key version sent on open.

## 112a — as built (2026-09-30)

- The OPRF matches the PIN service bit for bit on both of its interop
  exchanges; its Unicode vector gives the PIN decomposed and the input
  composed — the NFC step is pinned by it.
- The flow draws the OPRF blind from an injected random source first, then
  salt and nonce: the tests fix the source and compute the mock service's
  answers ahead of time (the mock evaluates with a known key).
- Opening refuses an Argon2id cost above 256 MiB (`unsupported`, before any
  request): the parameters come from the file, and a changed header must
  not exhaust a browser's memory.
- Argon2id and BLAKE2 are optimised in test builds (`[profile.dev.package]`),
  so the tests run the real 64 MiB.
- The module is compiled for tests only; 112b wires it into the browser.

## 112b — design (2026-09-30)

Branch `step/112b-recovery-surface`, stacked on 112a.

- **Configuration:** `storage: { mode: "server", url, tokenExchangeUrl,
  pinServiceUrl }` — `pinServiceUrl` optional; without it the recovery calls
  refuse with `InvalidInput`. The manager keeps the recovery services
  (token exchange + PIN service) beside its keystore and home.
- **The recovery state in `kupferdraht.db`** (migration `V30`): one row —
  `exported_at_ms` and the PIN verifier. `Store` gains
  `save_recovery_state` / `load_recovery_state` (breaking for other
  `Store` implementations). In the server mode it travels with the
  databases, so a restored account knows its recovery data exists.
- **`Client.recovery()`** (ADR-0035) → `Recovery` with:
  - `state()` → `{ state: "disabled" }` or `{ state: "enabled", exportedAt }`;
  - `enable(pin)` → the first recovery data (only while disabled; the PIN
    must pass `validateNewPin`);
  - `changePin(currentPin, newPin)` → new recovery data; the current PIN
    is checked locally (`WrongPin` without attempts);
  - `verifyPin(pin)` → whether it is the PIN (local).
  The PIN token is bought with the client's own session (renewed once on
  `401`). In the local mode `state()` is `disabled` and the rest refuses
  with `InvalidInput` (no recovery data in the local mode).
- **`PendingLogin.recover(data, pin, { takeOver })`** — with `keys`
  `"missing"`: open the data for the logged-in account (the login's
  token), store the seed durably, then the native registration path
  (`registerDevice` with `takeOver`), returning the client. A failure
  before the seed is recovered leaves nothing; `HeldElsewhere` after it
  keeps the seed (`keys` becomes `"present"`), the embedder continues with
  `registerDevice({ takeOver: true })` — no second PIN attempt. `cancel()`
  removes a recovered seed like a created one.
- **`Kupferdraht.validateNewPin(pin)`** → `"ok"` / `"tooShort"` / `"tooCommon"`.
- **Tabs layer:** `PendingLogin.recover`, `Client.recovery` returning a
  `Recovery` handle (`state`, `enable`, `changePin`, `verifyPin`) and
  `Kupferdraht.validateNewPin` mirrored; the parity test learns the `Recovery`
  class.

## 112b — tests (red first)

- Natively (a seed home, the login mocks, a mock token exchange and a mock
  PIN service that evaluates every blinded PIN with a known key):
  - `enable` gives recovery data of the account and the state `enabled`;
    a second `enable` refuses; `verifyPin`; `changePin` with a wrong
    current PIN is `WrongPin`, with the right one new data under the new
    PIN; the state survives the store being reopened.
  - `recover` in a new browser (another home, its keystore empty) with the
    first browser's data brings the same seed back and registers; a wrong
    PIN stores nothing and the pending login stays usable (a first setup
    works after it); `recover` with the seed present refuses; another
    account's data is `otherAccount`.
  - The store's recovery state round-trips.
- Pins: the binding's `recover`, `recovery`, `Recovery`, `validateNewPin`,
  `pinServiceUrl`; the embedder guide.
- Smoke (test stage): enable recovery in browser A, restore it in a fresh
  browser B (its own home id) with the PIN — then B's `resume` / takeover
  against A, the two-home takeover smoke 111d left open.

## 112b — as built (2026-09-30)

- **`cancel()` after a recovery keeps the databases.** A created seed's
  databases are new and go with it; a recovered seed's are the account's —
  on the storage server those of every browser of the account. `cancel`
  forgets a recovered seed and its keys here and leaves the databases
  (`recovered_seed` beside `created_seed`); a test pins it
  (`cancel_after_a_recovery_keeps_the_accounts_databases`). Found while
  implementing: the first-setup `cancel` would have deleted them.
- **Seed creation and recovery share one path:** `install_account_seed`
  (remove what the account has without a seed, store the seed durably);
  `store_seed_durably` replaces `create_seed_durably`.
- **The PIN token is bought with a `WireSession`** — the login's token in
  `recover`, the client's session (renewed once on `401`) in `enable` /
  `changePin` — like the storage token.
- **The recovery errors carry their detail in JavaScript** (`reason`,
  `attemptsRemaining`, `retryAfter` on the thrown `Error`), and the tabs
  layer carries them across the channel (`describeError` / `remoteError`
  copied only `name` and `message` before).
- **A tabs handle reopens itself:** `#resolveTarget` asks the proxy's
  `reopen()` — a `Client` re-resumes, a `Recovery` asks its client again —
  instead of special-casing `Client`. Two browser tests cover the recovery
  calls through a thin tab and a `Recovery` handle across a leader change.
- `validateNewPin` is an instance method of `Kupferdraht` (the tabs layer
  forwards it to the leader like every other call).
- `Store` gains `save_recovery_state` / `load_recovery_state` without
  default bodies (breaking for custom stores, as decided).
- `exportedAt` is milliseconds since the epoch, like `lastActiveMs`.
- Two 111c pins follow the surface: `TakeOverOptions` (shared by `resume`
  and `recover`) and the `storage` declaration with `pinServiceUrl`.
- No CLI command: recovery data is browser-only (the native home keeps
  random keys, no seed).
- **Smoke (2026-09-30, test stage, real PIN service):** browser A (the
  111b profile, logged in again: its copied profile had rotated the
  cookie) `enable` → `ENABLE-OK before=disabled after=enabled bytes=129
  verifyPin=true otherPin=false`. A then holds the account (`hold`); a
  fresh browser B (its own home id) logs in → `keys=missing
  recoveryExpected=true`; a wrong PIN → `WrongPin attemptsRemaining=3`,
  keys still missing; the right PIN → `HeldElsewhere`, keys present; →
  `registerDevice({ takeOver: true })` → `RECOVER-OK deviceReplaced=false
  recovery=enabled` (the recorded device reused, the recovery state came
  with the databases); A → `EVICTED device`. B reloaded → `RESUME-OK`. This
  is also the two-home takeover smoke 111d left open.
