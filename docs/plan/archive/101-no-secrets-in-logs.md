# Step 101 — No secrets in logs or `Debug` output

**Status: MERGED 2026-09-29** — commissioned 2026-09-28 (Markus — finding
H6 of the production-readiness review, `open.md` §71). `a27d0c3`; fix
`bc12893`. Merged to `main` as PR #70 (`e6563a6`); CI green. Smoke
2026-09-28 against schwarz-dev-wire (release CLI, `RUST_LOG=kupferdraht=debug`).
Scope shipped: the login logs cookie names only; every type carrying a
token, password, code or asset key has a redacting `Debug`.
Retrospective: `docs/LEARNINGS.md` §5.42. Residuals: `open.md` §71 (the
UniFFI-generated `toString` / `description` of FFI records). Archived
with this header.

## Problem (verified in code 2026-09-28)

- `wire/http/auth.rs` (login): the full `Set-Cookie` headers — the `zuid`
  refresh cookie, a session on its own — at debug and in a warning, the
  same headers inside the `BackendRejected` text a consumer receives when
  no `zuid` came, and the cookie's first 8 characters at debug.
- Derived `Debug` prints secrets verbatim: tokens (`SessionRecord`,
  `LoginResponse`, `LoginOutcome`, `RebindOutcome`), asset keys and
  asset tokens (`AttachmentRecord`, `AssetSummary`, FFI `AssetMeta`,
  `AssetMessageInput`, `AssetUploadResponse`), passwords and join /
  activation codes (`CreateConversationCodeRequest`,
  `ConversationCodeInfo`, `JoinConversationByCode`, `NewUserRequest`,
  `ActivateRequest`). One `?value` log field or `{:?}` in an error text
  away from a log file.

## Decisions (2026-09-28)

- **D1 — log cookie names, never values.** The login logs which cookies
  came (names) and whether a `zuid` was among them; the error text names
  the cookies, not their values; no token prefix is logged.
- **D2 — hand-written `Debug` for every type above,** printing
  `<redacted>` (or `<n bytes>` for key bytes) in each secret field and
  everything else as before. Public types keep their fields — no API
  change.

## TDD

- `tests/step_101_no_secrets_in_logs.rs`: a global capture of Kupferdraht's
  own tracing events during a login whose `zuid` carries a sentinel —
  no event contains it, nor its first characters; a login without a
  `zuid` — neither the error nor any event carries the other cookie's
  value.
- `src/secret_debug_tests.rs`: each type built with a sentinel in every
  secret field; its `Debug` output must not contain it (bytes checked in
  their numeric form too).

**As built (2026-09-28).**
- `src/redact.rs`: `Hidden` (`<redacted>`), `hidden_option`
  (`Some(<redacted>)` / `None`), `byte_count` (`<n bytes>`).
- Hand-written `Debug` (generated once from the field lists, `Debug`
  dropped from the derives) for the 14 types above plus — found in the
  review 2026-09-29 — the send-side `EncryptedAsset`, whose
  `Zeroizing<[u8; 32]>` key has a derived `Debug` that prints the bytes;
  `NewUserRequest`'s
  `team_code` (a team invitation code) redacted too; `AssetSummary`'s
  `client` back-reference left out (`finish_non_exhaustive`).
- `wire/http/auth.rs`: `cookie_names` — the login logs `names` at debug
  and in the warning, and the `BackendRejected` text lists the names; the
  token-prefix debug line is gone. `wire::http::auth` is `pub(crate)` so
  the unit test can build `RebindOutcome`.
- Tests: 5 red → green. The log capture keeps Kupferdraht's own events only —
  httpmock logs the requests it receives, cookies included (the fixture,
  not Kupferdraht).
- Residuals → open.md §71 H6 (UniFFI-generated `toString`, a threat-model
  sentence for the owner).
- Smoke 2026-09-28 (schwarz-dev-wire, release CLI, `RUST_LOG=kupferdraht=debug`):
  fresh `login`, `status`, `sync`, `logout --purge-device` — the login
  logs `names=["zuid"]`; no cookie or token value (`zuid=`, the
  `.k=<n>.d=` token shape, `Bearer`) in any output.
