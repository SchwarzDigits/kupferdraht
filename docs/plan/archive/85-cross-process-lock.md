# Step 85 — Cross-process lock for token rotation + key creation

**Status: MERGED 2026-09-25** — commit `8c9e4f4` (+ ADR-0024 `775440a`, Step-93 commissioning `80b793d`), fast-forwarded to `main` via the branch's merge commit `4664001`; built on `step/85-cross-process-lock` (worktree instance). Originally commissioned 2026-09-10 (transferred from open.md
gap-B.9; external-review finding S18, confirmed 2026-08-13).
**Phase 1 decided 2026-09-24** (Volker's directives in the worktree
brief + this orientation; the plan of 2026-09-10 re-verified against
`main@b7b13e8` — Steps 77, 86–92 landed since, the line references
below are today's).

## Problem (both want the same lock)

1. **`zuid` rotation race**: two processes sharing one home can
   both refresh the cookie; Wire invalidates the loser's refresh
   token — the second process's session dies (the §15a cookie
   class, but cross-process where the in-process mutex cannot
   help). Worse than "dies": the loser's `SessionExpired` handling
   (Step 39) blanks the persisted tokens the winner just wrote.
2. **`keystore::load_or_create_key` is get-then-set** (S18): two
   parallel FIRST initializations of the same account each generate
   a DB key; the second `set_secret` overwrites — the DB was
   created under key A, the keystore now holds key B:
   **irreversible data loss** on the next open.

Related accepted residual: open.md §69's missing `busy_timeout` is
the same multi-process theme one layer down — if this step lands,
add the pragma alongside (it stops being a residual once
multi-process homes are supported at all).

## Inputs (phase 1) — decided 2026-09-24

- **D1 — Lock mechanism: the standard library's advisory file lock.**
  `std::fs::File::try_lock` (stable since Rust 1.89; the workspace
  builds on 1.98) — `flock(LOCK_EX | LOCK_NB)` on Unix, `LockFileEx`
  on Windows. No new dependency. The kernel releases the lock when the
  holding process dies, so a crash leaves nothing to clean up (the
  same argument ADR-0023 makes for the browser lease). The lock file
  `kupferdraht.lock` stays on disk and carries the holder's pid
  (best-effort) so a refusal can name the holder. `flock` binds to
  the open file description, not the process: two handles in one
  process contend too, which is what makes the in-process tests
  honest. wasm: not compiled — ADR-0023's lease already makes a
  second instance per origin impossible.
- **D2 — Granularity: per account directory, plus one per home.**
  `base/accounts/<id>/kupferdraht.lock` guards both critical sections of
  an account (token refresh + persist; first creation of the
  `account-<id>` and `account-<id>-mls` keys); `base/kupferdraht.lock`
  guards the first creation of the device-level `global-db` key.
  Never machine-wide, and two homes never share a lock file — the
  acceptance criterion (two processes, two homes, two users on one
  machine work unchanged) is pinned by a test. Two processes on
  **one** home stay possible: the lock is taken only inside the two
  critical sections and only when work is due (key absent; refresh
  needed) — an open or resume with existing keys and a fresh token
  takes no lock at all.
- **D3 — Posture: fail-fast, typed, retryable.** A held lock surfaces
  as `KupferdrahtError::AccountLocked(String)` (payload: the scope and the
  holder pid when knowable). No blocking, no process-lifelong lock.
  Under the lock the token refresh **re-reads the persisted session
  and adopts another process's rotation** instead of rebinding — the
  lock alone would only serialise two rebinds and still kill the
  second one's cookie. The resume-time refresh
  (`resume::refresh_stale_access_token`) goes through the same guard.
- **D4 — Residual, documented, not closed:** keystore ids are
  keystore-wide, not home-scoped (`global-db`, `account-<id>`, …).
  Two homes on ONE keystore that both perform the *first-ever*
  creation of the same id at the same instant still race — a per-home
  lock cannot see the other home. This is the same root cause as
  gap-B.33 (the cookie label is per keystore). The fix direction —
  home-scoped keystore ids — is an ADR-level change of the keystore
  contract and Volker's call, not this step's (see the ADR draft).
  **Decided 2026-09-25: commissioned as Step 93**
  (`docs/plan/93-home-scoped-keystore-ids.md`).
- **D5 — `busy_timeout` 5000 ms on all three native open paths**
  (`kupferdraht.db`, `global.db`, `mls.db`); `synchronous` stays FULL and
  becomes explicit: a rotated `zuid` or an MLS epoch advance that
  fails to reach disk after a power loss costs a re-login or a group
  desync, which beats any write-throughput gain from NORMAL.
  Native-only like the WAL pragma; the browser's OPFS pool is
  single-instance by ADR-0023.
- **D6 — Harness: in-process tests for the semantics plus one
  re-exec cross-process test.** The integration-test binary
  re-launches itself as the lock holder (`current_exe()` +
  `--exact <child test>`), a stdin-gated keystore keeps the child
  inside the critical section, the parent asserts the typed refusal
  and the holder pid, then releases the child and succeeds. Honest
  cross-process coverage without a helper binary in the workspace.
- **D7 — ADR.** The mechanism, granularity and posture mirror
  ADR-0023 on native; captured as ADR-0024 + `rules/0024`, drafted as
  Proposed in the worktree (not self-accepted) and **Accepted by Volker
  on 2026-09-25**. Its open question — home-scoped keystore ids
  (gap-B.33 / gap-B.34) — is a separate follow-up decision.
- **D8 — CLI.** No new subcommand: `AccountLocked` is a failure mode
  of every command, so `kupferdraht-cli` maps it to exit code 3 with the
  holder-naming message (scripts retry on 3). Smoke: two concurrent
  `kupferdraht-cli status` on one home after the token aged out — exactly
  one `/access`, no `SessionExpired`.

## Mirror of ADR-0023 (browser lease) — where native matches and where it deliberately does not

| | Browser (ADR-0023, Step 92) | Native (this step) |
|---|---|---|
| Resource | one Kupferdraht **instance** per origin | two **critical sections** per account (+ the home's registry key) |
| Primitive | Web Locks lease `kupferdraht:instance`, `ifAvailable` | advisory file lock `kupferdraht.lock`, `try_lock` |
| Held for | the instance's lifetime | the critical section only (milliseconds) |
| Refusal | `Kupferdraht.open` fails closed ("already open in another tab") | the one call fails with `AccountLocked`; everything else proceeds |
| Takeover | explicit `steal` + `onEvicted` | none — nothing to evict, the holder finishes and the caller retries |
| Crash | browser releases the lease with the worker | kernel releases the lock with the process |
| Granularity | origin (pool + `global.db` are per origin) | account directory (+ home for `global-db`) |

Why not an instance lease on native: two consumers on one home
(kupferdraht-tui + kupferdraht-cli) are a supported workflow — the store is
multi-connection-safe with `busy_timeout`, and the only state that
cannot be shared is the `zuid` rotation and a first key creation.
Why not per-section locks in the browser: the OPFS pool's handles
are exclusive per worker anyway; a second instance cannot even open
the files.

## Not this step — gap-B.33 (cookie label per keystore)

Two homes of the **same** user on one keystore share
`kupferdraht.install-id`, log in with the same label, and Wire keeps one
refresh cookie per `(user, label)`: the later login evicts the
earlier home's `zuid` (observations §83). That is Wire's cookie
model plus a keystore-wide install id — no lock can help, because
the two homes do not share a database or a lock file. The lock here
covers processes on **one** home; the label problem covers homes on
one **keystore**. Both share the root cause in D4 (keystore ids and
the install id are not home-scoped), which is why the ADR draft
names home-scoped keystore ids as the follow-up direction.

## Scope

1. Phase-1 decisions above (D1–D8).
2. Guard exactly the two critical sections: token refresh/persist
   (in-session `Client::refresh_access_token` and resume-time
   `refresh_stale_access_token`), and first-time key creation in
   `storage::open_default_store` (`account-<id>`),
   `client/builder.rs` (`account-<id>-mls`, login + resume) and
   `storage::open_default_global_store` (`global-db`) — make
   load-or-create atomic under the lock with a re-check.
3. `busy_timeout = 5000` + explicit `synchronous = FULL` on the three
   native open paths (`storage/sqlite/mod.rs`, `storage/global.rs`,
   `client/login_pipeline.rs::open_mls_provider`) — closes the §69
   residual.
4. `KupferdrahtError::AccountLocked(String)` (FFI-mirrored by the derive);
   CLI exit code 3 + message naming the holder pid.

## TDD list

- Two homes, two users, one machine: fully concurrent logins, resumes
  and refreshes on separate keystores → no `AccountLocked`, each home
  has its own lock files (pins the acceptance criterion).
- Same on ONE shared keystore with sequential first logins → no
  conflict (different accounts on one keystore are unaffected).
- Two opens of one fresh account racing first-key creation (gated
  keystore) → exactly one key is generated, the loser fails
  `AccountLocked` and succeeds on retry with the winner's key.
- Cross-process: a child process holding the account lock → the
  parent's open fails `AccountLocked` naming the child's pid; after
  the child finishes, the parent opens.
- Two resumed Clients on one home: after the first refreshes, the
  second adopts the persisted rotation — `/access` is called exactly
  once.
- Resume with an aged token while another process holds the lock →
  `AccountLocked`, `/access` untouched; afterwards the resume refreshes.
- `busy_timeout`: a second connection's write waits out a 300 ms
  writer instead of failing `database is locked`; the three open paths
  read back `busy_timeout = 5000` and `synchronous = FULL`.
- Unit: `ProcessLock` (refusal, release on drop, independence per
  directory, holder pid), `KupferdrahtHome::{account_lock, home_lock}`, the
  CLI exit-code mapping.

## CLI

No new subcommand (D8). `kupferdraht-cli` exits 3 on `AccountLocked`;
`docs/cli.md` exit-codes table + storage layout gain the lock file.

## Docs touched

`docs/library.md` (KupferdrahtError table, Storage: lock + pragmas),
`docs/bindings.md` (the new error), `docs/cli.md` (layout, exit
codes), `docs/ARCHITECTURE.md` §3 module map + §7 storage model,
`docs/api-mapping-matrix-rust-sdk.md` (`AccountLocked` row), ADR-0024
draft + `rules/0024-*`, open.md (§69 residual closed; gap-B.9 closed;
D4 residual recorded), `docs/observations.md` if the smoke shows
anything new.
