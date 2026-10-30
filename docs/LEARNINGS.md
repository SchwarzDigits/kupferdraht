# Kupferdraht — Session Learnings (2026-04-21 / 2026-04-22)

This document captures what we learned across the first two weeks
of live-testing Kupferdraht against a live Wire v14 backend. It is deliberately
retrospective: what turned out to be wrong, what surprised us,
what the next implementation pass should internalize from day one.
It is not a status report — for the current state of the code,
see `CLAUDE.md` "Current Status" and `docs/observations.md`.

> **Second-pass learnings (Steps 1–6b, 2026-04-23 → 2026-04-28)
> live in [`observations.md`](observations.md) §16 onwards.** The most
> recent entries are the canonical record of every Wire-API
> landmine and openmls quirk discovered while shipping the second
> implementation pass. Read docs/observations.md after this document.

The observations are grouped by category. Each entry states the
original assumption, what actually happened on the live backend, and the
adjustment we made (or still need to make).

---

## 1. Wire API version assumptions

### 1.1 We started on v5; Wire is on v14

We pinned `CLIENT_SUPPORTED = &[5]` because v5 was the version we
had manually read the Kalium source for, and "Wire keeps old
versions alive" made it feel safe. It wasn't.

**What went wrong:**

- Login, `/clients`, `/mls/public-keys`, and every HTTP call we
  exercised works on v5 because Wire's nginx routes them through
  to backends that still handle the v5 shape. That gave us false
  confidence.
- The event stream (`/v5/await`) returned a **generic nginx 403**.
  The versioned route doesn't exist anymore on nginx.
- Falling back to `/await` (unversioned, the v0–v8 path per
  Kalium's `NotificationApiV0`) handshaked successfully, but the
  upstream only delivered **binary frames that didn't decode as
  JSON** and **no message events arrived** even while iOS was
  actively sending.
- v9 introduced `ConsumableNotifications` with per-event
  acknowledgment and a new `/v{N}/events` endpoint. Wire's server
  has effectively migrated to this; the old stream is a nginx-
  level compatibility stub with no usable upstream.

**What to do differently:**

- Pick the **current documented version** (v14 per
  `reference/wire.swagger.json`) as the target. Wire maintains older
  versions for a year or two but features migrate forward; the
  actively-tested server code path is always the current one.
- Accept the consequence: ConsumableNotifications (per-event ack,
  `EventAcknowledgeRequest`, sync markers) is the event-stream
  reality. Don't try to use the v0–v8 model.
- Validate version choice by trying **one endpoint from each
  category** (auth, conversation listing, event stream) before
  building out the HTTP surface, not after.

See observations.md §14 and §15 for the full diagnostic trail.

### 1.2 Wire's MLS ciphersuite is P-256, not the RFC default

We initially set `MLS_CIPHERSUITE =
MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519` (ciphersuite 0x0001)
because it is the RFC 9420 "reference" suite and what openmls's
examples use.

**What went wrong:**

- Every existing Wire conversation's `GroupInfo` reports
  ciphersuite **0x0002**
  (`MLS_128_DHKEMP256_AES128GCM_SHA256_P256`). P-256 presumably
  lets iOS clients back their signing keys with Secure Enclave,
  which only supports NIST curves.
- Our Ed25519-credential KeyPackages could not be claimed into
  P-256 groups; external commits failed with
  `LeafNodeValidation(UnsupportedCredentials)`.
- Discovered only when the first `conversation-rejoin` test hit
  the error — all prior testing (create own group on CS1) hid
  the mismatch because we controlled both sides.

**What to do differently:**

- Fetch `GroupInfo` from any existing conversation **before**
  picking a ciphersuite. The conversation's first 4 bytes after
  `ProtocolVersion=0x0001` are the ciphersuite u16 and they
  dictate what our client has to match.
- Plan for **multi-ciphersuite support** as a follow-on. Wire
  publishes multiple removal keys (`ed25519`,
  `ecdsa_secp256r1_sha256`, …) under `/mls/public-keys`; a
  production-grade client maintains per-suite signature keys and
  KeyPackage pools and picks at runtime based on the target
  conversation's suite.

See CHANGELOG.md §13 and ADR-0008.

### 1.3 RequiredCapabilities expects X509 support

We set the leaf node's capabilities to `[CredentialType::Basic]`
and assumed that was enough because our credential is Basic.

**What went wrong:**

- Wire's e2ei-enrolled groups declare `X509Credential` in their
  `RequiredCapabilities` extension. This means **every member
  must declare they understand X509 leaves**, even if their own
  leaf is Basic.
- External-commit join fails with
  `LeafNodeValidation(UnsupportedCredentials)` because our leaf's
  capabilities list doesn't include `X509`.

**What to do differently:**

- Declare `[Basic, X509]` as supported credential types from day
  one. This is not a claim that we enroll X509 — it's a declaration
  that we can validate a fellow member's X509 leaf.
- Same logic may apply to other group requirements (extensions,
  proposal types). When building our own groups, set
  `RequiredCapabilities` to the smallest intersection of what
  every current and future member can support.

See docs/observations.md references and the Step 2b/1 commit message.

### 1.4 Z-Client is injected from the access token, not set by the client

Wire's backend scopes some endpoints (notably `/mls/commit-bundles`
and `/mls/messages`) to a specific client device. We assumed we
could set `Z-Client: <client_id>` ourselves on the request.

**What went wrong:**

- Wire's **nginx strips** client-supplied `Z-Client` headers on
  the way in. The header visible to the backend comes from a
  different source: nginx itself injects it based on the
  `client_id` claim inside the access token.
- Our `/login` access token carries only the user claim (no
  client, because the client wasn't registered yet at login time).
- `/mls/commit-bundles` rejected our upload with
  `400 "Header Z-Client is required"` even though we were setting
  the header — nginx had stripped it and the token didn't carry
  the claim either.

**What to do differently:**

- After `POST /clients` succeeds, immediately call
  `POST /access?client_id=<id>` with the zuid cookie to re-issue
  the access token **bound to the new client**. Every subsequent
  refresh passes `client_id` along so the token stays scoped.
- Do not trust that HTTP-header pass-through is symmetric between
  Wire's documented API shape and what actually reaches the
  backend — nginx sits in the middle.

See CHANGELOG.md entries for `auth: rebind access token to
client_id after registration`.

### 1.5 Commit-bundle segments must be MlsMessage-framed

The `POST /mls/commit-bundles` endpoint accepts a concatenation of
three TLS-serialized parts. We wrote `commit || group_info ||
welcome?` with `group_info.tls_serialize_detached()` (raw
GroupInfo).

**What went wrong:**

- Wire parses each segment as a full `MlsMessage` envelope
  (`ProtocolVersion || WireFormat || body`). A raw `GroupInfo`
  has no such envelope; Wire's parser reads whatever byte lands
  at the wire-format position and errors out with
  `400 "Unknown ContentType: 31"` (or whatever random byte
  happens to be there).
- openmls provides `impl From<GroupInfo> for MlsMessageOut`; that
  wraps the GroupInfo in the MlsMessage envelope. Using the
  wrapper fixed it.
- The commit and welcome segments happened to come from openmls
  as `MlsMessageOut` already, so they framed correctly — masking
  the bug until we built the GroupInfo segment specially.

**What to do differently:**

- **Every segment of a commit bundle must be MlsMessage-framed,
  without exception.** Enforce this at the helper-function level
  so no callsite can accidentally pass a raw-struct bytes.

### 1.6 WebSocket auth is not HTTP auth

We built the event-stream WebSocket with `Authorization: Bearer`
and `Z-Client` handshake headers, identical to the HTTP API.

**What went wrong:**

- nginx either strips custom headers on WebSocket upgrade, or
  uses a different auth module on the WebSocket path. Bearer
  headers alone yielded `403 Forbidden`.
- The working pattern (found by experiment against the backend and
  confirmed in our tests): pass `?access_token=<token>` as a
  query parameter. (Correction 2026-10-20: the original entry
  credited "Kalium's v0 behavior"; Kalium authenticates its
  WebSocket with a Bearer header via Ktor and never uses the query
  parameter, so that attribution was wrong.) This is the
  JS-compatible channel because
  browser `new WebSocket()` can't set custom headers, so Wire
  standardized on it.
- Keep the `Authorization: Bearer` header too for defensive
  redundancy — some deployments accept it, none reject having
  it alongside the query param.

**What to do differently:**

- Default WebSocket auth goes through the query parameter. Treat
  headers as secondary.
- Do not assume HTTP and WebSocket auth surfaces are identical,
  even when pointed at the same server.

---

## 2. Implementation-order misconceptions

### 2.1 Step 2b (external-commit rejoin) was planned late; reality demands it early

PLAN.md's original "Suggested Order" placed Step 2b *after* Steps
5 and 6: `3 → 4a → 5 → 6 → 2b → 4b → 7 → 8 → 9`. The rationale was
that 2b is a "legacy cleanup" — joining conversations we're
already a member of on some other device.

**What went wrong:**

- A fresh Kupferdraht-CLI client is **never** a member of any existing
  Wire conversation. Our CLI registers a new `client_id` at
  login; the user's iOS-device leaves in every existing group
  are unrelated to our leaf.
- Without Step 2b, Step 5 (send) has no local MLS state to
  encrypt against for any pre-existing conversation. Step 6
  (receive) similarly has no state to decrypt against.
- Live-testing Step 5 and 6 against production-existing
  conversations was therefore **blocked by 2b**, not enabled by
  it.
- We pivoted mid-session: 2b/1 (per-conversation rejoin) landed
  before 5, and the planned 2b/2 (bulk auto-rejoin inside
  `sync()`) was deferred as a production-UX polish.

**What to do differently:**

- In the new plan, put **rejoin/join-existing infrastructure**
  right after account/session setup. A fresh client's first
  real action against an established Wire account is to join
  the groups the user is already a member of on other devices.
- The "forward-message-only" post-compromise-security caveat
  (mentioned in the original Step 2b rationale as "2b alone is
  useless without historical message access") is a MLS property,
  not a 2b flaw. Accepting it is correct.

### 2.2 Step 4 was a monolith; splitting it helped

Original Step 4 bundled: SQLite migration + per-field DEK
encryption + KEK keystore integration + LockedSession vs
UnlockedSession types + AuthMode stubs. Big, scary, deferred-by-
its-own-size.

**What went wrong:**

- Steps 3f (add_members) and 3g (remove_members) landed as code,
  but couldn't be live-tested cross-process because the MLS
  group state lived only in `OpenMlsRustCrypto`'s `MemoryStorage`.
- Each subsequent Step (5, 6) would have hit the same blocker.
- Step 4 didn't block on lack of design; it blocked on lack of a
  *narrow enough chunk* to ship.

**What we learned:**

- Persistent MLS state *alone* (no encryption, no keystore, no
  session-lock typing) is a useful slice. We split Step 4 into
  **4a (persistent state)** and **4b (encryption + session
  lock)**.
- 4a shipped in a day and immediately unblocked 3f/3g live
  testing. 4b remains as the full security-baseline step.

**What to do differently:**

- Design steps as the **smallest slice that unblocks the next
  step's test**. "Right security posture from day one" is a
  plan-document ideal; reality is iterative.
- Document the interim security relaxations explicitly (as we
  did in PLAN.md's "Interim security posture" for 4a) so the
  debt is visible, not hidden.

### 2.3 Over-decomposing Step 3 was right

Step 3 (group creation) was explicitly split into 3a–3g during
planning: credential setup, KeyPackages, external senders,
zero-member create, with-members create, add_members,
remove_members + members. Each sub-step was one commit.

**What worked:**

- Each commit validated one live-against-backend primitive. When
  something broke (ciphersuite mismatch, X509 capability, …),
  we knew exactly which sub-step surfaced it.
- No sub-step took more than 1–2 hours; iteration was fast.

**What to do differently:**

- **Keep the sub-step granularity.** Even when a larger Step
  decomposes into 5+ sub-steps, one commit per sub-step is the
  right unit. The overhead is low (short commit messages) and
  the bisect/rollback value is high.

---

## 3. Rust-specific technical issues

### 3.1 `rusqlite::Connection` is `Send` but not `Sync`

When we swapped `OpenMlsRustCrypto` (whose storage is just a
Mutex-wrapped HashMap, both `Send + Sync`) for
`SqliteStorageProvider<_, rusqlite::Connection>`, the whole
`MlsClient` became `Send + !Sync`.

**What broke:**

- Every async method on `&self` now produces a `!Send` future,
  because `&MlsClient` is `!Send` (borrows of `!Sync` types are
  `!Send`). UniFFI requires `Send` futures.
- `persist_signature(&self).await` inside `MlsClient::new_fresh`
  broke the outer function's future.

**Fix:**

- Moved `persist_signature` from a method to a **free function**
  taking `&Path` and `&SignatureKeyPair` directly. The future
  then holds only `Send + Sync` references.
- Call-sites in `conversations.rs` pattern: load MlsClient after
  the last async boundary, use it synchronously, drop before
  the next await. Owned `MlsClient` crossing an await is fine
  (it's `Send`).

**What to do differently:**

- For types that are `Send + !Sync`, prefer **owned** lifetimes
  across await boundaries, not shared references.
- When touching async trait methods, check `Send` requirements
  of downstream callers first — adding an async method on
  `!Sync` state is a refactor, not a drop-in.

### 3.2 UniFFI `Trait` interfaces need specific Rust shape

`MessageObserver` is a UDL `[Trait]` interface. Rust side must be
`pub trait MessageObserver: Send + Sync` with methods on `&self`
taking owned arguments.

**What worked:**

- Keeping trait methods **synchronous** at the UDL boundary.
  Observers can forward to async work internally but the trait
  surface is sync.
- Argument types as dictionaries (not references) so UniFFI can
  lift into Kotlin/Swift.

### 3.3 `tokio::signal` requires an explicit feature

Our `kupferdraht-cli` `Cargo.toml` had `features = ["macros",
"rt-multi-thread"]`. Adding `tokio::signal::ctrl_c()` for
graceful shutdown of `listen` compiled-error'd with
"could not find `signal` in `tokio`".

**Fix:** add `"signal"` to the feature list.

**What to do differently:**

- Keep an eye on tokio's feature gating. `cargo add tokio
  --features signal` or reading tokio's module docs for the
  gate is a pre-check, not a post-error fix.

### 3.4 `prost-build` needs `protoc`; vendoring solved it

Adding `prost-build` to the `build.rs` failed on a machine
without `protoc`. Solution: `protoc-bin-vendored` as a build-
dep, with `PROTOC` env var set in `build.rs`.

**What to do differently:**

- Any `prost-build` introduction should come with
  `protoc-bin-vendored` from the start. CI and new-contributor
  machines won't have `protoc`.

### 3.5 serde-JSON-error-body captures require read-first

Early HTTP error handlers did `response.json::<ErrorResponse>().await.ok()`
which consumes the body once. If JSON parsing failed, the error
string was empty — we had to debug blind.

**Fix:** read `response.text().await` first into a string, then
attempt JSON parse on that string. Log the raw text in DEBUG.

**What to do differently:**

- Default HTTP-error path: capture raw text first, attempt
  structured parse second, log both.

### 3.6 rust-analyzer lags behind cargo check

During the session, rust-analyzer's diagnostics frequently
contradicted what `cargo check` reported. In every case, cargo
was authoritative — rust-analyzer hadn't re-indexed after the
latest edit.

**What to do differently:**

- Trust `cargo check` / `cargo clippy` over rust-analyzer
  diagnostics when they disagree. Don't chase the
  rust-analyzer warning.

---

## 4. Strategic / architectural learnings

### 4.1 Kalium is a reference trap

PLAN.md v1 had "Kalium prior art" tables on every Step linking
to specific Kalium files and line numbers. The intent: a cheat
sheet for "here's where they solved this, we mirror it."

**Why this was problematic:**

- **Licensing.** Kalium is GPL. Kupferdraht at the time targeted
  Apache-2.0-compatible delivery (ADR-0016 later chose GPL-3.0-only;
  the independence argument stands either way). Deriving code,
  structure, or even naming from Kalium may carry legal risk.
- **Wrong abstractions.** Kalium's architecture is constrained
  by its JVM + CoreCrypto + Ktor + KMP stack. Rust + openmls +
  reqwest + tokio have different idiomatic patterns. Blindly
  mirroring Kalium led us toward JVM-shaped sync code in places.
- **Overly narrow source of truth.** For the Wire *API* (what
  endpoints exist, what JSON shapes), the authoritative source
  is Wire's Swagger (`reference/wire.swagger.json`). Kalium is a
  *consumer* of that API, one step removed from ground truth.

**Decision (this session):**

- **Kalium for API-protocol observation only** — e.g., "what
  does `POST /access` accept for `client_id`?" — where Swagger
  is incomplete.
- **Never Kalium for implementation, structure, or naming.**
- **matrix-rust-sdk (Apache 2.0) as the conceptual model** for
  Kupferdraht's public API. Same domain (E2EE messaging), compatible
  license, widely used, positions Kupferdraht for a future backend
  swap.

See ADR-0008 (v14 as target) and ADR-0009 (matrix-rust-sdk as
model) for the formal records.

### 4.2 Wire's domain vocabulary is an external constraint

Field names that come from Wire's Swagger (`qualified_conversation`,
`key_package_ref`, `mls_public_keys`, …) are **externally
constrained** — Kupferdraht can't rename them without breaking
interoperability. They happen to match Kalium's names too, but
that's because both tools talk to the same backend.

**What to do differently:**

- Keep a clean separation:
  - **Wire Swagger names** live only inside the HTTP layer. Use
    them verbatim in deserializable structs that talk to Wire.
  - **Public API names** (Kupferdraht's UDL, consumer-facing Rust
    API) follow matrix-rust-sdk conventions.
  - **Internal Kupferdraht names** (module structure, private types)
    are our own, matrix-rust-sdk-inspired.
- The HTTP layer is the **only** place the two naming systems
  touch. Mapping happens there, explicitly.

### 4.3 Live testing finds bugs no plan predicts

Every technical learning in Section 1 above was found by running
the CLI against a live backend, not by reading docs or planning.
Each of:
- CS1 vs CS2 ciphersuite
- X509 capability
- Z-Client via token rebind
- MlsMessage wrapping of GroupInfo
- WebSocket auth via query param
- /await unversioned path
- v5 event-stream dead-end

… would have stayed invisible in a pure-design session. They
surfaced because the CLI was exercised end-to-end against
real Wire servers with real iOS counterparts.

**What to do differently:**

- **Every step includes a live-test commit.** No step is "done"
  until a CLI command has round-tripped through the real Wire
  backend.
- The Rust CLI (ADR-0007) is not just a test harness; it's the
  **primary forcing function** for correctness. Keep it primary.
- Discount time estimates for steps that depend on Wire behavior
  we haven't already validated. "Should work per spec" has been
  wrong every time in this session.

---

## 5. Process learnings

### 5.1 Branch-per-step discipline helps; doc-on-main is fine

We established mid-session: every Step gets its own branch
(`step/6-event-stream`), doc/observations updates go to main
directly.

**What worked:**

- Step 6 landed on its branch, got to the v5 dead-end, and
  didn't contaminate main until we explicitly decided to merge
  with the "v5-blocked" closeout commit.
- Cross-cutting doc updates (PLAN.md restructure, observations.md §14)
  on main without a branch-hop were quick and unambiguous.

**Kept as a rule going forward.**

### 5.2 docs/observations.md is where live-test learnings live

We wrote to `docs/observations.md` for every Wire-API surprise, every
security note, every deferred concern. 15 entries by end of
session, covering security, Wire API quirks, deferred work, and
pivot decisions.

**What worked:**

- One file, numbered entries, append-only. Easy to scan for
  "what's open," cross-referenced from commit messages.
- Marking resolved entries with "Fixed (date)" keeps the
  history while signaling closure.

**What to do differently:**

- When an observations entry becomes load-bearing (like observations.md §14 was for
  §15), reference it from ADRs so the decision chain is traceable.

### 5.3 Wire Swagger is the source of truth for API shape

`reference/wire.swagger.json` landed mid-session (user-added). It
immediately became the primary reference for:
- Endpoint discovery
- Request/response schemas
- Status code semantics
- Version differences

Every subsequent Step should cite the Swagger path entry for
the endpoints it uses. See `docs/wire/api-notes.md` for the
extracted view.

### 5.4 Adding a `GenericMessage` field fans out to every struct literal; red-phase stubs must dodge `deny(warnings)`

Two mechanical lessons from Step 60a (the first schema *extension* of
`GenericMessage`, ADR-0014):

- **A new prost field breaks every existing struct literal.** Adding
  `optional TaskPayload task = 1001` required touching nine
  `GenericMessage { .. }` construction sites (builders + tests) to add
  `task: None`. Expected cost, not a design smell — but budget for it
  when extending the envelope, and let the compiler enumerate the
  sites rather than grepping.
- **Tests-first stubs fight `#![deny(warnings)]`.** A red-phase
  `todo!()` stub that nothing outside `#[cfg(test)]` references trips
  `dead_code` in the non-test build and the build dies before the
  failing tests can run. The working pattern: wire minimal
  *declaration glue* (real call sites that compose the stubs and then
  `todo!()`), and demonstrate red with `cargo test -p kupferdraht-core
  --lib` — the full workspace gate only has to pass at the green
  phase.

### 5.5 Rebuild the CLI binary before every smoke — the gate does not

From Step 60c: a post-gate fix (`move_task` bootstrap) went through
fmt/clippy/`cargo test` — none of which refresh
`target/debug/kupferdraht-cli` — and the live smoke then exercised the
*old* binary for twenty minutes. The tell was an error text that no
longer existed in the sources.

- `cargo build --workspace` is part of smoke *preparation*, not just
  the gate; run it after the last code change, before handing out the
  smoke script.
- When a smoke error looks impossible, grep the binary
  (`strings target/debug/kupferdraht-cli | grep "<message>"`) against the
  sources before debugging the code.

### 5.6 Cut decision logic as pure functions at design time, not after review

Step 60c's `move_task` shipped with an unreachable state: referencing
a neighbor required that neighbor to already carry a `sort_key`, and
no call shape existed to create the *first* key — manual ordering
could never start. Unit tests missed it because the guard lived
inside an async `Client` method; only extracting `resolve_move_bounds`
as a pure function (after external review) made the case enumerable
and testable (`(None, None)` = bootstrap). The general form: when an
API takes "references to existing state", write the resolver as a
pure function first and enumerate its empty-state case in tests
before wiring it into the client.

### 5.7 The swagger is not the endpoint's whole truth — api-notes is part of phase-1 orientation

Step 63 built the key-package count call from the pinned swagger,
which does not mark `?ciphersuite=` as required; the live smoke
promptly got `400 Query parameter ciphersuite is required`. The
kicker: `docs/wire/api-notes.md` had documented the parameter all
along ("Kalium uses it to decide when to refill the pool") — the
endpoint's own section was simply not read during orientation. The
charter already names api-notes the live-notes home; the concrete
habit: **for every Wire endpoint a step touches, read its api-notes
section alongside the swagger in phase 1** — the swagger says what
the API schema claims, api-notes says what the backend actually
enforces.

### 5.8 A git-ignored secrets file is only ignored on branches that carry the ignore rule

The Step-62 branch introduced `scripts/smoke.env` (instance URL +
test-user passwords) together with its `.gitignore` entry — but the
entry lived only in that unmerged branch. On the Step-63 branch
(cut from `main`), the same file sat in the working tree unprotected
and ended up staged; only a pre-commit `git status` review caught it
before the credentials reached the public repo. Rules: (1) a secret
file's ignore entry lands on **every** line that can see the file —
in practice: commit the ignore rule on the first branch to merge, or
directly as a one-line chore on main; (2) `git commit` always with
explicit paths after a `git status` read, never from a blind stage.

### 5.9 Fix the defect class, not the instance you were pointed at

The 2026-08 review flagged `notify_message` firing outside the
`inserted` guard in the *text* dispatch path (S6). We fixed exactly
that path — and the re-check found the identical pattern in
`dispatch_inbound_simple` (knock/location/task), two files away.
The habit: when a finding names a pattern, grep for the pattern's
siblings before declaring the class fixed (`grep -rn notify_message`
would have listed both call sites in seconds). A reviewer verifying
the fix will run that grep; run it first.

### 5.10 Ask "who can trigger this path adversarially?" before widening an automatic recovery

Step 64 first made *every* failing `mls-message-add` trigger
discard+rejoin room recovery — reasoned operationally ("a lost
commit should heal now, not on the next message"). The re-check
reasoned adversarially: any group member can post garbage frames,
and garbage in a *healthy* group would have triggered a full
discard+rejoin once per drain — cheap, repeatable epoch churn for
the whole room. The narrowing (heal only on positive
broken-local-state signals: `Storage` / no-local-group) keeps the
operational win and closes the vector. Automatic recovery paths are
attack surface; scope them by evidence of breakage, not by error
occurrence.

### 5.11 A green mock test can pin a stub's dead end — re-audit the pinned contract after every live falsification

Step 14a's tests pinned `create_dm` = "fetch + mirror locally +
sync" with mocks that *listed the conversation unconditionally* —
so the suite stayed green for months while the real server hid
unestablished 1:1s from `list-ids` and every live `create_dm`
dead-ended (observations §66). The mock encoded a hope about the
server contract, and the test then certified the hope. Two habits
from Step 68: (1) when a server contract is the feature, the mock's
shape must come from a live observation (api-notes), not from the
swagger or from what the code under test needs to pass; (2) when a
live smoke falsifies a contract, immediately grep for the tests
that pin the *old* contract — they are now green lies, and updating
them (14a: epoch-1 fixtures now drive the external-join branch) is
part of the fix, not test churn. Related: the first smoke draft
also encoded two wrong assumptions (Welcome absorption; heal
without inbound traffic) — a smoke stage that asserts a path not
under test is a guess, and live runs falsify guesses fast.

### 5.12 A freshly-linked test binary can hang the gate on a macOS keychain prompt — run the suite with `KUPFERDRAHT_DEV_KEYSTORE`

The Step-71 gate stalled for four runs on
`http_hardening::login_rejects_plain_http_url_without_permit_http`
— a pure client-side scheme check that "cannot" be slow. A stack
sample showed the truth: `ClientBuilder::login` →
`install_cookie_label` → `KeyringStore::get_secret` → macOS
keychain, waiting forever for a user-authorization prompt. Every
`kupferdraht-core` lib change relinks the test binaries; a new binary
signature invalidates the keychain grant, and the re-prompt has no
place to appear in a sandboxed/headless gate run. Only the tests
that omit an explicit `.keystore(InMemoryKeyStore)` hit this —
`http_hardening` is the one file that exercises the *default*
keystore path. Fixes that held: (1) run the gate with
`KUPFERDRAHT_DEV_KEYSTORE=<scratch-dir>` (the builder's designed
file-backed dev escape, NG-15 — same trick the smoke scripts
already use); (2) when a test "cannot" be slow but is, `sample
<pid>` beats speculation — the hang was diagnosed in one stack
trace after an hour of scheduling around it.

### 5.13 httpmock pools its servers — `host:port` is not a per-test key; and "green locally" is not "green"

CI was red for three days (Steps 69–71) on one Step-69 test that
passed locally every time. Root cause: the test captured the
client's key-package upload in a `static` keyed by the request's
Host header and looked it up with `find` (first match). httpmock
does not start a fresh server per `MockServer::start()` — it hands
out servers from a **pool** and recycles a finished test's
`host:port` for the next test in the same process. On a many-core
laptop the three tests ran concurrently on three distinct servers;
on the 2-core CI runner the third test landed on a recycled port,
`find` returned the *previous* test's key packages, its Welcome
was built over a foreign key package, and the group never joined
(`MlsGroupMissing` — a misleadingly deep symptom for a fixture
bug). `cargo test -- --test-threads=1` reproduced it locally in
seconds. Two lessons: (1) any per-test state that outlives the
test (statics for fn-pointer matchers) must key on something the
pool cannot recycle, or take the most recent entry — never "the
first for this host"; (2) run new integration tests once with
`--test-threads=1` before declaring green, and **look at the CI
badge after every merge** — nobody had, and three consecutive
"green gate" merges landed on a red main.

Recurred in Step 79 (2026-09-25, CI red on the merge commit): the new
test file's capture statics were keyed by `host:port` — fine in a
parallel run, where every live server has its own port, but under
`--test-threads=1` (and on the CI runner) a later test gets an earlier
test's pooled server back and inherits its captured ciphertexts (`left:
2, right: 1`, then `UnableToDecrypt(AeadError)` on a foreign message).
The local gate had passed because it ran parallel only. Two rules that
would have caught it: run a new test file once with `--test-threads=1`
before the gate (the standing rule since Step 69 — skipped), and any
per-host static a fixture captures into is purged for that host at
fixture setup (a pooled server serves one test at a time, so the purge
is exact).

### 5.14 Before waiting on an "ops flag", read the upstream chart — the knob may not exist

Step 62's smoke sat "blocked on ops" for five days (2026-08-13 →
18) waiting for `consumableNotifications: true`. The flag does exist
in wire-server — but PR #5116 hard-codes it to `false` in the chart
*templates*, so no `values.yaml` change could ever have flipped it;
only Wire could, and the public record says: hard-coded off, no
timeline. Reading
the upstream diff (`gh api repos/wireapp/wire-server/pulls/5116/
files`) took ten minutes and also revealed the real path forward
(the legacy `/await` route carries every push when the flag is
off). Lesson: when a step blocks on an external configuration
change, verify in the upstream source that the change is *possible*
for the party you are waiting on — before waiting. Corollary for
this codebase: the wire-server repo is one `gh api` away; a
behaviour that surprises us live usually has a changelog line.

### 5.15 Mechanical inserts before a `fn` anchor eat the anchor's doc comment

Three times in the Step-72 series the same accident: a scripted
insert placed a new item "before fn X", landing between X's doc
block and X itself — the stray doc merged into the NEW item's docs
(rustdoc then claimed ADR-0014 fallback modes for the case builder,
the exact opposite of ADR-0018), and X went undocumented. Reviews
caught it each time; the third one shipped to the hard stop.
Rule: anchor mechanical inserts on the END of the preceding item (a
closing brace), never on the start of the following `fn`/doc line
— and after any scripted insert, read the seam. Corollary from the
same series: Wire's per-account device cap (~8) is real — every
fresh-home smoke registers devices, and all three test users hit
the cap in two weeks; `login --delete-device` + `device-delete`
clean up, and smoke authors should budget for it.

### 5.16 Phase-7 freshness must grep for the OLD invariant, not just touch the diff's files

Step 73's review found eleven stale doc spots after a phase-7 pass
that had felt thorough. The pattern behind every miss: the pass
updated the homes the *diff* touched (library.md, cli.md prose, the
new api-notes section) but never looked for prose describing the
*old state* elsewhere — ARCHITECTURE §5.3, README, two api-mapping
rows, an `on_typing` rustdoc that still said "pull-only" (stale
since Step 62!), a contradicting api-notes heading two pages above
the new section, rules/0008. Rule: when a step *changes an
invariant* (here: "push = `/v14/events`"), grep the repo for the
old invariant's keywords (`/v14/events`, "pull-only", "retired")
across docs/, rules/, README, and rustdoc — every hit is either
still true or a defect. The diff's file list finds additions;
only the keyword sweep finds contradictions.

### 5.17 A future polled inside `select!` needs a written cancel-safety contract

Step 73's review flagged the legacy frame read as droppable by the
keepalive arm. The claim's mechanism was wrong (`tokio::select!`
commits to the first Ready branch; it never discards a completed
one), but the audit it forced was right: the only thing standing
between `next_legacy_frame` and silent frame loss is that its
single await is the stream read and everything after the message
is consumed runs synchronously to `return`. That property held by
accident, not by contract — one future "answer pings explicitly"
edit inside the read loop would have broken it invisibly, and on
`/await` there is no redelivery. Rule: any future a `select!`
races against a timer gets a `# Cancel safety` doc section
stating the invariant the body must keep; `biased` + data-arm
first is worthwhile determinism but is NOT the protection — a
non-cancel-safe read future loses data with `biased` too.

### 5.18 Smoke with the binary you just built — and verify claims live before designing around them

Two Step-74 lessons in one. (1) The first live smoke ran the STALE
release binary: the implementation had only been debug-built, and
the ad-hoc smoke commands reused `target/release/kupferdraht-cli` from
Step 73 — a full seeding round demonstrated the old behavior. The
scripted smokes never hit this (`smoke-step73.sh` builds first);
ad-hoc smoke sequences must start with `cargo build --release` too.
The accident paid for itself as a same-load A/B (6 commits + 4
rejections vs 3 + 0), but only by luck. (2) The step's scope
halved because the orientation phase re-probed the live claim
before designing: gap-B.25's "permanently wedged rooms / blocked
joins" did not reproduce (the groups had self-healed), so the
"join story for backlogged groups" half of the planned design was
never built. Re-verify a dramatic live finding before sizing a
step around it — the failure you saw may already have resolved
itself.

### 5.19 Hand-over hooks need a stale-sender stage in the smoke

Integration fixtures cannot catch a whole failure class of
"re-broadcast current local state" features: the fixture client is
always fresh (it just folded everything the mock served). Step 75's
invite hook passed all 10 integration tests and then under-delivered
live — the one-shot CLI invited before draining a pending task, so
the late joiner's board was permanently short (MLS has no history;
the hook can only send what the inviter has folded). Any feature
that hands over the sender's CURRENT fold needs a live-smoke stage
where the sender has undrained state, and the freshness expectation
written on the API as a contract (`invite_members` docs; open.md
§68 tracks core-side enforcement).

### 5.20 Live smokes must reuse device slots — fresh homes leak them

`rm -rf $HOME && login` per smoke run registers a new Wire device
every time; Wire caps at 7 per user. Step 75's first run died on
`device limit reached` for test user 1 — the accumulated corpses of
every prior step's smoke. Recovery is painful (map orphaned device
ids to dead local homes, `device-delete` one by one, without
touching real devices like the TUI profiles). Smoke scripts keep
persistent per-user homes and re-login into them (the Step-7e
device-reuse path); fresh rooms per run preserve the test
semantics. `FRESH=1` stays as a deliberate, slot-costing escape
hatch.

### 5.21 Commit messages with backticks go through `-F <file>`

`git commit -m "… \`task mark\` …"` let the shell execute the
backticked text as command substitution — the landed message
contained `plus ;  shows the` where `` `task mark --id <room>` ``
should have been (caught by re-reading `git log -1 --format=%B`,
fixed by amend). Any commit message containing backticks, `$`, or
quotes is written to a scratch file and committed with
`git commit -F <file>` — never inlined through `-m`.

### 5.22 A dependency's migration warning describes THEIR general case — check which path YOU are on

Two Step-76 instances of the same mistake class (a sharper flavor
of §5.18's "verify claims before designing around them"):
(1) openmls 0.9's storage-format warning ("0.8.1 compat needs a
feature") was adopted into the step commissioning as "without it
every installation loses its groups". A negative probe plus a
source read showed the break lives in the serde representation of
a few enums and only manifests in non-self-describing codecs
(bincode) — Kupferdraht's `JsonCodec` was never affected (observations
§73). The plan doc, a manifest comment, and a test doc all carried
the false dramatic claim and needed correction. (2) The step plan
asserted "hpke-rs 0.7 drops libcrux-secrets entirely" from the
crates.io dependency API — which lists DIRECT dependencies only;
the crate stayed in the tree transitively (on the fixed line) and
a red test had encoded the wrong expectation. Upstream changelogs
and registry APIs answer their question, not yours: before
designing a migration step around them, reproduce the claim
against your own configuration (probe build, lockfile, vendored
source).

### 5.23 Validate the sink, not only the source — a second infallible constructor voids source validation

Step 80 hardened `UserId::parse` against path metacharacters and
looked done — until tracing the actual login flow showed the
account identity is built via the INFALLIBLE `UserId::new` from
server-supplied parts, bypassing `parse` entirely; the
malicious-backend path was never covered. The durable fix guards
the SINKS (the two `create_dir_all(account_dir)` materialization
sites refuse path-unsafe ids outright), which covers every present
and future flow regardless of which constructor built the string.
Corollary on layering: the sink guard uses whole-component
semantics (`"uuid@.."` is one harmless directory name — traversal
needs separators) while `parse` is stricter per part; a test case
written on the wrong layer failed and taught the split. When a
validation closes a hole, grep for every OTHER way the same value
is constructed before calling it closed.

### 5.24 A time-based state transition needs three triggers — a live timer, a cycle hook, and a resume sweep

Step 89's ephemeral expiry looked like "spawn a task that sleeps
until the TTL and redacts". Walking the consumer surfaces showed
that alone is wrong on three of them: the one-shot CLI exits before
the task fires; a push-transport consumer has no periodic drain, so a
sync-cycle hook alone would redact hours late; and a restart drops
every in-flight task, so a row whose timer passed while the process
was down would print its body on the first read. The shipped shape
is the union — a per-row `tokio` timer (prompt, native), a sweep at
the end of every dispatch cycle (REST drain AND both push dialects,
folded into the existing Step-74 end-of-cycle hook), and a sweep +
re-arm on `load_rooms_from_store` — with an idempotent, indexed
sweep so duplicate triggers are harmless. Any future "after time T,
local state changes" feature (reminders, drafts expiry, read-marker
decay) should enumerate the same three paths before the first test
is written; the Step-89 test for the resume path
(`overdue_ephemerals_are_swept_on_resume`) is the template.

### 5.25 A new column on a widely-constructed record is 30 mechanical edits — reach for `..Default::default()` in fixtures first

Adding `expires_at_ms` to `TimelineEventRecord` touched 31 struct
literals across core, storage, dispatch, and tests before a single
line of feature code compiled — every site that spelled out all
twelve fields. The handful of fixtures written with
`..TimelineEventRecord::default()` (Step 65 onward) needed nothing.
The record derives `Default` precisely for this; test fixtures and
one-off builders should use the spread, and production sites that
set every field should go through one constructor per path (Step 89
added `Client::outgoing_record` for the three send paths). The next
record column should be a one-file change plus the constructor.

---

### 5.26 A "stay" built on an unreproduced failure is a decision on a rumour — pin the repro before you pay for the workaround

CHANGELOG §52 recorded a > 60 s `PRAGMA key` hang, never root-caused,
and ADR-0013 then carried a vendored OpenSSL into every native build
partly on its strength. Step 88 could not reproduce the hang on the
current SQLite3MC in any configuration — including the exact §52 build
flags — and the other June finding ("header pragma never engages") was
a documented upstream bug fixed a month earlier. Two habits would have
caught both: (1) before a failure becomes a driver in an ADR, it needs
a minimal harness that reproduces it — the harness is then also what
tells you when it is gone; (2) before declaring a vendor feature
"provably broken", search the vendor's issue tracker for the exact
symptom (issue #233 had the diagnosis and the fix). Corollary from the
same step: read the current amalgamation before carrying over build
flags — the `SQLITE_EXTRA_INIT` hook §51 had to add by hand is now
built in, and passing it twice would break initialisation.

### 5.27 Predict each test's colour before the red run — a "red phase" with green tests in it is a labelling defect, not proof

Step 87 wrote three tests for the atomic-migration fix. Only one was
red: the second ("a failed run can be retried once the fault is gone")
was green before the fix too, because the fault model — a migration
that FAILS after an earlier one succeeded — leaves the earlier one
fully committed AND recorded in non-grouped mode, which is retry-safe.
The brick needs the crash BETWEEN a migration's two commits, which no
test can produce without hooking the driver. Habit: before running the
red suite, write next to each test whether it is expected red (pins the
fix), green (pins a preserved property) or green (pins a dependency's
semantics) — then the run confirms the prediction instead of surprising
you, and the module says which test is the guard. Corollary for
storage work: pin a storage invariant at the seam where the fix lives
(`run_migrations` on an in-memory connection, no cipher pragmas) rather
than through the public open path — the test is then engine-agnostic
and a parallel engine-swap step never has to touch it.

### 5.28 A "read it back off the live engine" API costs one round of type surprises — write the read-back test before trusting the pragma's shape

Step 90's swap itself was uneventful: the Step-88 harness had pinned
every mechanic (patch stand-in, pragma order, first-read verification,
in-place fixture rekey), so 770 of 772 tests went green on the first
run after the engine change. The two that stayed red were the ONE new
surface the harness had never exercised: `StorageEngineInfo`, read off
the live connection. SQLite3MC answers `PRAGMA plaintext_header_size`
as TEXT (`"32"`), so `row.get::<_, u32>` failed with `Invalid column
type` — a shape question no amount of reading the engine docs settles
as fast as one failing test does. Two habits follow. (1) When a step
adds a "report what is actually in effect" accessor, its red test is
the cheapest place to learn the engine's answer types — write it first,
read the values as strings, parse deliberately, and treat a parse
failure as runtime data (`KupferdrahtError::Storage`), never a panic or an
assert. (2) A verification harness that goes green on a spike branch
does not cover the production glue you add around it; list the
surfaces the swap adds beyond the harness (here: the engine accessor,
the CLI line, the FFI mirror) and expect exactly those to be where the
first green run stumbles. Bonus, for re-vendoring: the SQLite3MC
release signature is Sigstore keyless (base64-wrapped `.sig` + `.pem`),
verifiable with plain OpenSSL against the embedded certificate —
`PROVENANCE.txt` records which half of the chain was checked, so the
next bump does not have to rediscover the format.

---

### 5.29 "Same engine on every target" is a configuration you pin, not a version you notice — and a browser harness proves persistence across connections, never across sessions

Step 91 put the native SQLite3MC store into the browser. Three
things were cheaper than expected and one was not provable. Cheap:
`sqlite-wasm-rs` 0.6.1 turned out to bundle the byte-identical 2.5.1
amalgamation Kupferdraht vendors — a coincidence of release timing that
would silently drift on the next bump, so the pin is the crate's
`SQLITE_WASM_RS_SOURCE_DIR` pointing at `vendor/sqlite3mc/` in the
root cargo config, plus the engine-identity test on wasm; never "the
versions happen to match". Cheap too: rusqlite 0.37 has no browser
path, but a target arm in the stand-in we already own (a re-export
and two stubs for entry points the wasm engine compiles out) made
rusqlite, refinery and openmls_sqlite_storage run unchanged — read
the sys crate's blocklist against the consumer's `ffi::` symbol set
mechanically before assuming a bump is needed. And cheap: the spike's
first real finding (`PRAGMA journal_mode = WAL` is silently `delete`
on the OPFS VFS, and WAL under exclusive locking traps every later
open) came from one probe test that recorded what the engine
*answered* instead of asserting what the docs promised — the 5.28
habit again. Not provable: reload survival. Every headless run is a
fresh browser profile AND a fresh origin (the test server picks a
random port), so OPFS state cannot outlive a run; the harness pins
persistence across connections and the at-rest bytes, and a
session-level smoke needs a real browser on a fixed origin. Write
that limitation into the test's doc comment the moment you hit it,
or the next reader will spend an hour "fixing" a counter that
correctly reads 1.

---

### 5.30 A browser-level invariant needs a browser-level primitive — and its test can play the other party from inside

Step 92 turned "one tab per account, documented" into an enforced
lease in a day, because the primitive already existed in the platform:
Web Locks are origin-scoped, released by the browser when the holder
dies, and stealable with an observable rejection on the loser's side.
The alternatives on the table — a device per tab, a SharedWorker host,
a lock file — each failed on a browser fact (one cookie jar per origin,
no `SyncAccessHandle` outside dedicated workers, a file that outlives
its owner). Two habits. (1) Before designing coordination, list the
platform's own primitives for exactly that invariant and their failure
behaviour on crash; the one the browser cleans up wins. (2) A lease's
contract has two parties, and the harness runs one worker — so let the
test play the other party with the raw primitive (a second
non-reentrant request in the same agent behaves like another tab), and
let the smoke page play both tabs from one page. The one thing neither
can prove is that a real embedder terminates the evicted worker; that
is a documented contract, and the takeover's bounded wait is what makes
ignoring it fail loudly instead of hanging.

---
### 5.31 An env-var switch has as many activation sites as crates that read it — grep the variable, not the function, before placing a guard

Step 77 was commissioned as "warn once, at activation in
`default_keystore`". The variable it guards, `KUPFERDRAHT_DEV_KEYSTORE`,
is also read by `kupferdraht-cli`'s own `cli_keystore`, which constructs
the `FileKeyStore` directly and never passes through the builder —
the CLI is the main consumer of the escape hatch, so a warn in the
builder alone would have been invisible exactly where it matters.
The fix was to place the warn in the constructor every site shares,
not to add a second warn. Two habits that fall out: (1) when a plan
names the activation point of an env-driven switch, `grep -rn` the
variable across every crate before trusting that point; (2) under the
no-`unsafe` rule a test cannot call `std::env::set_var` (unsafe in
Rust 2024), so keep the env read a one-line wrapper and put the
behaviour to pin — here the warn — in a function the test can reach
without the environment.

---
### 5.32 A queued lock grant cannot be re-taken by the code that needs it — hand it over by stealing, and let the platform's own crash rule be the election

Step 94 built "many tabs, one instance" on the ADR-0023 lease in a
day, but not the way the plan sketched it. The sketch — every tab
queues a `navigator.locks.request`, the grant runs `Kupferdraht.open` —
breaks on a fact §5.30 already recorded for another purpose: a lock is
not reentrant within one agent. The JavaScript that holds the grant
cannot let the Rust open pass its own `ifAvailable` request, and
releasing first hands the lock to the next queued tab before the Rust
request exists (FIFO grant, no gap to re-take). The race-free hand-off
was hiding in the existing takeover: `Kupferdraht.open({ takeOver: true })`
*inside* the grant callback steals the lease from the candidate's own
callback, and because a steal preempts the queue nothing can slip in.
Zero change to the lease code. Three habits. (1) When a plan says
"then X runs inside the grant", check reentrancy and release ordering
of the primitive before designing around it — the hand-off has to be
an operation the primitive defines (here: steal), not a sequence of
two. (2) Read the platform's termination rule before adding liveness
machinery: Web Locks release a dead agent's locks and drop its queued
requests by themselves, so the browser *is* the election and a
heartbeat is a hint at most — the browser test pins promotion on
`worker.terminate()` with no heartbeat involved. (3) A JavaScript
module under test can be loaded by the wasm-bindgen harness as local
snippets even from outside the crate (`module = "/../bindings/…"` is
copied beside the glue, relative imports intact); spike that loading
path first — it decided the whole test design (a JS driver returning
plain outcome objects, Rust asserting) and cost ten minutes to prove.

---

### 5.37 A durable cursor may only move through a source that redelivers — and three Step-97b traps (room-wide typing set, large futures in a long loop, state-keyed timing tests)

B6 was not a coding slip but a design rule: ADR-0019 let the legacy
`/await` socket — best effort, no ack, no redelivery — advance the
same cursor the REST queue pages by. Any frame the socket skipped, and
anything stored between the pre-connect drain and the registration,
fell behind a cursor that had already moved past it. Rule: a
persisted cursor advances only from a source that can hand back
everything before it (the REST queue, an acked `/v14/events` queue); a
best-effort channel may only *trigger* a read from that source
(ADR-0030).

Three traps from the same step:

1. **`TypingEvent.typing_users` is the room's current typing set,
   not the sender.** A test that flattened all events saw `bob` twice
   and looked like a double dispatch; assert per event instead.
2. **Holding a large future across a long-lived `select!` loop trips
   `clippy::large_futures`** (the drain future is ~20 KB). Box the
   `Fuse` that holds it, not the whole loop.
3. **Key mock answers to observable state, not to sleeps.** The
   connect-window test answers `/notifications` by a flag the WS
   handshake callback sets and by the `since` cursor; the coalescing
   test counts drains within a range (2..=3). Eight local runs were
   stable.

Left as is: the held-transient cap (64) is a judgement call; the
"socket ended mid-drain → finish the drain" path has no own test
(cancelling it would lose nothing — the cursor persists per
notification and the reconnect drains again); the live smoke cannot
hit the tens-of-milliseconds connect window on purpose, only the
mock test proves it.

### 5.38 A per-record storage trait leaves atomicity to the caller — and fault-inject with SQLite itself (Step 97c)

`openmls_sqlite_storage` implements openmls's `StorageProvider` one
record per method, each an autocommit statement. Nothing in the API
says "wrap me in a transaction", yet openmls writes a dozen records per
merge and drops the pending commit first — atomicity was always the
application's job. Rule: when a library persists through a trait with
per-record methods, find out where its operations begin and end and
put the transaction boundary there; the trait's generic connection
parameter (`ConnectionRef: Borrow<Connection>`) was the intended hook.

Two cheap, deterministic test tools, no mocking of openmls:

1. **A SQLite trigger is a fault injector.** `CREATE TRIGGER … BEFORE
   INSERT ON openmls_group_data WHEN NEW.data_type = '…' BEGIN SELECT
   RAISE(ABORT, …); END` — created through a second connection to the
   same file — fails exactly one chosen write in the middle of a library
   operation.
2. **`commit_hook` counts transactions.** "Every operation commits once"
   pins the boundary for all operations at once and showed the
   before-state plainly (merge 13, Welcome 12, create 11).

Found on the way: atomicity inside one database does not cover the
step *between* databases — decrypt commits to `mls.db`, the plaintext
reaches `kupferdraht.db` later (open.md §71 B9). And a plan's test sketch
may name a property that is not observable deterministically ("waits,
does not interleave"); replace it with a test of the mechanism that
guarantees it and say so at the approval stop.

### 5.39 `busy_timeout` does not cover a `DEFERRED` read-then-write — and three Step-97d lessons (scope the ADR to today's gap, carry the validation along, observe healing by its first request)

ADR-0024 promised that a second process's write waits out a writer.
Step 85's test proved it for a single-statement write only. The Step-97d
smoke — two `sync` processes on one home — failed one of them with
`database is locked`: the ephemeral sweep opened a `DEFERRED`
transaction, read, then wrote; the other process committed while it
waited, the read snapshot went stale, and SQLite (WAL) failed the
upgrade at once instead of calling the busy handler. Rule: every
transaction that may write starts `IMMEDIATE`; a "the second process
waits" test must include a transaction that reads before it writes.

Three more from the same step:

1. **Scope an ADR to the gap that exists today.** The first ADR-0031
   draft designed for an iOS notification extension (receive-only mode,
   time budget, keychain classes). The reviewer pulled it back: the
   drain and own-commit gaps exist without it. Future platform needs go
   to `open.md`, not into a decision nobody asked for yet.
2. **Moving code moves its input contract.** The config re-fetch moved
   into a follow-up and gained a `debug_assert!` on the property key —
   server data. The Step-95c hostile-input test caught it (§5.36's rule
   again): name the entry check that makes an assertion an invariant,
   or don't assert.
3. **Observe a heal by its first request.** A rejoin starts with
   `GET …/groupinfo`; mocking it with 404 and counting calls made every
   "no rejoin" assertion deterministic, and a failed heal visible.

Left as is: the 60 s abandonment bound is a heuristic; the deferred
`/v14/events` path has no integration test (the feature is disabled on
current wire-servers); a notification with several events that defers
on a later one re-dispatches the earlier ones on retry (MLS ones are
then skipped as stale; Wire notifications normally carry one event);
two processes resuming at the same moment with an aged token still see
Step 85's fail-fast `AccountLocked` from the token refresh.

### 5.40 Commit the irreversible step last — and three Step-97e lessons (a gate needs a fail-closed guard, one event can be two writes, measure without side effects)

Decrypting consumes a key; that is the irreversible step, so it must be
the last to become durable: persist the content, then commit the MLS
state, then the cursor. Every crash point then replays into a correct
state as long as the handlers are idempotent — at-least-once plus
idempotence instead of an outbox. Rule: in a pipeline over two stores,
order the commits so the one that destroys information comes last, and
make everything before it safe to repeat.

Three more from the same step:

1. **A lock discipline needs a guard where it is broken, not only where
   it is kept.** The MLS write gate is taken at sixteen call sites; the
   provider makes any other write fail while a window is open, so a
   forgotten site surfaces as an error instead of a silently rolled-back
   ratchet (nonce reuse). An `assert!` would have crashed consumers; a
   deadlock (a reentrant gate) would have hung them.
2. **"Idempotent per message id" is not enough when one event is two
   writes.** Text + mentions and asset + attachment are two `kupferdraht.db`
   writes; a failure between them replays, the replay sees the row and
   skips the rest (open.md residual). Audit the write count per event,
   not only the dedupe key.
3. **Measure without side effects.** Counting received messages with
   `kupferdraht-cli timeline` between SIGKILLs drained the queue itself (the
   command syncs first) and made every kill look late; counting the
   killed process's own output showed the mid-drain kill.

Step-97d addendum: the pooled-server trap (§5.13) struck again — a
helper copied without its comment took the *first* key-package upload
per host; the CI's scheduling reused a server and two tests failed
there only.

### 5.43 A default must be safe for production; "fast for tests" needs an explicit zero — and pace-test on a paused clock (Step 103)

`SyncSettings::poll_interval: None` meant "no pause", documented as
"primarily useful in tests; production callers should set a value". The
derived `Default` handed exactly that to every consumer who did not read
the doc, and the FFI loop, the most common entry point, inherited it.
Rule: the default is the production behaviour; the test shortcut is an
explicit value (`Some(Duration::ZERO)`). Existing tests did not break —
they already set 1 ms, which is what a test needs anyway.

Pacing is testable without waiting: the FFI loop is generic over its
step, so `#[tokio::test(start_paused = true)]` with a scripted step
measures the pause exactly (`Instant::elapsed()` == 5 s / 7 s / 60 s)
in no wall time. The loops that do real HTTP could only be pinned with
a wall-clock window and a request count (1.5–2.5 s); keeping loop logic
generic over its step is what makes the cheap test possible.

### 5.46 Before a destructive action, ask what the triggering input can prove (Step 106)

Three heal / leave triggers each took a signal at face value: the epoch
number of a message nobody could decrypt, a storage error whose old
meaning ("torn merge") a later step had made impossible, and a Remove
proposal from anyone. None of them could prove what the action assumed
— a missed commit, broken local state, an actual removal. Rule: for
every destructive or group-wide action, name what must be true, then
check whether the trigger is authenticated evidence of it; when it is
not, get the evidence from the party that has it (here the delivery
service's epoch) or wait for the authenticated form (the commit). And
when a step removes a failure mode, re-read the triggers that were
keyed to it.
### 5.45 A "fresh row" flag must come from the same write it describes — and count only the effect under test (Step 105)

Every inbound event decided "fresh or replay" from its first write and
did the second one afterwards. Under at-least-once processing (Step 97e)
that split is a trap: a failure between the two writes leaves the first,
and the replay's duplicate check — correct in itself — then skips the
second forever. Rule: when a duplicate check guards more than one write,
the check and every write it guards are one transaction; the store call
that answers "was it fresh" is the call that writes everything.

Two traps on the way:

1. **A counting test counts everything.** The edit-of-an-edit test
   first expected two `UpdateAt` diffs and passed on the unfixed code —
   the send's own status update was the second one. Measure the delta
   of the step under test (count before, count after), not a total.
2. **Don't truncate the evidence.** The smoke's first read-out cut
   timeline lines at 200 characters and hid the mention suffix, which
   looked exactly like the bug being fixed. Filter lines, not columns,
   when the claim is about a line's tail.
### 5.41 A client-wide timeout is a deadline, not a stall detector — prove it before arguing (Step 98)

`reqwest`'s `ClientBuilder::timeout` runs "from when the request starts
connecting until the response body has finished": it cut off every asset
transfer longer than 30 s, however steadily the bytes flowed. The
reviewer asked whether it was really total or only while waiting — the
answer came from the library's own docs and a red test that streamed a
download past the deadline and saw it cut mid-body. Rule: for streaming
transfers, bound silence (per chunk, per response), not duration; and
for an upload, silence at the sender does not mean silence at the
receiver (kernel send buffers), so there the honest bound is none —
the connection's failure and the caller's cancel.
### 5.42 A derived `Debug` on a secret-carrying type is a leak waiting for one `?value` — and a "nothing leaked" check needs a positive control (Step 101)

No code logged a token field by name, yet every type holding one derived
`Debug`, so any future `debug!(?session)` or an `{:?}` in an error would
have printed it. The login went further and logged the raw `Set-Cookie`
headers "for diagnosis". Rule: a type that carries a secret writes its
own `Debug` (a shared redaction helper keeps it one line per field), and
the test is a sentinel value that must not appear in the output — for
key bytes in their numeric form too.

Two traps:

1. **A log-capture test catches the fixture too.** httpmock logs every
   request it receives, cookies included; the first capture failed on
   the mock server's own lines. Capture Kupferdraht's targets only.
2. **An empty result proves nothing until the check is shown to see.**
   The first smoke ran with `RUST_LOG=kupferdraht_core=debug`, logged no
   debug line at all (the library's tracing target is `kupferdraht`) and
   looked clean. Grep for the line you expect (here "login Set-Cookie
   names") before counting what must be absent.
### 5.44 A synchronous trait over an asynchronous store needs an explicit durability point (Step 104)

`SecureKeyStore` is synchronous because the native keystores are; the
browser keystore can only honour it by answering from memory and writing
to IndexedDB in the background. That is fine for reads and fatal for the
one write that matters: a fresh database key, used to create a database
in the same synchronous call. The trait's shape hid that its contract
("stored") was weaker in the browser than everywhere else. Rule: when an
implementation must weaken a synchronous contract, give the callers an
explicit, awaitable durability point and call it before anything
irreversible depends on the write — here, before the database exists.
The trait could stay unchanged (foreign implementations) because the
durability point lives beside it, not in it.

### 5.54 Take the other side's test values, not your own (Step 112a)

- **Interop vectors beat a round trip.** A seal-then-open round trip
  proves only that Kupferdraht agrees with itself. The PIN service ships the
  values of whole exchanges it computes with its own library; reproducing
  them bit for bit proves Kupferdraht's OPRF client talks to that service.
- **Read the vectors closely.** The service's Unicode vector gives the PIN
  decomposed and the OPRF input composed — the NFC step is not a guess but
  pinned by the other side's data.
- **Slow cryptography in tests:** Argon2id with the real 64 MiB takes many
  seconds unoptimised. A `[profile.dev.package]` override keeps the tests
  on the production parameters instead of a weaker test setting.
- **Inject the randomness a mock must answer to.** The mock PIN service
  cannot compute an answer per request; with the flow's random source fixed
  in the test, the answers are computed ahead of time.

### 5.55 A second way to obtain the same thing inherits the first way's clean-up — check whether it still fits (Step 112b)

- **`cancel` would have deleted the account.** A first setup's `cancel`
  removes the seed *and* the databases — right for a created seed, whose
  databases are new. A recovered seed opens the account's existing
  databases, on the storage server those of every browser of the account;
  reusing the same flag for it would have let "back" on a failed
  registration wipe them. The test that pins it was written only when the
  implementation made the question visible — list the clean-up paths of
  what a new entry point creates while writing the red tests.
- **A detail that crosses a boundary must be carried at every hop.** The
  recovery errors carry `reason` / `attemptsRemaining` / `retryAfter`; the
  binding set them, but the tabs layer rebuilt errors from `name` and
  `message` only. A browser test through a thin tab found nothing because
  none existed — the stub now throws an error with a detail.
- **Generalise the special case you are about to copy.** The tabs failover
  re-resolved only `Client` handles; a second handle kind (`Recovery`)
  became a `reopen()` each proxy provides, not a second `if`.

### 5.56 Model who shares what before predicting an error (Step 114)

- **One seed, one device.** Browsers that hold an account's seed share its
  databases *and* — because a restore brings the recorded device back —
  its Wire device. The plan predicted `NothingStored` for a second
  browser after `purgeDevice`; the smoke showed `SessionExpired`, since
  deleting the shared device ended every browser's session. Drawing the
  ownership (seed → storage subject → databases → recorded device →
  cookies) would have predicted it.
- **Closing without owning.** A live client's database handles are shared
  by the sync loop and the tabs layer's handle table, so "drop it" cannot
  close a database on demand. Swapping the connection for an empty
  in-memory one inside the owner's mutex closes it without an `Option` in
  every accessor — one flag where later calls must be refused.

### 5.47 "In flight" is a claim about one owner — name the owner, and settle every reader (Step 97a, after review)

Step 97a made "a `Sending` row not in flight reads `Failed`" the rule
and kept the in-flight set on the `Client`; its decision text said "in
this process". The review found the gap between the two: two `Client`s
on one store read each other's live sends as failed. Rule: a liveness
claim names its owner precisely (this `Client`, this process, this
device) and states what everyone else sees. Two smaller traps from the
same review: one reader (`find_message_by_id`) bypassed the settling
every other reader applied — a derived status needs one choke point, or
a list of every reader in the review; and a resend path that predated
the in-flight set never joined it — a new invariant must be applied to
the old paths that touch the same rows. A first fix that also mapped
`Failed` to `Sending` while in flight broke Step 79's pin: the failed
first send's own settling `UpdateAt` fires while its guard is still
held.

### 5.57 A keepalive that was never measured may be unnecessary — and an echoed keepalive is a self-inflicted drain (Step 115)

`legacy.rs` has pinged cannon every 25 s since Step 73, because "cannon's idle
timeout closes a silent connection otherwise". Our own `observations.md` §68
recorded the premise as untested: *"not probed: server ping/idle-timeout
behaviour on the legacy route"*. Step 115 needed to know, because a browser
**cannot** send an RFC-6455 Ping — the WebSocket API has no `ping()`, by design,
the user agent owns the connection — so the obvious move was an application text
frame instead.

Both assumptions were wrong. Two browser sockets on `/await`, 392 s:

- The socket that sent **nothing at all** stayed open the whole time — nearly 4x
  the 100 s idle window the Step-73 smoke used. Whether cannon pings us (the
  browser answers Pong invisibly; JS cannot observe control frames) or has no
  idle timeout on this route, the client needs to send nothing.
- The socket sending `"ping"` got **every frame echoed back, 1:1** — 15 sent, 15
  received. In a doorbell design that is a drain per keepalive: the consumer that
  had copied our ping as a text frame was triggering a REST drain every 25 s per
  tab, silently, because a doorbell rings before it parses.

Two things generalise. A comment that says *why* a mechanism exists is not
evidence that it is needed; ours even carried the word "proven", and what was
proven was that pinging works, never that not pinging fails. And a
protocol-level mechanism has no automatic application-level substitute: the
echo made the substitute worse than nothing. When a platform withholds a
primitive, measure whether the thing it guards against actually happens before
building around it.

### 5.58 Rehearse the end state of a merge series before the first merge — textual conflicts are the small part (merging PRs #78–#99, 2026-09-30)

Seventeen open PRs, two stacked chains and six independent ones, merged
in one go. Merged first on a local integration branch (main plus every
PR, conflicts resolved, full gate plus a live smoke), then merged on
GitHub in the same order; main ended bit for bit on the rehearsed tree.

- **The semantic conflicts hid behind clean or doc-only merges.** Step
  109's page gate made every `kupferdraht.db` write take a permit; Step 112b's
  new write (the recovery state) merged without a conflict and without the
  permit. The Step 108 spike's test no longer compiled against the VFS the
  chain pinned, and neither it nor Step 115's browser test was in any CI
  list — both would have merged as dead code. Only building and running the
  combined tree found them.
- **A new test can break an old one by sorting before it.** The wasm test
  runner goes in reverse name order; Step 113's new scenario ran right
  before the hand-off scenario and left the lease held for a moment, which
  CI (slower than a laptop) caught and every local run missed. A scenario
  that asserts a clean start waits for the previous one's release.
- **`git rerere` replays conflict hunks, not the edits beside them.** The
  permit line and a dropped dev-dependency had to be redone by hand on the
  real branches; a tree comparison against the rehearsal proved nothing
  was missed.
- **`git config` in a worktree writes the repository's config.** A
  placeholder identity set for a throwaway worktree authored eight pushed
  commits. Use `git -c user.name=… …` for one command, or set nothing.
- **Know the merge path's permissions before starting:** the `gh` token
  needed the `workflow` scope for PRs that touch `.github/workflows/`.

### 5.59 A flake is a bug until a deterministic test says otherwise (Step 116)

The takeover scenario failed twice on CI and passed on every rerun and
every local run — easy to file as flaky. Reading the assertion instead
("the new leader is `thin` right after it announced itself") pointed at
a message that overtook another, and the deterministic version is cheap:
post the late message yourself. Two traps on the way: a check of the end
state after a pause passed without the fix, because the system recovered
by then — a transient wrong state needs every transition recorded; and
`BroadcastChannel` orders messages per sender only, so any protocol that
learns "who leads" from several senders needs a monotonic stamp and must
use it.

### 5.60 A smoke parameter must reach the code it configures (Step 117)

The first `log=debug` smoke showed only warnings, and the obvious suspect
was the product (release builds stripping debug events, the callsite
cache). A two-minute browser probe test — `init_logging(Debug)`, three
events — proved the product right; the smoke page simply never passed
`&log=` on to its worker's URL, so the worker used the default. Rule: when
a smoke contradicts a unit test, check that the knob reached the code
before suspecting the code. The same run paid for the step at once: the
first warning it forwarded named a real fault (`open.md` §73) that had
been silent in every browser run before.

### 5.61 Check that a smoke exercised its premise before reading its numbers — and a cheap per-pass check may be a safety net (Step 118)

The first idle smoke counted a drain every five seconds and showed a
rename two minutes late; it looked like a failing step. The harness had
logged the browser in with an empty websocket base (`login(api, '', …)`),
so the push socket was never attempted: the run smoked the no-socket
fallback, and the Mac sleeping mid-run distorted it further. Rule: before
reading a smoke's numbers, check that its premise held — here a `WS open`
line in the relay's log. With the socket relayed, the second run showed
the design exactly: room created, renamed and a message, each in the
same second; five silent minutes; then one renewal with one room-list
fetch. Two real faults still came out of the distorted run: the first
drain compared against a cursor the client never had and fetched the
room list twice, and a fresh login revoked the profile's previous cookie
before the killed headless browser had written the new one to disk (wait
before killing it after a login).

The gate caught the other one. Moving the device-baseline check from
"every pass" to "with the room-list refresh" looked like pure savings —
baselines only change when rooms do. But the per-pass check was also the
safety net for a room without a baseline, and a Step-95d test said so.
When a step removes repeated work, ask what else the repetition was
quietly guaranteeing; a local read per pass was never the cost.

### 5.62 Periodic work can carry a guarantee nobody wrote down (Step 119)

Step 118 removed the browser's 2.5 s full pass and with it a write every
pass. Nobody had listed "notice a takeover within seconds" among that
pass's duties, but it was one: the VFS reports a takeover only when a
write meets it, so the eviction showed after ~5 s by accident — and after
Step 118 only at the 5-minute room-list refresh (364 s, found by the
natrium-web SDK, not by our tests or smoke). The fix is an explicit
signal (the VFS's takeover event), not a timer. Rule, extending §5.61:
before removing repeated work, list what each side effect of it guaranteed
— a write, a reconnect, a refresh — and give each guarantee its own
mechanism or an explicit "no longer".

### 5.63 A test harness has its own limits — a pool, a cache (Step 120)

Two failures in Step 120 were the harness's, not the product's. Adding a
third httpmock server per recovery test made the parallel manager suite
hang for good: httpmock serves from a bounded pool, and tests that hold
several servers while waiting for one more wait for each other. Sharing
the exchange's server for the slot query fixed it. And a
warning-capture test, unchanged, failed about once in thirty runs:
`tracing` caches a callsite's interest for every thread, so a parallel
test that hit the callsite with no subscriber can hide the event from the
capture; moving it into its own binary made it deterministic. Rule: when a
suite hangs or a capture comes back empty, check the harness's shared
state (pools, global caches) before the code under test — and give a test
that depends on process-global state a process of its own.

### 5.64 A path no smoke walked keeps its first guess — and so does a comment about data nobody measured (Step 122)

`join_via_link` picked the first cached room whose id ended in the given
domain. Step 8 shipped it with the redeem unsmoked ("needs a second
account"), and Steps 20 and 98b wrapped it for Kotlin, Swift and the
browser without anyone redeeming a link as a user with other rooms —
whose first room of the domain is almost never the link's. Beside it, a
browser doc comment said the domain "comes from the link URL's query" and
the CLI help printed a `https://wire.com/join/<key>/<code>` shape; one
look at an issued link (`https://account.<operator>/conversation-join/?key=…&code=…`)
showed neither was true. The fix needed the Swagger's answer to "which
conversation does this link name" (`GET /conversations/join`), which had
been there all along. Two smaller drifts of the same kind: Step 120's new
`reason` value never reached kupferdraht-tabs' hand-written type union (the
parity test compares names, not values), and the bug report's "the
message is not in `messages()`" did not hold when measured — what was
missing was the signal to read again. Rule: a path exposed without a smoke
says so where it is exposed (plan header, `open.md`), and a statement
about external data (a URL shape, a field's origin) is measured once
before it is written down — a guess repeated in three bindings reads like
a fact.

### 5.65 A smoke that kills the browser tests the browser's disk writes too (Step 124)

Two smoke runs (Steps 123 and 124) met `SessionExpired` on the first
resume after a login; a third passed. The cause was the harness: in the
browser the refresh cookie lives in the browser's own cookie store, Wire
rotates it on every `/access`, and Chrome writes the store to disk late —
the scripts stopped Chrome three seconds after the login, so the restarted
profile held the rotated-away cookie. Kept 40 s, the same flow passed every
time. Before blaming the new dependency (VFS 0.5.0 was the change under
test, and the storage side ruled it out in an hour), ask what else the
harness cut short. Rule: a scripted browser smoke lets the browser live
long enough to persist what the step wrote — or stops it the way a user
would — and a failure that appears only right after a hard stop is first
checked against that.

## 6. What would we do differently from scratch

If we started Kupferdraht over tomorrow with what we know now, the
order would be:

1. **Read the Wire Swagger v14 first.** Map endpoints to our
   planned API surface before writing any code.
2. **Target v14 from day one.** `CLIENT_SUPPORTED = &[14]`.
   Accept ConsumableNotifications as the event model.
3. **Pick P-256 ciphersuite from day one.** Don't start on the
   RFC default.
4. **Declare X509 support in leaf capabilities from day one**
   even when using BasicCredential.
5. **Persistent MLS storage before any test against existing
   conversations.** openmls_sqlite_storage from day one, not
   mid-project.
6. **Access-token rebind in the login flow.** Never ship
   `POST /clients` without the subsequent `/access?client_id=`.
7. **Public API shaped after matrix-rust-sdk.** `Client`,
   `Room`, `Timeline`, `SyncService`, `EncryptionSync` (or
   adjacent names). No Kalium-style scope-nested architecture.
8. **Commit-bundle helper that always emits MlsMessage-framed
   segments.** No raw-struct escape hatch.
9. **HTTP error handler captures raw body text first, parses
   second.** From the first HTTP helper.
10. **Live-test harness (the CLI) ready before Step 1 is
    considered done.** Every step lands a CLI command.

That's about the sum of a session's expensive live debugging
compressed into a ten-point day-zero checklist. The next
implementation pass starts from this checklist.

### 5.32 A storage-format swap must list its smoke fixtures as "existing databases" — and a two-home smoke counts device slots before it logs in

Step 90's "existing developer databases: not migrated — fresh home"
clause was read as "your `~/.kupferdraht`", and nobody re-checked the smoke
fixtures. Two sessions later (Step 86) the bill arrived twice. (1)
Every `~/.kupferdraht-smoke*` home of Steps 68–89 is unreadable now, but the
first failure is not the database: the scripted smokes all `source
smoke.env`, which points the CLI at a **shared `FileKeyStore` dir**
still holding the pre-swap 48-byte blob per account, so even a *fresh*
home dies at login with `DB key material must be 32 bytes (raw key),
got 48` until the keystore dir is swapped too (`KUPFERDRAHT_DEV_KEYSTORE=
<own dir>`). (2) The dead homes' devices still occupy the test user's
slots — user 3 was 7/7 after the one fresh login the step needed —
and freeing a slot is a decision about someone's device, not a smoke
step; the plan's two-device stage stalled on it. Rules: when a step
invalidates a persisted format, enumerate every fixture home **and
keystore dir** the smokes depend on in its "existing databases" clause
and purge their devices in the same session (slots are the scarce
resource, homes are just bytes); and a two-home smoke starts with
`devices` — count first, log in second, because the fresh login you
need for the count may itself take the last slot.


### 5.33 A positional row reader owns every SELECT that feeds it — and "the send fails, everything before it works" is one env var away

Two Step-79 findings, one root each. (1) `read_raw_timeline_row`
reads columns by index; Step 89 appended `expires_at_ms` to it and to
the two SELECTs in `timeline_events.rs` — but not to the task-backfill
query in `tasks.rs` (`load_task_timeline_rows`), which also feeds that
reader. Every V23 backfill of a home with task rows would have failed
with an invalid column index; no test ran that query, and the gate
stayed green for nine days. Rule: when a row reader gains a column,
grep for the reader, not the table — every SELECT that calls it is a
call site — and pin the odd one out with a test that inserts a row and
runs exactly that query (the `tasks.rs` test now does). (2) Producing
a live `Failed` row looked hard because every CLI room command
pre-syncs, so cutting the network kills the sync, not the send. The
library path has no pre-sync: a throwaway program resuming the same
home with `HTTPS_PROXY=http://127.0.0.1:1` fails deterministically at
`POST /v14/mls/messages` after local MLS encryption — a fault injection
that needs no code change and no timing. Its one trap: a helper outside
the workspace is outside the workspace's `[patch.crates-io]`, opens the
SQLite3MC home with plain SQLite, and dies with `unsupported file
format` on the first read — copy the patch section into the helper.

### 5.34 A "missing" pragma may be a dependency default — read it back before recording a residual; and a cross-process harness needs a re-exec child, not a helper binary

Step 85 carried open.md §69's "no `busy_timeout` on any connection"
into its red phase, and the three pins came up **green before the
implementation**: `rusqlite` 0.37 sets `sqlite3_busy_timeout(db, 5000)`
on every connection it opens (observations §84). The §69 grep of
Kupferdraht's own source was right and the conclusion was wrong — a
behaviour can be in effect without a line in our tree. Rule: a
residual about engine behaviour is recorded only after reading the
value back off a live connection (the §5.28 discipline applied to
reviews, not just to new pragmas); the step then pins the value
explicitly so the contract stops depending on a crate default.

Two harness lessons from the same step. (1) Honest cross-process
coverage needs no helper binary: the integration-test binary
re-launches itself (`current_exe()` + `--exact <child test>` +
`--nocapture`), a stdin-gated keystore parks the child inside the
critical section, and the parent asserts the typed refusal and the
holder pid — sixty lines, no workspace member. The one trap: libtest
running single-threaded prints `test <name> ... ` **without a
newline** before the test's own stdout, so the parent must match the
marker with `contains`, never line equality (the first run hung on
exactly that). (2) `flock` binds to the open file description, so two
handles in one process contend like two processes — the in-process
two-thread race test is a real lock test, not a simulation.

And one semantic lesson: a newer persisted timestamp is **not** a
rotation. The just-logged-in client's session row is stamped after the
client object was built and carries the very token that just got the
401; adopting on the stamp alone re-sent the stale token and the
retry failed `Unauthorized`. The signal of another process's refresh
is a *different* access token (the newer stamp only keeps a stale row
— our own failed persist — from winning).

### 5.35 A column nobody reads is a column any writer may clobber — and four Step-93 traps (wasm clippy, shared OPFS origin, validate-before-I/O, live two-home timing)

Step 93 gave `SessionRecord.cookie_label` a meaning (the label a
session was logged in with) fourteen steps after the column was
created — and the first `kupferdraht-cli sync` after a login would have
blanked it: the CLI re-saved the whole row with `cookie_label: None`,
a Step-1 leftover nobody had reason to touch while the column meant
nothing. Found by reading, not by a test (the CLI has no integration
harness). Rule: when a column gains meaning, grep every writer of the
row (`save_session`) in every crate, not just the reader you are
adding, and make partial updates load-then-mutate (the pattern
`token_refresh.rs` already uses) so a writer can only change what it
knows about.

Four traps from the same step. (1) **The CI's wasm32 clippy is part of
the gate whenever a storage/keystore path moves.** The dev
`FileKeyStore` is compiled on wasm too (its tests are not); a helper
extracted for it under `#[cfg(not(wasm32))]` broke the wasm build while
every native check was green. Run the exact `cargo clippy … --target
wasm32-unknown-unknown` line from `ci.yml` locally before the commit —
it is minutes, the CI round trip is not. (2) **wasm integration tests
share one OPFS origin across test files within a browser session.**
`step_91` seeded a keystore id that Step 93 made derivable from
persistent state (`/kupferdraht/home.id`); a test that needs such an id
must read it from the pool or pin it there first — the old assumption
"the pool is fresh" was never true across files, it just never
mattered. (3) **Validate before you resolve.** Moving the home
resolution (which creates `base/` and `home.id`) in front of Step 80's
path-safety refusal made a refused account id leave a home directory
behind; the Step-80 pin ("no directory may be created for a refused
id") caught it at the gate. Input validation goes before any I/O with
side effects, and an existing pin is worth more than a review. (4) **A
live two-home replay gets one direction for free and must wait for the
other.** A home whose token is older than 14 min forces `/access` on
its first `status` — so "second home logs in, first home's next
`/access` succeeds" costs nothing; the reverse direction needs the
first home's re-login *and then* the second home's token to age past
the TTL — schedule that stage, never fake the age (the row is inside
the encrypted database). Count device slots through the login itself:
a fresh login that hits the cap lists the devices and stops without
consuming a slot, which is the "count first" §5.32 asks for without a
resume (a resume would have rotated the very cookie under test).


### 5.36 A rule enforced only by review decays — validate where data enters; and five Step-95 traps (red for the wrong reason, first mock wins, re-read the plan, test the upgrade path, scan with braces)

CLAUDE.md's "Assertions" rule (runtime data is never asserted) has
existed since the 2026-08-13 incident. Step 95c's plan listed seven
known violations; an inventory by operand provenance found **132**,
five of them panicking in release builds from data any room member
controls. A rule nobody checks mechanically erodes one reasonable-looking
`debug_assert!` at a time: every site guarded "its" parameter, and the
parameter's origin was two calls away. The fix that held was not
rewriting 132 assertions but **validating where the data enters** —
the `wire/` decode, the public entry point, the resume load — after
which the assertions below are true code invariants again. Rule: a new
`assert!` / `debug_assert!` on a parameter names, in the same change,
the entry check that makes it an invariant; if there is none, it is a
typed error instead.

Five traps from the same step:

1. **A red test must fail for the named reason.** The "empty removal
   key" test was red-then-green for the wrong reason: its mock body
   omitted the other curves, so decoding failed with `Encoding` before
   the key was ever looked at. Check the red run's panic site or error
   text, not only the variant.
2. **httpmock serves the first matching mock that was created.** A
   hostile variant of a fixture response must be mounted *before* the
   shared login fixture, never after it.
3. **Re-read the plan's recorded decisions before the approval stop.**
   The red-test summary proposed keeping a poisoned MLS mutex
   fail-closed — contradicting plan D4, which the reviewer had already
   accepted (and which the code supports: openmls reloads each group
   from storage per operation). It was caught only by re-reading the
   plan.
4. **Persisted baselines need an upgrade-path test.** Step 95d's tests
   created every device baseline through a join; none started from a
   room that existed before the migration. The live smoke found that
   room's first device change swallowed as its silent baseline. A
   feature that persists state for diffing needs one test that starts
   from a database without it.
5. **Scan with a brace-aware scanner.** A line scanner that treats
   everything after `#[cfg(test)]` as test code skipped eight library
   sites that followed a single-item `#[cfg(test)]` or a mid-file test
   module. Track the item or module the attribute applies to, not "the
   rest of the file".

### 5.49 Cut sub-steps along compile units, not layers — and read the native path before designing a browser variant (Step 110)

- **A sub-step must build on every target by itself.** Step 110 was
  planned as core (110a) and browser binding (110b). The core pieces only
  the browser uses — first setup, `cancel`, `recoveryExpected`, creating
  the seed, removing an account's files — are compiled for wasm and tests
  only (the pattern of `ensure_global_key_durable`); without the binding
  the wasm gate reports them as dead code. The two became one step. Plan a
  split where each half has a consumer on each target.
- **The native code often has the browser case already.** ADR-0035 first
  gave the browser its own device errors (`DeviceRemoved`, cleanup of
  orphaned remote databases). Step 40's device reuse — recorded device,
  replaced device with its history wiped, first registration — covers all
  of it with the seed unchanged; the ADR lost two errors and a branch of
  the user flow. Grep the native login before inventing a browser path.
- **Binding types limit the contract.** `wasm_bindgen::JsError` cannot
  carry a `name`; named errors meant switching the surface to
  `Result<_, JsValue>` (81 mechanical changes). Check what the binding
  type can express before promising a JS contract in an ADR.
- **Test code that only compiles on wasm is invisible to the native
  gate.** The new browser test missed a `JsCast` import; native clippy and
  tests never compile it. Run `scripts/wasm-test.sh` before calling a
  browser test red or green.
- **A derived key closes a durability gap for free.** The device manager
  passes its default provider as a custom one, so
  `ensure_account_keys_durable` skips creating the `kupferdraht.db` key ahead
  (`with_store_key` is false) and the provider makes it synchronously —
  in the browser persisted in the background. In a seed home that key is
  re-derivable from the seed, which is made durable before anything else,
  so a lost write no longer strands a database. Native homes keep the
  behaviour they had.
- **The browser smoke needs the Wire API same-origin.** The browser owns
  the refresh cookie; a local proxy that rewrites its domain to host-only
  and its path to `/` (the natrium showcase's Vite proxy does the same)
  makes the login vertical scriptable in headless Chrome
  (`bindings/web/opfs-smoke/?scenario=login`).

### 5.50 A synchronous open over an asynchronous registration needs an explicit prepare step — and a module worker drops messages it has no handler for yet (Step 111a)

- **Prepare, then open.** `sqlite-remote-vfs` registers asynchronously (it
  starts a worker) while SQLite opens synchronously, and it allows one
  database per registration. The browser storage keeps a per-worker
  registry from path to VFS; every async path that opens a database
  prepares it first (`Kupferdraht.open`, opening an account's storage, `resume`,
  a first setup). A registry keyed by path also answers "does it exist"
  without a synchronous probe open.
- **Delete through the owner of the connection.** Deleting a local VFS
  database with `indexedDB.deleteDatabase` directly would wait behind any
  IndexedDB connection the VFS's worker still holds; its own
  `delete_database` does not — and is synchronous, which kept `cancel`
  synchronous (the red phase had expected otherwise).
- **Port tests by copying, not by recalling.** A carried-over helper
  (the `PROVENANCE.txt` parser) was rewritten from memory and wrong; the
  browser run caught it. Copy the helper, then adapt.
- **Some pins do not survive a change of medium.** OPFS allowed exporting a
  closed database's bytes and asserting "no plaintext at rest"; the VFS's
  block store does not expose them. The guarantee now rests on the VFS's
  own tests with SQLite3MC — recorded, not silently dropped.
- **A module worker drops early messages.** A dedicated module worker that
  sets `onmessage` after a top-level `await` loses what the page posts
  before that line: the scripted login was lost once. A worker that takes
  commands says when it is ready.

### 5.51 A server lease outlives the page that held it — and a new seed must not meet an old key (Step 111b)

- **A reload meets its own lease.** The storage server grants a database
  without a takeover only once the holder's lease expired (30 s by
  default), whoever asks. A killed or reloaded page leaves its lease
  behind: the smoke's resume right after the login run got
  `HeldElsewhere`, 40 s later it worked. Found only live — no test of ours
  runs a lease against a real server. The VFS side made the same instance
  id reacquire its own lease; Step 111d gives each home and account a
  stable id.
- **Forget every derived key with the seed.** The storage-login key is
  stored beside the database keys; `forget_account_secrets` first forgot
  only those. A first setup after a cancelled one would have loaded the
  old login key for the new seed — the new databases under the old
  subject, unreadable with the new seed elsewhere. Any key derived from the
  seed belongs in the same forget.
- **Know which way the error points.** The first server-mode smoke failed
  with `ServiceUnavailable` after the token exchange had admitted the
  user — which located the fault at once: the storage server's origin
  allow-list (a deployment setting). Classifying errors by where they arise
  pays off in the first live run.
- **Synchronous consumers want a cached value.** The VFS asks its token
  source synchronously while SQLite waits; the token therefore lives in a
  cache renewed in the background, and the renewal — not the VFS — carries
  the network calls and their failures.

### 5.52 Check a step against its ADR before building on it — a flag with two meanings hides the gap (Step 111c)

- **The deviation surfaced one layer up.** 111b took the takeover of an
  account's server databases from `Kupferdraht.open({ takeOver })` and opened
  them at login; ADR-0035 had put the takeover on the opening call and
  the open on `registerDevice`. Nothing failed until the tabs layer was
  examined: its lease hand-off always opens with `takeOver: true`, so
  every new leader would have taken another browser's databases, and two
  browsers resuming after an eviction would have taken them from each
  other forever. Re-reading the ADR's API section at the start of a step
  that implements it would have caught it in 111b.
- **One flag, one meaning.** `takeOver` meant "this tab takes the
  origin's instance"; extending it to "and another browser's databases"
  looked natural and was wrong — within one browser a takeover is
  harmless (the other tab's instance is gone), across browsers it is a
  user's decision.
- **A dead leader's last word must not move the view.** The leader's
  eviction notice reaches every tab; sent as an ordinary event it would
  have re-pointed tabs that already follow the next leader back to the
  dead one (the transport treats any event as a sighting). It is a
  message kind of its own.

### 5.53 Question a constraint at its owner before building around it (Step 113)

- **The rule we were working around was the VFS's own.** "After an
  unreachable commit the database must be closed and reopened" looked like
  a law; the first design built a heal task, a backoff, a probe or an
  asynchronous reconnect, live connection swaps and keys kept in memory
  around it. Asked why, the VFS side answered: SQLite does not need it —
  the VFS only does not trust its own blocks. Healing in place at the VFS
  left Kupferdraht a check and a status.
- **Markus's yardstick: every condition makes it more complex.** It was the
  right push; the design shrank twice (a Kupferdraht reopen on use, then the VFS
  healing in place) before a line of the final version was written.
- **Validate the premise before the red phase.** The reopen tests went red
  and green, then out — the design they pinned was dropped. The question
  "why must it be reopened?" belonged in the orientation, not after it.
- **Guard what a guarantee rests on.** In-place healing makes a failed
  commit "not committed" only with a rollback journal; a pin test now fails
  if the browser ever sets `journal_mode` to `MEMORY` or `OFF`.


### 5.48 What the embedder hears is part of a transaction's effect — emit after the commit, before the cursor (Step 109)

Step 109 commits a drained page as a whole (ADR-0037). Six lessons:

1. **A long transaction on a shared connection captures every other
   writer.** `kupferdraht.db` has one connection (and the remote VFS one per
   database). A page transaction held across the drain's awaits would
   have taken in a send's row, a read receipt, a setting — and rolled
   them back with a failing page. Found in orientation, not in a test:
   before holding a transaction across `await`, list who else writes on
   that connection meanwhile. Here: a page gate every write method
   shares and only the page holds exclusively.
2. **Notifications belong to the commit.** Announcing inside the
   transaction tells the embedder of data that may be rolled back, and
   the replay announces it again. Hold them in the page, drop a
   rolled-back notification's share at its savepoint, emit after the
   commit.
3. **…and before the cursor.** The first green build emitted after the
   cursor write. A failed cursor write then replays the page, the replay
   dedupes — and the committed messages are never announced. The Step
   97e test caught it. Order: commit → emit → cursor.
4. **A red test must fail, not hang.** Three tests waited for a
   `commit_page` the old drain never calls; the run hung for ten minutes
   until killed. Every wait on the code under test gets a timeout that
   names what never arrived.
5. **A partial commit moves the cursor to a place the mock never
   served.** After a savepoint rollback the next drain asks for "since
   n2"; the mock only knew n1 and n4 and answered 404. A drain test needs
   the backend's answer for every cursor the code can reach.
6. **Know the replay semantics before asserting them.** Since Step 97e a
   consumed key is `Ok(stale)`, not an error; the red MLS test asserted
   `is_err()`. Assert "not an application message" — the property, not
   the mechanism.

And one tooling note: inserting a helper by script right before
`pub fn lock` separated `lock` from its doc comment; clippy's
`missing_errors_doc` was the only signal. Insert before the doc comment.

### 5.66 A parity test proves a member exists, not that its name is free — and the five-check gate never compiles the browser surface (Step 125)

A one-getter step, and it broke every tab-proxied session before a browser
touched it.

- **A name-for-name mirror can collide with the mirror's own machinery.**
  `Client.handle` was added to the wasm surface and, as ADR-0026 requires,
  to `kupferdraht-tabs`. `HandleProxy` stored a proxy's leader-assigned routing
  id as `this.handle` — so the new getter shadowed that field with a
  getter-only accessor and the constructor's own assignment threw
  `TypeError: setting getter-only property "handle"`. Every
  `restoreLastSession` died. `step_94_tab_proxy_parity.rs` was green
  throughout: it checks that both sides *have* the member, which was true
  and was the problem. The routing id is now `$handle`, as the wire format
  already called it, and a new pin walks the `GETTERS` table asserting no
  proxy assigns `this.<getter> =`. A test that checks presence should be
  asked what it would say about absence of conflict.
- **The gate does not compile the code a browser step changes.** `pub mod
  wasm` is `#[cfg(target_arch = "wasm32")]`, so `cargo build`, `cargo
  clippy --workspace --all-targets` and `cargo test --workspace` — four of
  the five gate commands — never type-checked the new getter. All five were
  green before the browser ran. CI knows this and has a wasm32 job (whose
  own comment records a non-exhaustive match "red for weeks" for exactly
  this reason); `CLAUDE.md`'s gate section did not mention it. The commit
  that follows this step adds them: the five stay the five, and a step
  touching `src/wasm/` or `bindings/web/` adds `cargo check` and
  `cargo clippy --lib` for `wasm32-unknown-unknown` plus
  `scripts/wasm-test.sh`.
- **The embedder's smoke is the one that answers the question.** The value
  itself cannot be pinned in CI (no wasm execution), so it was read through
  an embedder's showcase app against the live account — engine getter and
  the `GET /self` it replaces, side by side, same string. Reading only the new
  path would have proven that *something* produced a handle.

### 5.67 A rename has three layers, and the tests know which one you touched (rename to Kupferdraht, ADR-0041)

- **Count the layers before the sed.** The name sat in branding (crates,
  namespaces, module names), in the consumer API (`Sodium*` types,
  `SODIUM_*`) and in persisted identifiers (HKDF labels, file names,
  keystore ids, the keyring service, a Web Locks lease, a cookie label).
  Only the third layer forced a decision — clean cut or compatible — and
  it had to be taken before the first substitution, because the two
  variants are different edit sets. A mechanical rename also decides
  names it was never asked about: 25 doc mentions of the TUI read
  `kupferdraht-tui` before anyone had chosen that name. List the external
  names (sibling repos, products) and decide them first.
- **The failing tests were the real content of the change.** Nine of 1289
  went red, and every one named a consequence of the clean cut: five
  known-answer vectors tied to the renamed HKDF labels, a cookie-label
  length, a keystore id that grew five bytes past the 255-byte file-name
  budget (the documented domain cap moved from 158 to 153), the migration
  digests and a committed fixture whose refinery checksums pin the shipped
  SQL byte for byte. The vectors were recomputed with an independent
  HKDF-SHA-256 that first reproduced all five old answers; the migrations
  and the fixture kept their bytes. Before a global substitution, grep the
  things a checksum protects: `*.sql`, `tests/fixtures/`, known-answer
  tables.
- **A longer name changes formatting, and formatting changes lints.**
  `KupferdrahtError` pushed four one-line match arms over the width;
  rustfmt turned them into blocks, and `semicolon_if_nothing_returned`
  fired on the block form. Harmless, but invisible until clippy ran.
- **Check which compiler the gate ran on.** This machine had Homebrew's
  Rust 1.99 first on PATH and no rustup toolchain; `rust-toolchain.toml`
  pins 1.98, which only rustup honours. Clippy 1.99 was red with 111
  errors on `main` itself. Even with 1.98 installed, `rustup run 1.98
  cargo clippy` picked Homebrew's `cargo-clippy` driver from PATH. The
  gate is only a gate on the pinned toolchain: print `cargo --version`
  and `cargo clippy --version` at the top of the log.
- **Script the rename on the first day, not when the first PR lands on
  top of it.** A colleague's PR merged mid-way; because every manual
  edit had been captured as a script (the `git mv` list, the perl line
  with its exclusions, the vectors, the two entries), the branch was
  rebuilt on the merged `main` in minutes and differed from the first
  attempt by exactly the PR's six files. An open branch of someone else
  takes the same line first and then rebases; the conflicts shrink to
  real overlaps.

### 5.68 A claim is only as strong as the record behind it (release preparation, 2026-10-07/08)

- **State the claim as practised, not as wished.** The charter said
  "from Swagger + observed behaviour only" and three ADRs said
  "clean-room", while about twenty-five places in `docs/` cited Wire's
  source at file:line, as the observation rule of ADR-0009 permits. The
  cure was not to thin the record but to rewrite the sentence: no Wire
  code in the tree, Wire's repositories read for protocol observation,
  every observation recorded with its source. A weaker sentence that is
  true outlasts a stronger one that is not. Two outside reviews then
  caught two fresh overclaims in sentences written during this very
  cleanup ("nothing of it is reproduced here", "every such observation
  is cited"): absolute words invite the counterexample, so write "only
  identifiers and constants are quoted" and name the five repositories.
- **Generated artifacts rot silently.** The third-party license report
  still listed two GPL crates from `wireapp/proteus` five months after
  Step 9 removed them; no gate regenerates or checks it. Either the
  release step regenerates it, or CI diffs its crate list against
  `Cargo.lock`.
- **A fixture generated from live data carries that data forever.** The
  author's own account on a hosted backend was the test identity in seventeen source
  files, and two openmls 0.8.1 storage fixtures bind their persisted
  keypairs to it, so the swap to synthetic identities stops at those two
  (the pin and the fixtures were retired on 2026-10-08). Choose synthetic
  identities on day one; a fixture is
  the one place a later rename cannot reach.
- **The example output is part of the example.** The examples were
  switched to a self-hosted backend while the output blocks beneath them
  still showed a hosted backend and real handles; a reviewer saw the
  contradiction, the author of the change did not. Read a reference as a
  reader would, block by block.
- **The toolchain trap, measured again.** `rustup run 1.98 cargo …`
  resolves rustc and cargo-clippy from Homebrew on this machine, so a
  deprecation that exists only on 1.99 was diagnosed as a 1.98.1 finding
  and fixed in a commit that had to be reverted. Only prepending the
  toolchain's `bin` directory gives a true pinned run; before trusting a
  red, print `cargo --version`, `cargo clippy --version` and the `rustc`
  cargo will find.
- **The likeliest complaint was never copyright.** The README logged in
  against Wire's hosted service two paragraphs below the Terms-of-Use
  note. The fix was documentation, and the strongest argument,
  that the CLI has no default backend and refuses to run without an
  explicit target, had been in the code all along, unstated.

- **A provenance header is a claim about history; check it against the
  first commit.** The proto header said layout and comments were
  Kupferdraht's own; the file's first version said "we copy only the
  variants we use" from Kalium's `messages.proto`, and the field order
  of `Asset.RemoteData` still showed it five months later. The cure was
  not a better sentence but the GPL's own mechanism: Wire's notice kept
  intact, a modification notice with holder and year (GPL-3.0 §5(a)),
  and one line in the charter that names the single Wire-authored
  artifact in the tree. The same pass retired the dates and process
  narrative from the comments of the shipped core and wrote the rule
  down (CLAUDE.md "Comments in Shipped Code"): a comment describes the
  code and names the decision, the dated record lives in
  `docs/observations.md`.
- **Check a rule's premise against the artefact it judges.** The comment
  convention first banned `6-II-d`-style codes as a "retired numbering
  scheme"; the archived plan is titled Step 6-II and names its sub-steps
  exactly so, only the filename spells `-2`. Purging them would have cut
  about a hundred valid pointers. The rule now says what is true: a
  sub-step code is a pointer like a step number, and only the dated
  narrative and the "V1" wording go.
