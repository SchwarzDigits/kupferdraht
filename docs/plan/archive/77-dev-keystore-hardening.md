# Step 77 — Dev-keystore hardening (loud activation + atomic writes)

**Status: MERGED 2026-09-24** — commit `402e245`, merged to `main` as `6439f24`; built on `step/77-dev-keystore-hardening`
(gate green, no smoke — refactor: no backend surface; the unit tests in
`keystore/file_store.rs` are the evidence). Commissioned 2026-09-10 as a
Kalium-audit follow-up (Volker: too critical to backlog); smallest of
the three audit steps.

## Problem (verified 2026-09-10; line references re-checked 2026-09-24)

The Kalium JVM audit found secrets in a plaintext `.properties`
file as a silent production default. Kupferdraht's analog is milder but
real: the `KUPFERDRAHT_DEV_KEYSTORE` environment variable switches
`default_keystore` (`client/builder.rs`) to the file-backed
`FileKeyStore` — plaintext key files, documented dev-only, mode
0600/0700. **Second activation site the original plan missed:**
`kupferdraht-cli`'s `cli_keystore` (`kupferdraht-cli/src/cli/mod.rs`) reads the
same variable and constructs the store directly, bypassing
`default_keystore` — so a warn placed only in the builder would never
reach the CLI, the main dev consumer. Three gaps:

1. **Silent downgrade.** Activation emits no log line. A production
   process with an injected/leftover env var stores every DB key in
   plaintext files without anyone noticing. (An attacker who can
   set env vars can usually do worse — the concern is the
   *accidental* leftover: CI configs, shell profiles, forgotten
   smoke setups.)
2. **Non-atomic write.** `write_key_file` (`keystore/file_store.rs`)
   was open-truncate-write in place, no fsync, no temp+rename. A
   crash mid-write destroys the key file — and with it the SQLite3MC
   database it keys (ADR-0021; the Kalium audit's "keystore
   passphrases unusable after crash" class, shrunk to a tiny window
   by the 32-byte payload — `DbKey` is hard 32 since Step 90 — but
   not eliminated).
3. **Dir mode: the doc lied about the code** (external review
   2026-09-10, verified; re-verified red 2026-09-24: 0755). The
   module doc promises "created with mode 0700 if missing";
   `set_secret` used plain `create_dir_all` — default umask,
   typically 0755. Only the key FILES were 0600.

## Scope

1. `tracing::warn!` once per construction in `FileKeyStore::new` —
   the one place both activation sites (`default_keystore` and
   `cli_keystore`) and any direct consumer pass through — naming the
   store, the base dir (structured field), and "development only —
   keys are stored in plaintext files". (Originally "at activation
   in `default_keystore`"; moved to the constructor for the reason
   above, without adding a public symbol.)
2. `write_key_file`: write to `<path>.tmp`, fsync, rename over the
   target, fsync the directory (keep 0600 on the temp file from
   creation, not post-hoc chmod; a stale `.tmp` is removed unread
   before `create_new`).
3. Create the base dir with mode 0700 on Unix (make the doc true);
   an existing directory keeps its mode (it is the operator's). On
   Windows both write paths stay `fs::write`-class — no ACL work
   for a dev-only store. The non-unix arm is not a separate code
   path any more: the write path is one cfg-free function; only
   `.mode(0o600)` / `.mode(0o700)` and the directory fsync are
   `cfg(unix)`, so nothing is silently skipped by the Unix tests
   (there is no Windows CI).
4. No behavior change for the keyring/in-memory/IndexedDB paths.

## TDD (landed as unit tests in `keystore/file_store.rs`)

- `set_secret` twice → latest readable, mode 0600, no `.tmp` residue
  (regression pin — held before the change).
- A pre-existing world-readable `.tmp` (simulated crash) is replaced,
  not read; the key file is 0600 from creation, not inherited (red
  before: the stale `.tmp` stayed).
- A missing base dir is created 0700 (red before: 0755).
- A lone `.tmp` without a key file reads as `None` (pin).
- The warn is pinned with a minimal in-test capturing
  `tracing::Subscriber` (no `tracing-subscriber` dev-dependency
  added): exactly one `WARN`, naming the base dir, containing
  "development only" and "plaintext" (red before: no event).

## Outcome

- `FileKeyStore::new` warns; `set_secret` wraps the incoming bytes in
  `Zeroizing`, creates the dir 0700, writes atomically; `debug_assert!`
  guards on every I/O function (non-empty id, `.key` suffix,
  temp ≠ target).
- CLI: no new command — `cli_keystore` is the existing first consumer.
  Verified 2026-09-24: `KUPFERDRAHT_DEV_KEYSTORE=<dir> kupferdraht-cli rooms`
  prints the `WARN` on stderr under the default `RUST_LOG=warn`
  filter and leaves `<dir>/global-db.key` at 0600.
- Doc homes: `docs/library.md`, `docs/cli.md` (the variable was
  undocumented there), `docs/api-mapping-matrix-rust-sdk.md`,
  `docs/ARCHITECTURE.md` §6.3, the `default_keystore` /
  `cli_keystore` doc comments, this file.
- Not done / deliberately: no process-global `Once` (a warn per
  construction is deterministic and testable; the CLI and the builder
  each construct once per run), no chmod of a pre-existing base dir,
  no Windows ACLs (open.md §69 residual stands).

## Out of scope

- Replacing the env-var mechanism (feature-gate discussion) — the
  escape hatch exists for unsigned-macOS dev builds and smoke runs;
  making it louder is the fix, removing it is not on the table.
- SQLite `busy_timeout`/`synchronous` pragmas and 0600 modes on the
  DB files themselves: checked in the audit, accepted residuals —
  open.md §69 carries the rationale.
- `login()` keystore-before-URL-check ordering (open.md gap-B.26) —
  the reason the gate needs `KUPFERDRAHT_DEV_KEYSTORE` on a sandboxed Mac;
  a separate reorder, not this step.
