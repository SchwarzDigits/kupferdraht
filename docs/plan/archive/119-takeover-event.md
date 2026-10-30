# Step 119 — A browser learns at once that its databases were taken over

**Status: MERGED 2026-10-02** — commissioned 2026-10-02 (Markus — the
device concept of the storage service; the service and the crate are built
by the storage side, Kupferdraht's part here). `58e236f`. Merged to `main` as
PR #107 (`bb2855d`); CI green. Smoke 2026-10-02 against the test stage
(storage server 0.3.0, release wasm). Scope shipped: sqlite-remote-vfs
0.3.0, the takeover event → `onEvicted("device")` at once, a failed
operation after a takeover → `Evicted`, the evicted sync loop stops.
Retrospective: `docs/LEARNINGS.md` §5.62. Archived with this header.

## Problem (found by the natrium-web SDK, 2026-10-02, on `89c3f00`)

Another browser of the account takes the account's databases over (same
seed, `takeOver`). The browser that held them noticed only at its next own
write: the VFS reports `Failure::TakenOver` when a commit meets the
takeover, and Kupferdraht checks that before each operation
(`storage::check_database`). Before Step 118 every 2.5 s pass wrote the
room list, so the eviction showed after about 5 s; since Step 118 an idle
tab writes only at the 5-minute room-list refresh — measured: 364 s. Two
more faults surfaced:

- the write that meets the takeover fails as `Storage: … disk I/O error`,
  not `Evicted`, and `onEvicted` does not fire (only the next operation's
  check reports the eviction);
- an evicted instance keeps its sync loop and its push socket. Both
  browsers share the Wire client, and the backend closes the previous
  socket of a client id when another connects — the evicted browser's
  reconnect would close the new browser's socket.

sqlite-remote-vfs 0.3.0 (tag `v0.3.0`, `fb257c7`) reports a takeover as
an event: `Server::on_takeover`, called in the worker that registered the
VFS, through its event loop, never during an SQLite call, once per
takeover and database; `failure()` is `TakenOver` already. On the same
server instance it comes at once; across server instances the client
learns at its next request (`FENCED` → `TakenOver`) until the server side
of the event lands. 0.3.0 also pings idle browser connections and no
longer keeps a revoked lease revoked forever.

## Design

- **D1 — sqlite-remote-vfs 0.3.0** (tag `v0.3.0`, `fb257c7`) instead of
  `8ecfc4d` (0.2.0).
- **D2 — the takeover event stops the instance.** Both server databases
  register one listener; it marks the instance evicted with the reason
  `"device"` — `onEvicted("device")` fires at once, every further store
  operation fails as `Evicted` (as a takeover found by the check does
  today). Repeated calls (the second database) change nothing.
- **D3 — an operation that fails because of a takeover reports it.** The
  browser binding's error conversion asks first whether a server database
  was taken over (the VFS's `failure()`); then the error is `Evicted` and
  the instance is marked evicted — for the cross-instance case, where no
  event comes yet.
- **D4 — an evicted instance stops its sync loop**, so its push socket
  closes and it never reconnects with the shared client id: the loop ends
  at its next wake once the instance is evicted, and a pass that failed
  because of a takeover ends it too.
- **Not here:** the device concept itself (slots, `clientId`,
  `claim_slot`/`delete_slot`, `SQLITE_PERM` → its own error, recovery data
  with the client id) — the next step, once the token exchange with
  `clientId` runs on the test stage.

## Tests (red first)

- **Browser, `tests/step_119_takeover_event.rs`** (its own binary — the
  eviction is final for the worker; dedicated worker): the takeover
  listener stops the instance with the reason `"device"`, `onEvicted`
  fires once although both databases report it, and the next store
  operation fails as `Evicted`.
- **Pin, `tests/step_91_wasm_build_glue.rs`:** the lockfile's
  sqlite-remote-vfs is 0.3.x.
- Not testable without a storage server: the event itself (D2's wiring)
  and the cross-instance failure (D3), and the sync loop (D4 — the
  browser has no loop harness). **Smoke** (test stage, storage server
  0.3.0): two browsers of the account (the second restored from recovery
  data, so with its own home id); the second takes over — the first,
  idle, fires `onEvicted("device")` within a second and its push socket
  closes; the second's socket stays up.

## As built (2026-10-02)

- `Cargo.toml`: sqlite-remote-vfs `rev = "fb257c7"` (v0.3.0); the lockfile
  changes only the two crates of that repository. Both pins follow.
- `storage::browser::database_taken_over` — the listener, registered for
  every server database (`server.on_takeover` in
  `storage::remote::server`); it marks the instance evicted with
  `BrowserEviction::Device`, which fires `onEvicted("device")` through the
  existing eviction watch. The first reason stands, so the second database's
  call changes nothing.
- `storage::browser::evicted_by_takeover` — evicted already, or a server
  database's `failure()` is `TakenOver` (then it marks the instance). Used
  by `wasm::manager::to_js` (any error becomes `Evicted`) and by the sync
  loop (`drain_once` ends it after a pass that met a takeover; the loop's
  condition ends it at the next wake once the instance is evicted).
- No CLI command: browser only.

## Smoke (2026-10-02, test stage — storage server 0.3.0, release wasm)

Browser B (account 2, server mode) made recovery data and went idle with
its push socket open. A fresh browser C restored the account from that
data and met `HeldElsewhere`, then took over:

| time | |
|---|---|
| 13:11:42 | C takes over |
| 13:11:42 | B: "another browser took the account's databases over; stopping", `onEvicted("device")` — the same second (on `89c3f00`: up to 364 s) |
| 13:11:42 | B's push socket closes; B opens none again |
| 13:11:43 | C `RECOVER-OK` |

The smoke page terminates its worker on `onEvicted`, as an embedder
should, so the loop ending on its own (D4) is not observed separately.
The cross-instance case (D3) needs a storage server with more than one
instance; the test stage runs one.

