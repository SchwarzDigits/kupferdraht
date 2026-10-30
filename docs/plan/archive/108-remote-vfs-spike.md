# Step 108 — Spike: Kupferdraht's browser databases on `sqlite-remote-vfs`

**Status: MERGED 2026-09-30** — commissioned 2026-09-29 (Markus). A
verification step (like Step 88): verdict **go**, with the commit cost as
the open issue that Step 109 (ADR-0037) then solved. `117ea9f`, `15f98e1`;
at the merge the spike moved to the crate's own sqlite-remote-vfs
dependency (0.2.0) and the current configuration, and CI's wasm clippy
now compiles it (`78a4ca5`). Merged to `main` as PR #79 (`81bbaae`); CI
green. No production code. Archived with this header.

## Why

Browsers evict OPFS and IndexedDB, and with them Kupferdraht's databases and
keys. The replacement for the stopped Kalium web line is Kupferdraht + a
Kupferdraht natrium-web; its browser databases (`global.db`, `kupferdraht.db`,
`mls.db` — all of them) move to `sqlite-remote-vfs` (SchwarzDigits,
Apache-2.0): the database file lives on a `sqlite-remote-server`,
SQLite3MC above the VFS keeps the server on ciphertext. A downloadable
keyfile (a seed) makes the keys survive eviction — that part is the next
step's ADR, not this spike. Browser only; native storage is unchanged.

## Questions the spike answers

1. **Linking:** does the VFS (`rsqlite-vfs` 0.2) work with Kupferdraht's wasm
   SQLite (`sqlite-wasm-rs` 0.6.1 + SQLite3MC) in one module?
2. **Kupferdraht's stores on it:** do `SqliteStore` (migrations, pragmas,
   rollback journal) and a keyed SQLite3MC database round-trip through
   the remote VFS — written, closed, reopened by a fresh registration?
3. **Latency:** what does one commit cost? Kupferdraht commits per event
   (Step 105: one transaction per event, plus the MLS state); a drain of
   `n` notifications is roughly `2n` commits.
4. **Takeover:** what does Kupferdraht see when another client with the same
   key takes the database over — and how would it map to `onEvicted`?

## Setup

- Dependency: `sqlite-remote-vfs` as a **wasm-only dev-dependency**, git
  (public repo), pinned to `e8c18cc` (after `6d25577`: one local copy per
  database). `deny.toml` allow-lists that git source.
- Server: `sqlite-remote-server` built from its public repo (`6a4bf9c`),
  memory store, `SQLITE_REMOTE_ALLOWED_ORIGINS=127.0.0.1:*,localhost:*`.
- Tests: `tests/step_108_remote_vfs_spike.rs`, browser, dedicated worker;
  the server URL comes from `KUPFERDRAHT_REMOTE_VFS_URL` at compile time —
  without it the tests return at once (CI has no server).
- One VFS registration per database, all with the same Ed25519 signer; a
  store opens through it by making that VFS's SQLite3MC wrapper the
  default (`sqlite3mc_wrap_vfs`, the stand-in's safe helper) before
  `SqliteStore::open` — no change to the stores for the spike.

## Result (2026-09-29) — **go**, with the commit cost as the open issue

All three spike tests pass in headless Firefox against a local
`sqlite-remote-server` (memory store), also through the harness's latency
proxy.

1. **Linking: yes.** The VFS and Kupferdraht's wasm SQLite (`sqlite-wasm-rs`
   0.6.1 + SQLite3MC) build into one module; `sqlite-wasm-rs` 0.6.1
   itself depends on `rsqlite-vfs` 0.2, so they unify. No change to the
   native build.
2. **Kupferdraht's stores: yes, unchanged.** `SqliteStore::open` (migrations,
   `foreign_keys`, rollback journal) runs on the remote VFS through its
   SQLite3MC wrapper made the default; 100 events written, the store
   closed, reopened by a fresh registration of the same key, all 100 read
   back.
3. **Latency** (one transaction per event, 100 events, one-way proxy
   delay → round trip = 2 × delay):

   | Round trip | per commit | 100 commits | open + migrate (new DB) | reopen + load 100 rows |
   |---|---|---|---|---|
   | ~0 ms (localhost) | 10.2 ms | 1.0 s | 139 ms | 28 ms |
   | 20 ms | 25.0 ms | 2.5 s | 179 ms | 57 ms |
   | 50 ms | 54.6 ms | 5.5 s | 281 ms | 113 ms |

   A commit costs about one round trip + 5–10 ms (worker bridge,
   `Atomics.wait`, the server's synchronous commit). Each single-event
   commit wrote ~9 pages (910 blocks for 102 commits), ~36 KiB upload.
   A drain today commits about three times per notification (MLS state,
   event data, cursor): at a 50 ms round trip ~165 ms per notification,
   ~16 s for 100, ~2.7 min for 1000 after a long offline period.
4. **Takeover: yes.** The fenced client's next write fails with
   `SQLITE_IOERR_FSYNC` (1034, "disk I/O error"); reopening without
   takeover while the newer client holds the lease fails with
   `SQLITE_BUSY` — the signal to map to `onEvicted`.

## Levers for the commit cost (for the ADR / the integration step)

- Write the cursor in the same `kupferdraht.db` transaction as the event data
  (Step 105's combined writes) — one commit less per notification.
- Group a drained page: one `kupferdraht.db` transaction and one MLS window
  for the page instead of per notification (ADR-0032's granularity would
  move from notification to page — needs its own decision).
- Keep the VFS's local copy (`Local::Browser`) for fast reopen and reads;
  it holds only acknowledged data.
- Open the three databases in parallel (three VFS registrations, three
  connections) at start.

## Second round (2026-09-29) — the three levers measured

`measure_drain_patterns`: one page of 100 notifications, each writing
the MLS state (stand-in: three ~2 KiB records rewritten per message), the
event row (Kupferdraht's real `timeline_events` schema) and the cursor
(`sync_state`). Three commit patterns, each without and with the VFS's
local copy (IndexedDB), then a reopen by a fresh registration.

| Round trip | Today (3 commits / notification) | Lever 1: cursor in the event's transaction (2 / notification) | Levers 1 + 2: one MLS + one data transaction per page |
|---|---|---|---|
| ~0 ms | 3.3 s (32.6 ms each) | 2.5 s (25.1 ms) | **0.24 s** (2.4 ms) |
| 20 ms | 7.8 s (77.7 ms) | 5.4 s (54.4 ms) | **0.27 s** (2.7 ms) |
| 50 ms | 16.8 s (168 ms) | 11.3 s (113 ms) | **0.32 s** (3.2 ms) |
| Upload for the page | 6400 KiB, 300 commits | 5600 KiB, 200 commits | **56 KiB, 2 commits** |

Local copy (IndexedDB): each commit costs 5–40 % more (the copy is
written after the server's acknowledgement) — with page batching that is
~10 ms per page. The reopen then fetches **0 blocks** from the server
instead of all of them (here 104 blocks: 69 ms instead of 113 ms at
50 ms RTT); the difference grows with the database, which in real use is
megabytes, not 104 blocks.

**Conclusion.** Grouping a page is the lever that matters: ~50× faster
and ~100× less upload at 50 ms RTT, and flat in the round trip because a
page costs two commits whatever its size. The cursor move alone gains
~30 %. Keep the local copy: it costs little once commits are grouped and
saves the full download on every start.

Caveats: the MLS writes are a stand-in (real openmls writes vary per
message); memory-store server on the same machine; the proxy adds latency
but no bandwidth limit.
