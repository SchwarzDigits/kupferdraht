# Non-goals — decided scope cuts

Deliberate decisions not to build something, kept so they are not
re-litigated. **Not** backlog — each is intentional and backed by an ADR,
by `CLAUDE.md`, or by a design note recorded here; reversing one is a scope
decision, not a bug fix. Open questions and backlog live in `open.md`,
shipped changes in `CHANGELOG.md`. Ids are stable: ADRs and
`docs/LEARNINGS.md` cite them (moved here from `open.md` on 2026-11-01).

The five hard prohibitions of the charter are listed in `CLAUDE.md`
("Functional Scope"); the consumer-facing summary is the README's "Known
limitations".

*Backed by an ADR / CLAUDE.md (pointer only):*
- **NG-1** No outbound AES-GCM for assets — AES-256-CBC only; inbound `AES_GCM` decoded but `download()` returns `InvalidInput` (warn-and-skip). ADR-0011; THREAT_MODEL L6.
- **NG-2** No asset preview pipeline — `Asset.Preview` is in the proto, not consumed. ADR-0011.
- **NG-3** Upload retention = `persistent` only. ADR-0011.
- **NG-4** No link-preview generation — `Text.link_preview` echoed verbatim, never fetched. CLAUDE.md.
- **NG-5** No voice / video / calling — `GenericMessage::Calling` skipped. CLAUDE.md.
- **NG-6** No Proteus encryption / decryption — prekeys generated + uploaded (Wire registration contract), never used. ADR-0001.
- **NG-7** No e2ei certificate enrollment — leaves advertise `X509` (W-7) but Kupferdraht never enrolls; BasicCredentials path only. CLAUDE.md.
- **NG-8** No user-property writes — `GET /properties` only; config awareness is read-only. ADR-0012.
- **NG-9** No login refusal on hostile feature policy — feature-config fetch is warn-and-continue. ADR-0012.
- **NG-10** No federation across non-Wire backends — single deployment domain. CLAUDE.md.
- **NG-11** External-senders extension = operator trust — the backend has membership-administration authority (Remove proposals) but never content; MLS-standard, visible to all members. ADR-0004 (W-5).
- **NG-12** Asset authenticity from MLS, not local AEAD — AES-256-CBC (unauthenticated) + a SHA-256 carried inside the MLS-encrypted `GenericMessage`. ADR-0011.
- **NG-13** `attachments` FK `ON DELETE NO ACTION` — deliberate, to avoid an FK-cascade history wipe. CHANGELOG.md §32.

*Standalone design notes (no ADR — kept so the rationale survives):*
- **NG-15** macOS keychain re-prompts per fresh build — dev-time only; `KUPFERDRAHT_DEV_KEYSTORE` is the workaround.
- **NG-16** Inbound / download cache filename carries `message_id` (`{message_id}.enc`); the send-side temp uses a random name.
- **NG-17** *(Step 79, 2026-09-24)* No resend of task / case / marker snapshot rows and no background resend scheduler — `Timeline::resend` refuses full-state snapshots (a stale replay can regress every receiver's LWW fold; re-send the current state via `update_task` / `set_case` / `set_task_list`), and retrying after an outage is the consumer's loop (`docs/library.md` "Reconnect contract"). ADR-0014 / ADR-0018 / ADR-0020; plan `docs/plan/archive/79-failed-send-recovery.md`.

*Retired:*
- **NG-14** *(browser delivery by polling only)* — retired by Step 115 /
  Step 118 (ADR-0033): the browser has a push doorbell and push is the
  default where a target has one. The id stays for the ADRs that cite it.

*Decided by Kupferdraht's purpose (2026-11-01, Volker):*
- **NG-18** No contact discovery and no direct contact initiation —
  no `/search/contacts` (find a user by handle) and none of Wire's
  `/connections` model (connect, accept, block). Kupferdraht's current
  purpose requires that a user cannot search for other users and cannot
  open a contact on their own: who talks to whom is set up by the
  embedding application and the backend it runs against, not discovered
  in the client. Blocking has no object without connections. A DM needs
  the peer's qualified id (`dm_with`, `create_dm`). Supersedes the
  "Safety & contacts" group of the 2026-07-19 gap map (open.md §64).
- **NG-19** No Wire push registration (`/push/tokens`, APNs / FCM) —
  mobile wake-up rides the embedding application's own push path
  (team decision 2026-08); a Kupferdraht-side surface would only be
  needed if the stock Wire push path ever came back into play.
- **NG-20** No history for a new device — a systemic MLS boundary (no
  server archive): a device that joins a group sees what arrives after
  its join. The task join snapshot (ADR-0014 decision 6) is the
  task-side answer; message backup is Step 12 (no endpoint in Wire API
  v14). Consumers should state the limit to their users.
- **NG-21** No presence / "online" status — Wire barely has it, and
  privacy argues against it.
- **NG-22** One account per browser in the server mode — the browser
  keeps one `zuid` per backend domain, so `resume` of an account other
  than the last one logged in is `SessionExpired` (the cookie names
  another user). Not planned (Markus, 2026-09-30): the browser deployment
  has one account per browser; revisit only if several accounts on one
  backend become a use case.
- **NG-23** No iOS notification service extension (NSE) — noted 2026-09-28
  while designing ADR-0031, whose Consequences hold the technical list. A
  second process started by a push, with a hard time and memory budget and
  killed without warning, needs more than ADR-0031: a receive-only drain
  mode (no own commits, no healing, stop before what needs the app), a
  time budget, database keys readable while the device is locked (a shared
  keychain access group, `kSecAttrAccessibleAfterFirstUnlock`, ADR-0029),
  and care with iOS terminating a suspended process that holds a file lock
  in a shared container (`0xdead10cc`) — `kupferdraht.lock` is held across
  `POST /access`. Not planned; an iOS consumer that needs it reopens the
  question with that list.
- **NG-24** Linux desktop is not a supported target of the keystore yet —
  `keyring`'s `linux-native` feature is the kernel keyring (keyutils): in
  memory, lost on reboot and when the persistent keyring expires, after
  which the account's databases cannot be opened on that machine
  (ADR-0029 never replaces a key; a re-login registers a new device and the
  local history is gone). Decided 2026-11-01 (Volker): no step until Linux
  is actually used; Markus had deferred it on 2026-09-27. The route when it
  is: `keyring`'s `linux-native-sync-persistent` + `crypto-rust` features
  (Secret Service over the sync libdbus backend — the async one can block
  inside tokio), `libdbus-1-dev` in CI or the crate's `vendored` feature
  (AFL-2.1 into `deny.toml`), and an ADR for headless Linux without a
  Secret Service (unsupported, a passphrase keystore, or systemd-creds /
  TPM). `Cargo.toml`, `keystore/`, `docs/library.md` and the threat model
  point here.
- **NG-25** One MLS ciphersuite, not negotiated —
  MLS_128_DHKEMP256_AES128GCM_SHA256_P256 (`0x0002`, `MlsClient::MLS_CIPHERSUITE`)
  is fixed; `mls.config.defaultCipherSuite` / `allowedCipherSuites` are
  stored with the other feature configs and not consumed, and the
  key-package pool is single-suite. Decided 2026-11-01 (Volker): no
  negotiation until a backend with another default appears; what ships
  instead is a loud mismatch check at login (Step 132), so a differing
  backend fails at the gate rather than on the first group add. One such
  backend appeared on 2026-11-02 — a self-hosted wire-server whose team
  default is `1` (observations §94); the decision stands, the operator
  sets the team's default to `2` for Kupferdraht devices.
- **NG-26** No processed-ids journal for the pull path — the Step-64 dedup
  catches a notification whose id equals the page cursor; a crash after
  dispatch and before `save_sync_state`, where the unsaved notification
  carries a different id, can dispatch it twice. Accepted 2026-11-01
  (Volker): text is guarded by the UNIQUE constraint and the S6 guard,
  replayed MLS ciphertexts die at openmls' replay protection, and the
  spurious recovery of the early version is excluded since Step 64. An
  idempotent dispatch or a journal is a design pass of its own if a
  duplicate ever shows up.
