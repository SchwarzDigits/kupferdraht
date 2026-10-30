# ADR-0023: Enforce one active Kupferdraht instance per browser origin with a Web Locks lease and explicit takeover

**Status:** Accepted
**Date:** 2026-09-24
**Deciders:** Volker Zöpfel

## Context

This ADR builds on ADR-0022 "Persist browser data in SQLite3MC over OPFS"
and supersedes **its decision item 7** ("single writer per file; V1
documents the constraint and builds no cross-tab coordination"). Every
other part of ADR-0022 stands. The rationale for deciding now: the
web consumer is about to be built on the wasm surface, and "one tab per
account, documented" is a constraint a web app cannot honour by itself —
users open tabs freely, and the failure mode today is a second worker
whose `Kupferdraht.open` fails on OPFS handle exclusivity with no path
forward (open.md gap-B.31).

The facts the decision rests on. The OPFS `sahpool` VFS implements no
locking; every pool file is held through an exclusive
`FileSystemSyncAccessHandle`, and the pool keeps all of its handles open
for as long as the worker lives — a second worker in the same origin
cannot open the same files until the first worker terminates.
`SyncAccessHandle` exists only in **dedicated** workers, so a
`SharedWorker` cannot host the store for several tabs. The browser keeps
**one cookie jar per origin**: Wire's refresh cookie (`zuid`) is stored per
origin, not per tab, and the per-install label of Step 41 separates two
devices of one account only when each has its own cookie store. Wire
caps the devices per user; closing a tab deregisters nothing. A new MLS
device has no history (Wire's own multi-device property). The Web Locks
API (`navigator.locks`; Chrome 69, Firefox 96, Safari 15.4 — MDN compat
data, 2026-09-24) grants origin-scoped named locks that the browser
releases automatically when the holding tab or worker goes away; a
request with `ifAvailable: true` returns immediately when the lock is
held, and a request with `steal: true` releases the current holder's
lock, whose own request promise then rejects with `AbortError` (W3C
Web Locks, "steal") — an observable eviction signal. `web-sys` 0.3.105
carries the Web Locks bindings only behind `--cfg=web_sys_unstable_apis`.

Constraints in effect: ADR-0015 Part 3 (the hand-written wasm-bindgen
surface is where the contract lives), ADR-0022 decision 3 (`Kupferdraht.open`
fails closed with a recoverable error; no silent fallback), no `unsafe`
in hand-written code, permissive-only dependencies (no new crate is
needed for a lock), and Kupferdraht's browser floor of Chrome 102 / Firefox
111 / Safari 15.2 (ADR-0022), which the Web Locks floor lies below.

## Options Considered

### Option A: One active Kupferdraht instance per origin — a Web Locks lease with explicit takeover

`Kupferdraht.open` acquires the origin-scoped lease `kupferdraht:instance` before
installing the OPFS pool; if another tab holds it, `open` fails closed
with a distinct, recoverable error ("already open in another tab"). An
explicit takeover option steals the lease; the evicted instance learns
of it through its lock promise and signals the embedder, which
terminates that worker (only termination releases the OPFS handles).

**Pros:**
- Matches what an MLS client is: one device, one key set, one history.
  The pattern users know from WhatsApp Web and Element ("open in another
  window — use here?").
- The browser releases the lease when the tab or worker dies: no lock
  file, nothing to clean up after a crash (the argument against lock
  files ADR-0022 already made for OPFS).
- Fits the fail-closed shape of `Kupferdraht.open`; the second tab gets a
  clear reason and a documented way forward (takeover), not a silent
  handle error.
- No new dependency; a few dozen lines around the existing install.

**Cons:**
- A second tab cannot *use* Kupferdraht until it takes over; there is no
  read-only view from a second tab in V1.
- Takeover needs embedder cooperation: Kupferdraht cannot terminate a foreign
  worker, so the evicted side's shutdown is a contract, not a guarantee.
- Web Locks are still a W3C Working Draft; `web-sys` keeps its bindings
  behind an unstable cfg (a hand-written binding avoids the cfg).

### Option B: Every tab is its own client (its own Wire device)

Each tab registers its own device, keys its own databases and MLS
identity, and lives in its own OPFS files.

**Pros:**
- No coordination code; tabs are independent by construction.

**Cons:**
- The origin's single cookie jar makes the independence false: the
  second device's login rotates the `zuid` the first tab depends on, and
  the first tab dies in `SessionExpired`.
- Every tab consumes a Wire device slot and leaves it behind on close;
  a purge on tab close is not something a browser guarantees.
- A new device starts with no history and adds a leaf to every group —
  the wrong semantics for a web app whose tabs come and go.
- `global.db` (the device registry) is per origin by definition, so two
  tabs meet the same handle exclusivity there anyway.

### Option C: A `SharedWorker` hosting one Kupferdraht for all tabs

**Pros:**
- Every tab could use one instance without coordination.

**Cons:**
- `FileSystemSyncAccessHandle` is unavailable in shared workers, so the
  OPFS store of ADR-0022 cannot run there; nesting a dedicated worker
  under a shared one is browser-specific and unproven here.

### Option D: Keep ADR-0022 decision 7 — document "one tab per account", build nothing

**Pros:**
- Zero work.

**Cons:**
- The documented failure is exactly the case a web app hits first; the
  error a second tab sees is an OPFS handle error with no way forward.

## Decision

**Option A.** One active Kupferdraht instance per browser origin, enforced
by an origin-scoped Web Locks lease taken in `Kupferdraht.open` before the
OPFS pool is installed:

1. **Lease first.** `storage::install_browser_storage` requests the
   lease `kupferdraht:instance` with `ifAvailable`. Held elsewhere →
   `Kupferdraht.open` fails closed with a distinct, recoverable error naming
   the cause ("Kupferdraht is already open in another tab of this origin")
   — the same posture as the dedicated-worker refusal of ADR-0022.
2. **Explicit takeover.** `Kupferdraht.open` gains a takeover option that
   requests the lease with `steal`. The evicted instance observes the
   `AbortError` on its lock request, marks itself evicted (every further
   operation fails with a recoverable "evicted" error) and fires an
   embedder callback (`onEvicted`, next to `onExpired`). The embedder
   **terminates the evicted worker** — that is what releases the OPFS
   handles; the new tab's install retries the pool until they are free,
   within a bounded wait.
3. **Granularity is the origin**, not the account: the pool and
   `global.db` are per origin, and the manager already serves several
   accounts from one instance.
4. **Binding:** a hand-written `wasm_bindgen` extern block for
   `navigator.locks.request` (no `unsafe`, no `web_sys_unstable_apis`
   cfg), unless the step's orientation finds the cfg preferable — a
   step-level call, not a decision here.

## Consequences

### Positive

- A second tab gets a clear, actionable state instead of a handle
  error; the embedder can render "open elsewhere — use here?".
- Crash-safe by construction: no lease outlives its tab or worker.
- One device, one history, one key-package pool per user in the
  browser — no device-slot leakage, no cookie-jar races.
- Small: no new dependency, one lease around the existing install, one
  callback on the surface.

### Negative

- No concurrent read-only tab in V1; a user who wants Kupferdraht in two
  tabs gets a takeover dialog, not two views.
- The evicted worker's shutdown is the embedder's duty; a consumer that
  ignores `onEvicted` leaves the handles held and the takeover waits
  out its bound and fails.
- A Working-Draft API: `web-sys` marks it unstable; the hand-written
  binding is a few lines the freshness rule now covers.
- Two new surface elements to document and mirror in spirit
  (`docs/bindings.md`): the takeover option and `onEvicted`.

### Neutral

- Native targets are untouched: the OS file locks and the per-process
  claim question of Step 85 stay where they are.
- The lease is per origin; per-account leases can be layered later
  without changing this decision.
- ADR-0022's other decisions (worker requirement, rollback journal,
  keystore, no data migration) are unchanged.

## Implementation Notes

For Step 92 (`docs/plan/archive/92-single-active-tab.md`, merged 2026-09-24):

- `storage/browser.rs`: the lease before the pool install; the lease
  name as a `pub const`; the eviction signal as a crate-internal channel
  the `wasm::Kupferdraht` binding turns into `onEvicted`.
- `wasm::Kupferdraht::open(permit_http, take_over: bool)` (or an options
  object — the step decides the JS shape) plus `onEvicted` on the
  handle; errors: "already open in another tab" and "evicted", both
  `KupferdrahtError::Storage`-class, recoverable.
- Takeover retry: after `steal`, poll the pool install with a bounded
  wait (the old worker needs a moment to terminate); on timeout fail
  closed with the "still held" error.
- Tests (dedicated worker, `scripts/wasm-test.sh`): the second install
  in the same worker is idempotent; a second dedicated worker spawned
  from the test fails with the tab error; takeover evicts the first and
  succeeds; the evicted instance's operations fail with the evicted
  error. `bindings/web/opfs-smoke/` gains a second-tab scenario.
- Docs: `docs/bindings.md` (the tab contract), ADR-0022 status line +
  `rules/0022` pointer, `rules/0023`, open.md gap-B.31 closes.

## References

- Builds on: ADR-0022 "Persist browser data in SQLite3MC over OPFS
  instead of IndexedDB per-record encryption" — supersedes its decision
  item 7 only.
- Constrained by: ADR-0015 Part 3 (hand-written wasm-bindgen surface),
  ADR-0010 / ADR-0016 (no new dependency needed), CLAUDE.md (no `unsafe`
  in hand-written code).
- Related: Step 85 (cross-process lock on native — a different
  mechanism for a different platform), open.md gap-B.31.
- External: W3C Web Locks API (Working Draft; `steal` releases held
  locks and rejects their requests with `AbortError`); MDN compat data
  for `LockManager` (Chrome 69, Firefox 96, Safari 15.4, read
  2026-09-24); `web-sys` 0.3.105 `LockManager` behind
  `web_sys_unstable_apis`; sqlite.org `persistence.md` (opfs-sahpool:
  no locking, dedicated worker only).

## Revision History

- 2026-09-24: Created and Accepted (Volker, in the Step-91 close-out
  conversation: "ein aktiver Tab" over "jeder Tab ein eigener Client";
  needed before the wasm/web consumer implementation)
- 2026-09-24: Step 92 merged (`ff222f4`, merge `25fd770`) and archived;
  the plan path in Implementation Notes repointed — no content change
- 2026-09-25: Extended by ADR-0026 "Serve every tab of an origin from the
  one active instance — a leader tab elected by the lease queue,
  thin-client tabs over BroadcastChannel" (Step 94): the lease queue
  becomes the election, the holder serves the other tabs; the negative
  consequence "no read-only second tab in V1" is resolved there. Nothing
  here is superseded — no content change
