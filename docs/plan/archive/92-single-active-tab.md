# Step 92 — One active tab: the Web Locks lease with takeover (ADR-0023)

**Status: MERGED 2026-09-24** — `ff222f4` (the step), merge commit
`25fd770` on `main` by Volker the same day; archived with this header.
Scope shipped: the origin-scoped Web Locks lease before the OPFS pool
(fail-closed "already open in another tab"), explicit takeover with
eviction (`onEvicted`, sticky `evicted`, store operations fail closed,
five-second wait for the evicted worker's handles), the new
`Kupferdraht.open({ permitHttp?, takeOver? }, onEvicted?)` shape, the
scripted two-tab smoke (`TWOTAB-OK` in headless Chrome 153 and Firefox);
gap-B.31 closed. Commissioned 2026-09-24 (Volker); **built 2026-09-24** on
`step/92-single-active-tab` (T1 = options object `Kupferdraht.open({ permitHttp,
takeOver }, onEvicted)`, T2 = hand-written `wasm_bindgen` extern block for
`navigator.locks.request`; eviction fails store operations closed through
the stores' `lock()` accessors — HTTP is not gated, the evicted worker is
terminated by the embedder and the taking-over instance cannot open before
that anyway). Executes ADR-0023 (Accepted
2026-09-24, supersedes decision item 7 of ADR-0022): one active Kupferdraht
instance per browser origin, enforced by a Web Locks lease taken in
`Kupferdraht.open` before the OPFS pool is installed, with an explicit
takeover that evicts the holder. Closes open.md gap-B.31. **Precondition
for the wasm/web consumer implementation** — a web app cannot honour "one
tab per account" by itself, and the failure a second tab sees today is
an OPFS handle error with no way forward.

## Inputs (phase 1)

- **T1 — the JS shape of takeover.** `Kupferdraht.open(permitHttp, takeOver)`
  (a second boolean) or an options object `Kupferdraht.open({ permitHttp,
  takeOver })`. Default if unanswered: the options object — the surface
  is curated for the browser (ADR-0015 Part 3), and a second positional
  boolean is the kind of call nobody can read.
- **T2 — the binding.** Hand-written `wasm_bindgen` extern block for
  `navigator.locks.request` (ADR-0023 decision 4, default) vs. `web-sys`
  behind `--cfg=web_sys_unstable_apis` in the wasm rustflags. Orientation
  measures both; the default stands unless the extern block turns out to
  need `unsafe` or a JS shim.

## Scope

1. **Lease before the pool.** `storage::install_browser_storage` requests
   `kupferdraht:instance` (a `pub const`) with `ifAvailable`; held → a new,
   distinct `KupferdrahtError::Storage` message ("already open in another tab
   of this origin"); `Kupferdraht.open` maps it like the dedicated-worker
   refusal. Idempotent within the holder's worker (a second install
   returns `Ok(())`, the lease is kept).
2. **Takeover.** `Kupferdraht.open` with `takeOver` requests the lease with
   `steal`, then retries the pool install with a bounded wait (the
   evicted worker needs a moment to terminate and release its OPFS
   handles); on timeout fail closed ("still held").
3. **Eviction.** The holder keeps its lock request pending; `steal`
   rejects it with `AbortError` → the instance flips an `evicted` flag
   (every further store/HTTP operation surfaces a recoverable "evicted"
   error) and fires `onEvicted` — a JS callback registered on the wasm
   `Kupferdraht` handle (next to `startSync`'s `onExpired`). The embedder
   terminates the worker; `docs/bindings.md` states that contract.
4. **Consumer proof (phase 6 equivalent):** `bindings/web/opfs-smoke/`
   gains the second-tab scenario — a second worker's `open` fails with
   the tab error, "use here" takes over, the first worker reports
   eviction and is terminated, the second opens.
5. **Docs (phase 7):** `docs/bindings.md` (the tab contract, the option,
   `onEvicted`), `docs/ARCHITECTURE.md` §6.2 browser paragraph,
   `docs/library.md` (the wasm-only `pub` items), api-mapping rows,
   CHANGELOG `[Unreleased]` (BREAKING for web consumers: a second tab no
   longer opens), open.md gap-B.31 → closed.

## Out of scope

- A read-only second tab, per-account leases, a SharedWorker host
  (ADR-0023 Options B/C/D).
- Native cross-process locking (Step 85).
- Any change to the OPFS store itself (ADR-0022 stands).

## TDD sketch (red first)

- `tests/step_92_single_active_tab.rs` (dedicated worker,
  `scripts/wasm-test.sh`): the lease is held after
  `install_browser_storage`; a second install in the same worker is
  idempotent; a second dedicated worker spawned from the test (a small
  inline module worker that calls `install_browser_storage`) fails with
  the tab error; takeover from the second worker evicts the first (its
  `onEvicted` fires, its next store call fails with the evicted error)
  and the second worker's install succeeds.
- `tests/step_91_main_thread_refused.rs` stays green (the worker check
  comes before the lease).
- Native pins: `docs/bindings.md` names the contract; no native code
  changes.

## Smoke

No live Wire (no backend surface). **Run 2026-09-24:** the smoke page's
scripted two-tab scenario (`?scenario=twotab` — one page plays both
tabs, Web Locks being shared across same-origin contexts) in headless
Chrome 153 and headless Firefox (via geckodriver): worker A opens,
worker B is refused with "already open in another tab", worker B' takes
over, A reports eviction and is terminated by the page, B' opens —
`TWOTAB-OK` in both browsers. The first run exposed a scenario-script
bug, not a library one: terminating A only after B' had resolved let
the takeover run into its five-second bound ("still holds the OPFS pool")
— which is exactly the failure the bound exists for.

## Exit criteria

Gate green (five checks + wasm check/clippy + `scripts/wasm-test.sh`),
the smoke page scenario run in Firefox and Chrome, every doc home above
updated in the same commit, ADR-0022 status line + `rules/0022` pointer
to ADR-0023 (done at commissioning), gap-B.31 closed.

Size: one day including docs.
