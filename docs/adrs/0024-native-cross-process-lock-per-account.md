# ADR-0024: Guard the native critical sections with a per-account advisory file lock, fail-fast

**Status:** Accepted
**Date:** 2026-09-24
**Deciders:** Volker Zöpfel

## Context

Two Kupferdraht processes can share one home on a desktop — kupferdraht-tui and
kupferdraht-cli on the same `KUPFERDRAHT_HOME`, or two CLI invocations racing a
script. The whole-DB-encrypted SQLite stores (ADR-0021) are
multi-connection-safe by themselves, and every native connection waits
for a concurrent writer (`busy_timeout`, 5 s — a `rusqlite` default that
Step 85 makes explicit, observations §85). Two things are **not** shared
through the database and go wrong when two processes do them at once:

1. **The refresh-cookie rotation.** Wire keeps one refresh cookie
   (`zuid`) per `(user, label)` and invalidates it on every successful
   `POST /access`. Two processes holding the same cookie both rebind on
   their next `401` (or at resume, when the persisted token has aged
   out); the second rebind fails `403`, the loser surfaces
   `SessionExpired` and — by the Step-39 rule — blanks the persisted
   tokens the winner had just written. The in-process `refresh_lock`
   cannot see the other process (review finding S18 / open.md gap-B.9).
2. **The first-time creation of a database key.** `load_or_create_key`
   is get-then-set against the platform keystore: two first
   initialisations of one account each generate a key, the second
   `set_secret` overwrites the first, and the database created under
   the first key is unreadable forever (irreversible loss).

Constraints in effect: keys are per database, stored raw under a
keystore id (ADR-0021 — the id namespace is the keystore instance, not
the home); no new dependency where the standard library suffices
(dependency hygiene of ADR-0010/ADR-0016); errors on runtime data are
recoverable, never panics (CLAUDE.md "Assertions"); the browser already
enforces one instance per origin with a Web Locks lease (ADR-0023),
whose "Neutral" consequences explicitly leave the native per-process
question to Step 85. The decider's directives for the step: the lock is
per home (or per account in the home), never machine-wide — two
processes, two homes, two users on one machine must work unchanged; the
same-home case fails fast with a typed, retryable error at exactly the
two critical sections; no blocking, no process-lifelong lock.

## Options Considered

### Option A: Advisory file lock per account directory (plus one per home), fail-fast, adopt-under-lock

`std::fs::File::try_lock` (stable since Rust 1.89; `flock` on Unix,
`LockFileEx` on Windows) on `base/accounts/<id>/kupferdraht.lock` around the
token refresh + persist and the first-time creation of the account's
keys; `base/kupferdraht.lock` around the first-time creation of the
device-registry key. A held lock is a typed `AccountLocked` error, the
caller retries. Under the lock the persisted session is re-read and a
rotation another process already persisted is adopted instead of
rebound; an existing key never takes the lock.

**Pros:**
- The kernel releases the lock with the process — crash-safe, nothing
  to clean up (the argument ADR-0023 made for the browser lease).
- No dependency; a few dozen lines around two existing call paths.
- `flock` binds to the open file description, so two handles in one
  process contend like two processes: the lock is testable in-process.
- Per home / per account by construction — two homes never meet;
  processes on one home contend only inside the two critical sections
  and only when work is due.

**Cons:**
- A refusal is an error the consumer must retry (a 401-triggered
  refresh can surface `AccountLocked` on an unrelated API call).
- Cannot cover two *different* homes on one keystore racing the
  first-ever creation of the same keystore-wide id (gap-B.34) — the
  lock lives in the home, the contended resource in the keystore.
- Advisory: a foreign process that ignores the file is not stopped
  (acceptable — the threat is our own second instance, not an
  adversary).

### Option B: The same lock, blocking with a bounded wait

Wait up to a few seconds for the holder instead of failing.

**Pros:**
- No error surfaces for the common few-millisecond hold.

**Cons:**
- A caller inside an async runtime blocks a worker (or needs a
  spawn-blocking detour) for the wait; a stuck holder turns every
  caller into a stuck caller.
- Rejected by the decider's directive: fail-fast, no blocking.

### Option C: One process-lifelong instance lock per home (the ADR-0023 shape on native)

The first process to open a home owns it; a second `Kupferdraht::open` fails
or takes over.

**Pros:**
- Mirrors the browser exactly; the two critical sections need no
  further thought.

**Cons:**
- Two consumers on one home (kupferdraht-tui + kupferdraht-cli) are a supported
  workflow; the database layer handles them and the only truly shared
  state is the cookie rotation and a first key creation. An instance
  lock forbids far more than it protects.

### Option D: Use SQLite itself as the mutex (a `BEGIN IMMEDIATE` on `kupferdraht.db` around the critical section)

**Pros:**
- No extra file; the engine's locking is already cross-process.

**Cons:**
- The key creation happens *before* a database can be opened (there is
  no key yet), and `global.db` / `mls.db` have their own keys — the
  mutex would not exist when it is needed most.
- Holding a write transaction across the `/access` HTTP round trip
  blocks every other writer for its duration.

### Option E: An `O_EXCL` lock file

**Pros:**
- Simplest primitive; works on every filesystem.

**Cons:**
- Stale after a crash: needs pid-liveness heuristics and a cleanup
  path — exactly what ADR-0023 argued against for the browser, and the
  kernel lock avoids for free.

### Option F: A file-locking crate (`fs2` / `fs4`)

**Cons:**
- Redundant since Rust 1.89 ships the same primitive in `std`; a
  dependency for nothing.

## Decision

**Option A** (implemented in Step 85):

1. **Primitive:** `std::fs::File::try_lock` on `kupferdraht.lock`; the holder
   writes its pid into the file (best effort) so a refusal can name it.
   Native only; the browser has ADR-0023.
2. **Granularity:** `base/accounts/<id>/kupferdraht.lock` guards both
   critical sections of an account; `base/kupferdraht.lock` guards the
   first-time creation of the `global-db` key. Never machine-wide.
3. **Posture:** fail-fast — `KupferdrahtError::AccountLocked(String)`,
   retryable, the payload names the scope and the holder pid when
   known. No blocking, no process-lifelong lock; the lock is taken only
   inside the critical section and only when work is due (key absent,
   refresh needed).
4. **Under the lock, adopt before rebinding:** the token refresh
   re-reads the persisted session and adopts a rotation another process
   persisted (a strictly newer stamp *and* a different access token)
   instead of calling `/access`; the key creation re-reads the keystore
   before generating.
5. **Alongside:** `busy_timeout = 5000` and an explicit
   `synchronous = FULL` on every native open path (`kupferdraht.db`,
   `global.db`, `mls.db`) — the multi-process contract of the database
   layer stated at the open sites, not inherited from a crate default.

## Consequences

### Positive

- Two processes on one home no longer kill each other's session or
  orphan a freshly created database; two homes on one machine are
  untouched by the lock.
- Crash-safe by construction; no cleanup path, no stale-lock heuristics.
- One new error variant, no new dependency, no new public API beyond it;
  the CLI maps it to exit code 3 so scripts can retry.

### Negative

- Consumers must treat `AccountLocked` as "retry": a background sync
  loop or a UI action can see it once when another process is inside the
  critical section.
- **Not covered — the decider's open question:** keystore ids are
  keystore-wide (`global-db`, `account-<id>`, `account-<id>-mls`), so two
  *different* homes on one keystore racing the first-ever creation of
  the same id still lose a database (gap-B.34). Closing it means either
  a per-keystore-id lock location *outside* the home (contradicts
  "per home, nothing written elsewhere") or **home-scoped keystore
  ids** — an ADR-level change of the keystore contract that would also
  fix the shared cookie label of gap-B.33. This ADR does not decide it.
- The pid in the lock file is informational only: it is best effort and
  says nothing about liveness (the kernel lock does).

### Neutral

- Two independent `Client`s for one account in one process contend the
  same way (the manager already hands out one live `Client` per account,
  Step 39).
- A session without a home (custom `StorageProvider`, no `base_dir`)
  takes no lock — there is no shared file to guard; the adoption re-read
  still runs when a store exists.
- Mirror of ADR-0023 by resource, not by shape: browser = one
  *instance* per origin for the instance's lifetime; native = two
  *critical sections* per account for milliseconds. Both fail closed
  with a distinct recoverable error; neither blocks.

## Implementation Notes

Step 85 (`docs/plan/85-cross-process-lock.md`), built 2026-09-24:

- `storage/lock.rs`: `ProcessLock::try_acquire(dir, scope)`; the file
  name `kupferdraht.lock`; `KupferdrahtHome::{home_lock, account_lock}` name the
  paths.
- `storage::load_or_create_key_locked(home, KeyLock, keystore, id)`:
  lock-free fast path for an existing key; `create_dir_all` + lock +
  re-read + create otherwise. Call sites: `open_default_store`
  (`account-<id>`), `open_default_global_store` (`global-db`, home
  lock), `client/builder.rs` login + resume (`account-<id>-mls`).
- `client/token_refresh.rs` (the refresh slice moved out of
  `session.rs`): `acquire_account_lock`, `newer_persisted_session`
  (newer stamp **and** different access token — a just-logged-in
  client's row is stamped after the client was built and carries the
  very token that got the 401); `client/resume.rs::
  refresh_stale_access_token` takes `home` and re-reads under the lock.
- `storage::set_multi_process_pragmas` on the three native open paths.
- Tests: `tests/step_85_cross_process_lock.rs` (two homes / two users,
  shared keystore, gated first-key race, re-exec cross-process holder
  naming the pid, adoption with exactly one `/access`, resume under a
  held lock, busy-wait behaviour + pragma read-back) plus unit tests
  for the primitive, the paths and the CLI exit code.

## References

- Builds on: ADR-0021 "Adopt SQLite3MC with ChaCha20-Poly1305 as the
  whole-DB encryption engine" — the per-database raw key under its own
  keystore id is the thing whose first creation is guarded, and the
  keystore-wide id namespace is the source of the gap-B.34 residual.
- Related: ADR-0023 "Enforce one active Kupferdraht instance per browser
  origin with a Web Locks lease and explicit takeover" — the browser
  mirror; its Neutral consequences leave the native question to this
  decision. ADR-0022 (the OPFS pool is single-instance, so wasm builds
  none of this).
- Constrained by: ADR-0010 / ADR-0016 dependency hygiene (no crate where
  `std` suffices); CLAUDE.md "Assertions" (runtime contention is a
  recoverable error, never a panic).
- External: Rust `std::fs::File::try_lock` (stable 1.89; `flock` on
  Unix, `LockFileEx` on Windows); `rusqlite` 0.37
  `inner_connection.rs` (`sqlite3_busy_timeout(db, 5000)` on open);
  open.md gap-B.9 (closed by Step 85), gap-B.33, gap-B.34;
  observations §83, §85.

## Revision History

- 2026-09-24: Created as Proposed (Step-85 worktree, per the decider's
  brief: draft the decision, do not self-accept)
- 2026-09-25: Accepted (Volker, in the Step-85 report review). The
  open question in Negative Consequences — home-scoped keystore ids for
  gap-B.33 / gap-B.34 — stays open as a follow-up decision, not part of
  this acceptance.
- 2026-09-25: The follow-up is commissioned as Step 93
  (`docs/plan/93-home-scoped-keystore-ids.md`; its ADR-0025 will build
  on this one). No change to this decision.
- 2026-09-25: The open question in Negative Consequences (home-scoped
  keystore ids for gap-B.33 / gap-B.34) is decided by ADR-0025 "Scope
  keystore ids and the login cookie label per home with a persisted
  home id": every keystore id carries the home id, so different homes
  never share an entry and this lock covers every first-time creation
  there is. No change to this decision.
