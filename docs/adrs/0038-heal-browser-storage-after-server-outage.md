# ADR-0038: Heal the browser's storage after the storage server was unreachable

**Status:** Accepted
**Date:** 2026-09-30
**Deciders:** Markus Troßbach

## Context

In the browser's server mode (ADR-0035) an account's `kupferdraht.db` and `mls.db` live on a storage server behind
`sqlite-remote-vfs`. The VFS bridges short interruptions itself: it reconnects for up to its reconnect timeout (10 s)
and, once reconnected, knows whether a commit whose acknowledgement was lost was applied. When the server stays
unreachable longer, the outcome of the last commit is unknown: the VFS reports `Failure::Unreachable`, SQLite sees an
I/O error, and the database accepts no more writes until it is closed and opened again.

Since Step 111b Kupferdraht turns that into `ServiceUnavailable` on every later store operation, and the only way back is
the embedder's: drop the `Client`, resume the account. An app that misses the recipe keeps failing without the user
learning why. Handling an outage has to be Kupferdraht's, and it has to be clean and convenient for the user.

Constraints from prior decisions and from the code:

- Reopening a database on the VFS is synchronous and, while the server is still down, blocks the calling worker until
  the VFS's timeout; the browser runs Kupferdraht in one dedicated worker (ADR-0022, ADR-0023), so a blocked worker stalls
  every call.
- The close-and-reopen is the VFS's own rule, not SQLite's: after an uncertain commit the VFS no longer knows whether
  its blocks in memory match the server. SQLite itself survives an I/O error on the same connection — it rolls the
  transaction back and re-reads the file on the next access; with a journal file (the default rollback journal the
  browser uses) a hot journal left by the failed commit is rolled back on that access.
- The unknown commit is the crash case ADR-0032 designed for: a replayed MLS commit is skipped (`StaleEpoch`,
  `AlreadyDecrypted`), persistence dedupes by message id, the cursor moves only after the MLS commit. MLS groups are
  loaded from `mls.db` per operation; no group state is held in memory.
- One browser home runs one instance (ADR-0023), and since Step 111d the VFS instance id is stable per home and
  account, so a reopen gets its lease back without a takeover.
- "Heal" here means the storage coming back. It is not ADR-0034's healing of a room (discard and rejoin), and a
  storage error never triggers that (ADR-0034 §2).

## Options Considered

### Option A: The embedder drops the client and resumes (status quo)

**Pros:**
- Nothing to build; the state is rebuilt from scratch, as after a restart.

**Cons:**
- Every embedder must detect the case and know the recipe; one that does not fails on and on.
- The sync loop, open handles and UI state are torn down for a network hiccup.

### Option B: Kupferdraht reopens the databases itself and reports a status

Before each operation Kupferdraht checks the VFS's failure and, while the server is down, closes and reopens the database
on the same registration. How Kupferdraht learns that the server is back:

- **B1: an asynchronous reconnect in the VFS** (`RemoteVfs::reconnect_async()`). Pros: nothing blocks. Cons: a second
  connect-and-login path in the VFS, a guard against SQLite calls meanwhile, and a reopen that throws the new
  connection away again.
- **B2: a Kupferdraht-side probe** (a WebSocket to the storage server's URL). Pros: no new API. Cons: a second path to the
  server, and still a reopen with swapped connections.
- **B3: reopen on use** — before an operation, at most every 30 s, a synchronous reopen attempt. Pros: no timer, no
  new API. Cons: Kupferdraht swaps live connections and keeps keys in memory; a black-holed server blocks the worker up to
  the timeout per attempt.

### Option C: The VFS heals the database in place

After `Unreachable` the VFS's connection worker keeps reconnecting in the background, without blocking. While the
database is `Unreachable`, every access fails at once — unless the connection is back, then the VFS heals the
database (login and resume, two round trips; the uncertain commit resolved with its existing check) and carries on.
Kupferdraht neither closes nor reopens anything.

**Pros:**
- The healing sits with the component that owns the connection, the lease and the blocks; the least logic in
  Kupferdraht, and every other application on the VFS gets it too.
- Nothing blocks while the server is down, apart from the one commit that first meets the outage (up to the
  reconnect timeout, which Kupferdraht lowers to 3 s).
- With the rollback journal a failed commit is certainly not committed — the uncertain commit disappears.

**Cons:**
- A VFS release is needed first (estimated 1.5–2 days on the VFS side).
- Kupferdraht depends on the rollback journal and on rolling back after an error inside a transaction — both true today,
  and pinned.

The status surface:

- **S1: `client.storageState` (`"online"` | `"reconnecting"`) and a sixth optional `startSync` callback
  `onStorage(state)`.** Pros: the shape of Step 95d's `onDevicesChanged`; the tabs layer broadcasts it with the other
  sync events (ADR-0026). Cons: the status is pushed only while a sync loop runs; otherwise the embedder reads the
  getter.
- **S2: a separate `client.onStorageStateChanged(callback)`.** Pros: independent of sync. Cons: a new subscription kind
  in the tabs layer.

Sends while the storage is down:

- **D1: fail at once with `ServiceUnavailable`.** Pros: nothing is lost silently; the embedder keeps the text. Cons: the
  user resends by hand.
- **D2: buffer in memory and send after the heal.** Pros: convenient. Cons: a closed tab loses the buffer — data lost
  without notice; ADR-0026 already rules out queuing mutations offline.

## Decision

**Option C, with S1 and D1.** The VFS heals a database in place after `Unreachable`. Before each operation Kupferdraht
checks the database's VFS failure: while it is `Unreachable`, one cheap read lets the VFS heal the database or fail
at once; a failed read fails the operation with `ServiceUnavailable`. The account's state — `reconnecting` while any
of its databases is unreachable, `online` otherwise — reaches the embedder through `storageState` / `onStorage`.
Sends are refused while the storage is down, never buffered. Kupferdraht lowers the VFS's reconnect timeout to 3 s.

The heal never gives up: an outage of any length stays `reconnecting` until the server answers again. Option B was
first chosen with B1; the VFS side's analysis showed the close-and-reopen to be the VFS's own rule, and healing in
place makes Kupferdraht's part a check and a status.

## Consequences

### Positive

- No embedder code for outages beyond showing the status; the account heals itself.
- Nothing blocks while the server is down; calls fail fast and say why (`ServiceUnavailable`).
- Sends fail loudly; nothing is silently lost.

### Negative

- Depends on a VFS release (in-place healing) that does not exist yet.
- The commit that first meets an outage still blocks the worker up to the reconnect timeout (3 s), once per outage
  and database.
- The status is per account; an embedder with several accounts combines it.
- The status changes only when an operation checks it; without a sync loop the embedder reads `storageState`.
- Healing relies on the rollback journal and on the rollback after an error; a later change to `journal_mode =
  MEMORY` or `OFF` in the browser would silently break the guarantee (a pin test guards it).

### Neutral

- Native is unchanged (no remote storage).
- The unknown commit of ADR-0032's crash case does not arise in the browser any more: a failed commit is rolled back
  through its journal. ADR-0032's replay rules stay for the native crash.
- A refused token during the heal stays the access path (`StorageAccessDenied`, `onEvicted("access")`); a takeover by
  another instance stays `onEvicted("device")`.

## Implementation Notes

- The gate: `SqliteStore::lock()` (`kupferdraht.db`) and the MLS provider's transaction entry (`mls.db`) call
  `storage::check_database(path, probe)` — `Unreachable` runs the probe (`SELECT count(*) FROM sqlite_master` on the
  caller's connection); it fails → `ServiceUnavailable` and the database is unreachable; it succeeds → the database
  is reachable again. `TakenOver` and `Denied` keep their meaning (`onEvicted("device")`, `StorageAccessDenied`).
- The account's health (`storage::remote::health`): the set of its unreachable databases and the observers.
- `Server::reconnect_timeout` = 3 s in the server registration.
- Tabs layer: the leader broadcasts `storage` with the client's user id, like `devices`.
- Smoke: the storage URL through a local WebSocket relay that is stopped and restarted.

## References

- Builds on: ADR-0035 (the server mode, `ServiceUnavailable`, the evictions `device` / `access`), ADR-0032 (the replay
  rules that make the unknown commit harmless).
- Constrained by: ADR-0023 (one instance per origin — the stable instance id takes its lease back safely), ADR-0026
  (the tabs broadcast; no mutation queued offline).
- Related: ADR-0034 (a storage error never heals a room), ADR-0036 (storage access with exchanged tokens), ADR-0022
  (browser persistence).
- `docs/plan/113-storage-self-healing.md`; `open.md` §72.

## Revision History

- 2026-09-30: Created (Accepted — Markus)
