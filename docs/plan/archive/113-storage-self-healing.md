# Step 113 — The browser's storage heals itself after a server outage

**Status: MERGED 2026-09-30** — commissioned 2026-09-30 (Markus —
ADR-0038). `8f61595` (ADR-0038 + rule), `bdf03da`, `76a00bc`, and at the
merge `e1f8d12` (the Step-94 hand-off scenario waits for a previous
scenario's lease: this step's storage-state scenario runs right before it
and raced it on CI). Merged to `main` as PR #96 (replacing #90, same
content with corrected commit authors; `634e222`); CI green. Smoke
2026-09-30 (relay outage on the test stage). Retrospective:
`docs/LEARNINGS.md` §5.53. Archived with this header.

## Problem

In the server mode (ADR-0035) a commit that finds the storage server
unreachable for longer than the VFS's reconnect timeout (10 s) leaves its
outcome unknown: the VFS reports `Failure::Unreachable`, and the database
takes no more writes until it is closed and opened again. Since Step 111b
every later operation fails as `ServiceUnavailable` until the embedder
drops the `Client` and resumes. Markus: the app must not keep failing
silently; the outage has to be handled cleanly and conveniently.

## Decisions (ADR-0038, 2026-09-30)

- **The VFS heals a database in place** after `Failure::Unreachable`: its
  connection worker reconnects in the background, and the next access to
  the database heals it (two round trips) or fails at once. Kupferdraht neither
  closes nor reopens anything. First planned as a Kupferdraht-side reopen with an
  asynchronous VFS reconnect; the VFS side showed the close-and-reopen to be
  the VFS's own rule (Markus: a pragmatic, robust solution with as little
  logic as possible). Built on the VFS side and delivered 2026-09-30:
  sqlite-remote-vfs `9085a9d` (with `979c185`, which removes a panic in
  the browser when an outage outlasts one reconnect attempt), released as
  v0.2.0 (`8ecfc4d`, version numbers only) — Kupferdraht pins that.
- Status: `client.storageState` (`"online"` | `"reconnecting"`) and a sixth
  optional `startSync` callback `onStorage(state)`; the tabs leader
  broadcasts it as `storage`.
- Sends fail at once with `ServiceUnavailable` while the storage is down;
  nothing is buffered.
- The reconnect timeout drops to 3 s: the commit that first meets an outage
  blocks at most that long, once per outage and database.

## Design

- **The gate** (`storage::check_database(path, probe)`): before each
  operation — `SqliteStore::lock()` for `kupferdraht.db`, the MLS provider's
  transaction entry for `mls.db` — the database's VFS failure is read.
  `Unreachable` runs the probe, one cheap read on the caller's connection
  (`SELECT count(*) FROM sqlite_master`): it lets the VFS heal the database
  or fail at once. Failed → `ServiceUnavailable`; succeeded → the
  operation runs. `TakenOver` / `Denied` keep their meaning.
- **The account's health** (`storage::remote::health`): the set of its
  unreachable databases — `reconnecting` while it is not empty — and the
  observers. The gate reports each database's outcome; a report that
  changes nothing is silent.
- **What healing relies on:** the rollback journal in the browser (the
  default; `journal_mode` is set natively only) and a rollback after an
  error inside a transaction (rusqlite's `Transaction`, the MLS provider's
  guard and window). With both, a failed commit is not committed — the
  uncertain commit disappears.
- **Surface:** `Client.storageState` getter; `startSync(…, onDevicesChanged,
  onStorage)`; the tabs layer's `LEADER_EVENTS` gains `storage`, `GETTERS`
  `storageState`, the proxy's `startSync` the sixth callback, the `.d.ts`
  all of it.

## Tests (red first)

- Natively: the health — `reconnecting` while any database is
  unreachable, `online` only when all are, one event per change, silent
  repeats, unsubscribing.
- In the browser (the Step-94 stub harness): `onStorage` reaches a thin
  tab's `startSync` subscriber and every host's `storage` event.
- Pins: the rollback journal in the browser; the 3-s reconnect timeout;
  both databases gated; the binding's `startSync` / `storageState`; the
  tabs layer's tables and `.d.ts`; the embedder guide.
- Smoke (with the VFS release): the storage URL through a local WebSocket
  relay; stop it — operations and sends fail with `ServiceUnavailable`,
  `onStorage("reconnecting")`; restart it — `onStorage("online")`, sync
  and send work again, no resume, no reload.

## Smoke (2026-09-30)

On the test stage, headless Chrome, a profile with the account (server
mode), the storage URL through a local WebSocket relay
(`?scenario=outage&storage=server`), sqlite-remote-vfs `9085a9d`:

- Logged in (the seed present), `storageState` `"online"`; the relay
  stopped two seconds later.
- The sync loop met the outage: `onStorage("reconnecting")`. Two sends
  during the outage failed at once with `ServiceUnavailable` — neither
  reached the other device.
- The relay restarted after 30 s: `onStorage("online")` within about two
  seconds — the VFS healed both databases in place — and the next send
  went through (received by the account's other device). No resume, no
  reload.
