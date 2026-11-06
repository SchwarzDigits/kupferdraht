# Open — Questions & Backlog

Everything still on the table. Two kinds of entry: **open questions /
unknowns** (the numbered §N findings log below) and **backlog** (decided work
not yet shipped — folded in 2026-06-30 from the former `docs/v1-*-audit.md`).
Decided scope cuts are not open: they live in `docs/non-goals.md` (moved there
2026-11-01). When an item resolves it leaves here — as a permanent Wire/MLS
behaviour fact → `docs/observations.md`, as a shipped change → `CHANGELOG.md`,
as a decision not to do something → `docs/non-goals.md`. Step-sequenced
delivery lives in `PLAN.md`.

## 60) Step 29 decomposition leaves three client/ files over the 200-LoC cap — finer split deferred to a later step (2026-06-16)

Step 29 split the 1741-code-line `client/runtime.rs` monolith into ten
cohesive modules (`mod`, `session`, `profile`, `http_calls`, `sync`,
`rooms`, `membership`, `timeline_read`, `mentions`, `typing`). Three of
the resulting files still exceed the CLAUDE.md 200-code-line cap while
hosting more than one responsibility:

- `client/membership.rs` (426): invite/kick · join/resync · member-list
  sync · commit-bundle/key-package helpers
- `client/rooms.rs` (361): list accessors · create/DM · metadata ·
  join-links
- `client/sync.rs` (237): sync loop + handlers · room-cache persistence

The finer split (the plan's conditional "29g") is **deferred to a
future, separate step** — per the Step 29 decision it is explicitly NOT
part of Step 29, whose scope was eliminating the `runtime.rs` monolith.
Proposed boundaries for that later step: `sync` → `sync` + `sync_cache`;
`rooms` → `rooms` + `rooms_create` + `rooms_links`; `membership` →
`membership` + `membership_join` + `membership_members` (each lands well
under 200).

Pre-existing over-cap files outside Step 29's scope: `login_pipeline.rs`
(391), `builder.rs` (236), `resume.rs` (221).

> **Re-count 2026-07-16** (docs catch-up audit; raw `wc -l` totals
> incl. doc-comments — and for `login_pipeline.rs` / `builder.rs` /
> `resume.rs` incl. their `#[cfg(test)]` modules, so subtract tests
> before comparing against the 200-code-line cap): the window's
> SSO + wasm work grew every file on this list by 60–100% —
> `membership.rs` 709, `rooms.rs` 760, `sync.rs` 456,
> `login_pipeline.rs` 809, `builder.rs` 707, `resume.rs` 488. The split
> boundaries proposed above predate that growth — re-derive them when
> the step is picked up.

---

## Backlog — decided work, not yet shipped

> Folded in 2026-06-30 from the former `docs/v1-*-audit.md` trackers; checked
> against the code on 2026-11-01. Gaps and improvements that are decided but
> unbuilt — none blocking, all quality. Step-sequenced delivery still goes
> through `PLAN.md`; an item graduates into a numbered Step when picked up.
> Candidates that are not decided yet live in §64, not here.

### Caller ergonomics

Two items. The rest of the June audit's wish list left on 2026-11-01: no
consumer asked for it, the `Option` accessors mirror matrix-rust-sdk by
ADR-0009, the two-step login is by design (the device cap asks the user),
and the CLI is a thin wrapper for live testing (`docs/cli.md`).

- **erg-C** three error messages mislead or give no hint: `pipeline.rs` "has no group_id — run sync_metadata first" (the fix is to join; mirror the typed `KupferdrahtError::MlsGroupMissing` text), `pipeline.rs` "{op}: {room_id} not in cache" (add "run sync / load_rooms_from_store"), `sqlite/mod.rs` "storage column … not valid UTF-8" (add a corruption / recovery hint).
- **erg-D.11** `kupferdraht-cli doctor` — a health check for live testing: session valid, device registered, MLS state present, key-package pool depth, store path, last sync, `file_sharing` / `receipt_mode`. Absorbs the former `status --verbose` idea.

### Live verification backlog

Facts no unit test can establish, each waiting for a smoke session on the
test stage (httpmock cannot stand in for openmls or for an official
client). Work through them in the next session that is on the stage
anyway; a verified item leaves here (the fact goes to `docs/observations.md`).

- **gap-B.14** an inbound asset from Wire Web to Kupferdraht;
- **gap-B.15** the `WIRE_RECEIPT_MODE` off path against a live backend;
- **gap-B.13** a live `/feature-configs` fixture captured (the tests use a synthesised one);
- **gap-B.7 residual** the low-pool live drain of the key-package top-up (Step 63) — needs a claim source, i.e. a second device (observations §66);
- **gap-B.36** the SSO login (ADR-0043) end to end against a backend with a configured identity provider; four assumptions pinned by tests (`docs/wire/api-notes.md` §2b: unversioned `/sso/*` paths, the cookie as a query parameter on the success redirect, `POST /access` from a bare `zuid`, `/sso/settings` as the discovery source);
- **gap-B.37** whether the backend itself refuses an MLS message for a future epoch (the Step-106 healing does not depend on it; robustness, not security).

### Code quality / maintainability

Correct code that is hard to read / navigate. The baseline safety rules (no
`unsafe` / `unwrap` / `println!` / `macro_rules!`; clean build / clippy /
`cargo deny`) are re-checked each gate run — not tracked here.

- **cq-A.1** Files over the 200-code-line cap (CLAUDE.md): 58 on 2026-11-01, counted without tests, blank lines and comments; the largest are `wasm/manager.rs`, `sync/service.rs`, `storage/sqlite/mod.rs`, `storage/records.rs` and `ffi/timeline.rs`, and the three Step-29 residues of §60 are among them. Re-count before acting; split by responsibility, not by size.
- **cq-A.2** `process_inbound_message` — forbidden `process` prefix. `MlsClient::process_inbound_message` (`encryption/messages.rs`) decodes / absorbs an inbound MLS message → name it for the effect (`decode_inbound_message` / `absorb_inbound_message`).
- **cq-A.3** Three `#[allow(clippy::struct_field_names)]` over renamable fields: `rooms/room.rs` (`room_type`→`kind`), `client/mod.rs` (`mls_client:`→`mls`), `ffi/sync.rs` (FFI `Message` `message_*` fields).
- **cq-A.4** Redundant `_with_storage` suffix on `MlsClient::new_fresh_with_storage` / `resume_with_storage` — pure rename (the in-memory `new_fresh` wraps the `_with_storage` variant; not dead).
- **cq-B.1** Magic byte arrays in fixtures (`vec![0xNN; N]`, no shared names) — introduce named `const`s.
- **cq-B.2** No property-based tests (`proptest` / `quickcheck`) and no fuzzing — crypto + protobuf round-trips and the frame parsers are candidates.
- **cq-B.3** No benchmarks (`criterion`) — add when perf becomes a question.

### Blocked upstream (Wire API, openmls)

- **Step 62 — `/v14/events` push (dormant):** wire-server
  hard-codes `consumableNotifications: false` since chart 5.30 (PR
  #5116 hard-codes the flag, no public timeline — observations
  §68). The transport is merged + archived as opt-in, self-guarded by
  `server_supports_push_events()`. **Re-check** on every wire-server
  upgrade of the own instance: `GET /v14/feature-configs →
  consumableNotifications.status`; when `enabled`, run the archived
  `smoke-step62` flow (its 7 stages are documented in
  `docs/plan/archive/62-events-websocket.md`).
- **gap-B.22 — openmls has no staged external join:** the
  `ExternalCommitBuilder` chain persists **and** merges the group
  atomically in `finalize`, so an external commit cannot be built,
  uploaded and merged only on server acceptance (the add / remove path's
  `upload_and_merge_commit_bundle` pattern). Step 66 ships
  discard-on-upload-failure instead (decided 2026-08-13); still not
  shipped in openmls 0.9.0. To do: file the upstream issue (a
  `StagedExternalJoin` that defers the storage merge); revisit Step 66's
  residual crash window if it ships.

## 64) Chat-system basics — the post-Step-60 gap map (2026-07-19; re-cut 2026-11-01)

Roadmap discussion after tasks shipped: what a daily-driver messenger
still needs. Backlog candidates, grouped; PLAN.md steps get generated
from here with the user's OK. Items already tracked elsewhere are
referenced, not duplicated. Re-cut 2026-11-01: contact discovery and
direct contact initiation are non-goals by Kupferdraht's purpose
(`docs/non-goals.md` NG-18); push registration, new-device history and
presence are recorded there too (NG-19 to NG-21); link previews
contradicted NG-4 and are gone. The self-room follow-ups of the Step 86
retrospective are Step 129 (`LastRead`, personal task list) since
2026-11-01; draft sync stays here.

### Conversation daily life

- **Mute / archive** — Wire carries `muted_status` / `archived` per
  conversation; small step, high everyday value.
- **Drafts** — per-room draft persistence (client-local first;
  Wire-style sync rides the self-conversation, the rail Step 129
  builds). Waits for a consumer that has drafts.
- **Local search** — under E2EE only the client can search;
  SQLCipher + FTS5 native, own path needed for IndexedDB. At a few
  thousand messages this stops being a luxury.

### Dependency lines

- **Kupferdraht's own RustCrypto crates to the 0.11 / 0.13 line** —
  `aes-gcm` 0.10, `hmac` and `hkdf` 0.12, `sha2` 0.10 and `ed25519-dalek`
  2 sit beside the 0.11 / 0.13 / 3 line the openmls fork brought with
  Step 128, so `cargo deny` warns about duplicate families (`aes`,
  `aes-gcm`, `aead`, `cipher`, `ed25519-dalek`, …). Moving them touches
  receipt, prekey and PIN code that has known-answer tests, with no
  advisory behind it; a candidate, not a step (decided 2026-11-02).

### Device trust

- **Device verification UX** — MLS gives cryptographic identity; since
  Step 95d a room's devices and their MLS thumbprints are readable
  (`Room::devices`, `subscribe_device_changes`), but there is no
  verification / trust state yet.

### Group governance (second tier)

- **Conversation roles** — *narrowed by Step 72 (2026-08-21):*
  roles ARE decoded, persisted (`room_members.role`, V26), merged
  through `sync_members`, and honored where authorization exists
  (the ADR-0018 case gate, `[admin]` in `room-members`). Still
  open: invite/kick are role-blind — deliberately so far, since
  Wire itself lets any member invite (observations §72); gating
  them would be a Kupferdraht-side policy decision, not parity.
- **Room avatars** — absent.

### Priority thesis

1. **Mute/archive** — daily comfort.
2. **Local search** as a filler between steps.

## 66) External code review triaged — 18 findings verified, dispositions assigned (2026-08-13)

Of the 18 findings one is still open: **S11** (task-cache
monotonicity) is a design question — ADR-0014's last-writer-wins means
*event order*; a wall-clock guard would change semantics; revisit if
cursor-reset replays become a real flow. (The dispositions of the
others: `docs/plan/archive/`, `CHANGELOG.md`.)


## 68) `invite_members` freshness is a contract, not enforced (2026-08-26)

The ADR-0020 invite hook hands the invitees this client's *current*
fold of case/marker/tasks — state the inviter has not drained is
permanently missing for the joiner (MLS has no history). Today that is
a documented contract on `invite_members` (the CLI drains first;
push-loop consumers are always fresh). Open decision: should
`invite_members` drain notifications itself (core-side enforcement —
costs a sync round-trip on every invite, duplicates the consumer's
loop) or stay a documented contract? Revisit if a second consumer
trips over it.

## 69) Kalium-audit residuals — checked, accepted, not commissioned (2026-09-10)

The Kalium-JVM production-readiness audit was cross-checked against
Kupferdraht (Steps 77–80 commissioned from the real findings; the
blocker class does not transfer — one Rust core, SQLCipher
everywhere, calling out of scope). Two minors were verified and
deliberately left as residuals rather than steps:

- **DB files carry umask default modes**, not 0600 — accepted on
  2026-09-10 as defense-in-depth only (ciphertext, keys in the OS
  keystore) for whichever step next touches the open paths; that is
  Step 134 (`docs/plan/134-db-key-rotation.md`, transferred 2026-11-02).
- **Windows `FileKeyStore` writes without ACLs** (the
  `cfg(not(unix))` arm is plain `fs::write`). Accepted: the file
  keystore is a dev-only escape hatch (Step 77 makes it loud and
  atomic; there is no Windows CI to regress against anyway — the
  CI-matrix gap itself is pinned in the 20e support-tier ADR
  precondition).

*2026-10-08, decided (Volker):* the findings stay named in the public
repository as written here and in Steps 77–80; they are documented in
Kalium's own pull requests and were the checklist, not the source, of
Kupferdraht's hardening.

## 72) Browser server mode — residuals of Step 111b (2026-09-30)

What Step 111b (ADR-0035, the account databases on a storage server) left
open, none of it blocking the step:

- **A long pending login.** Until `registerDevice` returns, the storage
  token is renewed with the login's Wire token, which expires after
  15 minutes; a login left pending longer retries the renewal every minute
  until the client exists. The first renewal is due after half the
  storage token's lifetime (30 minutes), so this only matters for a login
  left open that long.
- **A purge needs a live client in the browser (Step 114).**
  `logout({ purgeLocal })` runs on a `Client`: an account whose session
  expired is logged in again (its keys are here), then logged out with the
  purge. Natively Step 78 added `Kupferdraht::forget_account(account_id)`
  for exactly this; the browser gets no `forgetAccount` yet — adding it
  means a tabs-proxy mirror (Step 94's parity) and a browser test, so it
  waits for an embedder that needs it.

## 74) Guest links of another operator (2026-10-05)

A guest link carries only `key` and `code`
(`https://account.<operator>/conversation-join/?key=…&code=…`, measured on
the test stage); its host is the operator's account page, not the
conversation's domain. Every code endpoint of the v14 client API is
unqualified (`POST`/`GET /conversations/join`, `code-check`,
`/conversations/{cnv}/code`): a redeem asks the user's own backend, which
knows only its own codes. So Kupferdraht redeems links of its own operator only
(Step 122 finds the room by the conversation id the link names); a link of
another operator fails with `404 no-conversation-code`. Open (Markus): is
there a federated way to join through another operator's link? Measure
with an account on a backend federated with the test stage before
deciding anything.

## 77) The second receipt interop vector is missing from Kupferdraht (2026-10-07, narrowed 2026-10-08)

The PIN service's `interop.json` carries two receipt vectors; the second
one's account domain is the dev instance's hostname, inside the signed
message. A domain swap in Kupferdraht alone would turn the interop check
into a self-test, so `recovery/receipt.rs` now carries only the first
vector (`wire.example`), which exercises the same derivations with a
different seed and a shorter domain. Follow-up: natrium-pin-service
regenerates the second vector with a neutral domain; Kupferdraht adds it
back in the same step.

## 79) Compliance workflow — two upstream issues reported to the OSS Committee (2026-10-08)

Found by Kupferdraht's first full runs of `SchwarzDigits/oss-compliance`
`full-check.yml@v1`, both outside this repository. Reported 2026-10-09 to
the maintainer of the central workflow, together with the request to
classify the five permissive licenses behind the evaluator's hints
(Unicode-3.0, CDLA-Permissive-2.0, Apache-2.0 WITH LLVM-exception,
BSD-1-Clause, BSL-1.0; see §78). Open until the central repository
changes; what Kupferdraht does then is noted per item.

- **Scanner.** The workflow passes `--scanners Askalono`; the `ort:latest`
  image it runs is ORT 96, which no longer ships that scanner (offered:
  DOS, FossId, Licensee, Nomossa, Provenant, ScanCode, SCANOSS). Every
  repository on the current image fails at the scan step. Kupferdraht
  sets the workflow input `scanner: disabled` and relies on declared
  licenses, which `cargo deny check` verifies on every commit; flip it
  back to `auto` once the central workflow names a scanner ORT 96 has.
- **Gradle.** The Gradle Inspector resolves no plugin inside the
  container (`Plugin … was not found in any of the following sources:
  Gradle Central Plugin Repository`); natrium's runs show the same
  failure on all three of its Gradle files, tolerated because the
  analyzer's exit code 2 does not fail the job. Kupferdraht excludes its
  Gradle builds from the analyzer and lists their Maven dependencies by
  hand in THIRD-PARTY-NOTICES; the JDK-17 preparation script stays for the
  day the inspector works.

Contact: opensource@digits.schwarz. Not pursued locally: silencing the
hints through `resolutions` in `.ort.yml` — technically possible, but the
program's onboarding guide asks projects not to override central policy
without the Committee, and the hints are the signal it uses to grow the
classification. Kupferdraht's own license policy for the dependency
graph remains `deny.toml`: it allows Unicode-3.0 and CDLA-Permissive-2.0
by name, accepts the other three only as alternatives of `OR`
expressions whose Apache-2.0 or MIT branch is on its list (ORT, unlike
cargo-deny, reports every alternative), and reads `stringmetrics`'s
license file where ORT reports NOASSERTION.

## 82) The SQLite3MC fork line: branch policy and the next bump (2026-10-31)

Since Step 127 (ADR-0042) the engine comes from three SchwarzDigits forks
consumed by git tag: rusqlite `v0.37.0-sqlite3mc-2.5.1` (branch
`sqlite3mc/0.37`), refinery `v0.9.1-sqlite3mc` (`sqlite3mc/0.9`), openmls
`openmls-v0.9.2-schwarzdigits.1` (branch `schwarzdigits/openmls-0.9` — since Step 128
the one branch per openmls release line that carries every SchwarzDigits
change under one tag: `sqlite_storage` over the forks with its migrations
in one transaction (Step 142), the p256 0.14 providers; upstream's patch
releases of the line are merged into it, v0.9.2 on 2026-11-05; the first
tag `sqlite_storage-v0.3.0-sqlite3mc` and the earlier
`openmls-v0.9.0-schwarzdigits.1` / `.2` stay for the consumers and the
Kupferdraht releases that pin them). The openmls and SQLite3MultipleCiphers forks of
the organisation are also the staging area for upstream pull requests, so
a long-lived downstream branch now sits beside the PR heads; the forks'
`main` stays identical to upstream. GitHub's "ahead/behind" on that
branch measures against upstream `main`, while the branch tracks
upstream's `v0.9` release line (`openmls-v0.9.0`, then `v0.9.2`), which
is not on `main`: most "ahead" commits are upstream's own release-branch
commits, and
`main`'s `sqlite_storage` is unreleased code (async storage, rusqlite
0.40, refinery 0.10). The branch is not rebased onto `main` on purpose;
the next branch comes from the next release tag.

Open: the branch policy is to be confirmed with the maintainer of those
forks; the next bump — when `openmls_sqlite_storage` releases on rusqlite
0.40 — goes rusqlite fork → refinery and openmls branches →
`kupferdraht-core/Cargo.toml` → gate (`vendor/README.md` recipe), and a
rusqlite ≥ 0.38 can retire the fork's wasm arm for its own
`ffi-sqlite-wasm-rs`. (The first Rust embedder dropped its own
`[patch.crates-io]` entry on 2026-11-01.)

## 83) The login path does not enter the manager's client cache (2026-11-01)

Found by Step 78's `forget_account_closes_a_live_client`: only
`Kupferdraht::resume` records a `Weak<Client>` in the manager's cache
(Step 39's "one live `Client` per account"). A client that
`PendingLogin::register_device` or `PendingAccount::activate` returns is the
caller's alone — the manager does not know it. Two consequences:

- `Kupferdraht::forget_account` closes the storage of a client it holds,
  so a client from `login` in the same process keeps working on the
  deleted directory until it is dropped (POSIX unlinks under it; the
  consumer is expected to drop it first, as the TUI's picker does by
  running without a session).
- A `resume` right after `login` in the same process builds a second live
  `Client` for the account — the `zuid` race Step 39's cache was meant to
  prevent — because the cache has nothing to hand out.

The fix is one of: `PendingLogin` / `PendingAccount` get a handle to the
cache (an `Arc` around the map, threaded through `ManagerConfig`) and
insert the client when the registration completes; or `Kupferdraht::login`
returns a manager-aware wrapper. Not done in Step 78 (not needed by it:
every consumer today either runs one process per command or keeps the one
client it logged in with). The Step 78 tests hold their client through
`resume` and say so.

## 84) The browser build has no request timeout and no redirect policy (2026-11-03)

Both knobs are reqwest's native ones: the request timeout of Step 132
(`wire/http/mod.rs`, `storage/remote/token_exchange.rs`) and the
`redirect::Policy::none()` of Step 137 on the token exchange sit under
`#[cfg(not(target_arch = "wasm32"))]` — the fetch backend has neither. So
in a browser a Wire or storage-server response is read whole with no bound
at all (natively the timeout bounds it; the 1 KiB cap of Step 137 is on the
error string, not on the stream), and a cross-origin redirect of the token
exchange is followed by `fetch` — the browser drops `Authorization` on it
per the Fetch standard, so the Wire token does not travel, the body
(audience, public key) does. Noted while reviewing Step 137's residuals.

A fix is a second HTTP path beside reqwest's wasm backend
(`web_sys::RequestInit.redirect(RequestRedirect::Error)`, an `AbortSignal`
timeout) or a response-size policy (`Content-Length` pre-check, a bounded
`bytes_stream`) — a decision of its own. Step 139 (service tokens,
wasm-only, the same exchange client) takes it into its orientation.

Step 139 (2026-11-03) took it into its orientation as announced: the
service-token path (`serviceToken(audience)`, ADR-0036 revision 1.1) buys
through the same exchange client and shares the gap — a service token's
response is read without a bound, a redirect is followed with the
audience in the body. Nothing changed; the decision stays open.

## 85) The older wasm views cross `None` as `undefined`, the types say `null` (2026-11-03)

`serde_wasm_bindgen::to_value` serializes a Rust `None` as `undefined`
unless the serializer is built with `serialize_missing_as_null(true)`.
Every wasm view before Step 138 uses the default — `TimelineItem.body`,
`replyTo`, `asset`, `expiresAtMs`, `hotKnock`, `location`,
`MessageEvent_.replyToId` / `expiresAtMs`, `Member.displayName` / `handle`,
`DeviceInfo`'s optional fields, `RoomDevice.userId` / `deviceId` /
`thumbprint` — while `bindings/web/kupferdraht-tabs/index.d.ts` declares
them `| null`. A consumer comparing `=== null` misses them; `== null` and
truthiness checks do not, which is why nothing has surfaced. Step 138's
own views (`wasm/read_state.rs`) serialize `null` as declared, with a
browser pin. Aligning the rest is one serializer helper and a sweep of the
`to_value` sites — consumer-visible (`undefined` → `null`), so a
`Changed` entry and a d.ts review, not a silent fix.

## 86) `room_read_markers` rows outlive their rooms (2026-11-03)

A marker row (Step 67) and, since Step 138, its flag stay when the room
is left or deleted — nothing purges `room_read_markers`, and `rooms` is
replace-managed by every metadata sync, so a room that returns (rejoined
by link) meets its old position and flag. Harmless for the lists: the
rows are joined onto the room list, never shown on their own
(`read_states()`, the CLI's `rooms`). A purge on leave / delete, or a
sweep against `rooms` at the end of a sync, is a small item for the next
storage step.
