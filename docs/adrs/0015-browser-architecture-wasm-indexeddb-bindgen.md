# ADR-0015: Browser architecture — WASM-only delivery, IndexedDB per-record persistence, hand-written wasm-bindgen surface

**Status:** Accepted (Part 2 — persistence — superseded by ADR-0022; Parts 1 and 3 in force)
**Date:** 2026-07-17
**Deciders:** Volker Zöpfel, Sven Braune

---

## Context

This ADR supersedes ADR-0005 "Web delivery via WebAssembly only" **in
full**. ADR-0005's core decision — Web delivery exclusively as
WebAssembly, no JavaScript port — stands unchanged and is re-affirmed
here; what forced the supersession is that ADR-0005's Implementation
Notes described a browser architecture (sql.js-in-WASM, the SQLite
database serialized as a single blob into IndexedDB, UniFFI-generated
TypeScript bindings) that the actual wasm migration (Steps 44–59,
PR #44) deliberately did not build. A partial supersession ("the
decision stands, the notes don't") was considered and rejected as
confusing — one ADR now carries the whole browser architecture, and
ADR-0005 is cleanly retired.

The shipped design was worked out in natrium's `kupferdraht-wasm-storage.md`
§5 ("D-storage-1") — outside this repo's ADR system — and surfaced by
the 2026-07 documentation audit (`open.md` §61). The design discussion
between Volker and Sven on 2026-07-17 confirmed the shipped state as
the intended architecture; this ADR imports those decisions into the
repo's decision record.

Three questions are answered here: (1) how Kupferdraht reaches the browser
at all, (2) how the browser persists data at rest, (3) how the browser
API surface is produced.

---

## Part 1 — Web delivery mode (re-affirmed, unchanged)

The full option analysis (WASM only vs. WASM + JS port vs. JS-first)
lives in the superseded ADR-0005 and is not repeated; its conclusion is
carried forward unchanged:

**Kupferdraht reaches the Web exclusively as WebAssembly. There is no
JavaScript port and we will not accept one.** WASM linear memory
isolates keys and plaintext from page scripts — a structural security
property no JS implementation can offer. The isolation ends at the JS
boundary: once a decrypted value is returned for display, CSP / SRI /
origin isolation are the consumer's responsibility
(THREAT_MODEL.md N8, L2).

## Part 2 — Browser persistence

### Option A: sql.js — SQLite in WASM, one serialized blob in IndexedDB (the ADR-0005 sketch)

**Pros:**
- At-rest opacity equivalent to native whole-DB encryption: one opaque
  blob, no observable structure.
- Identical SQL schema and query layer on every target.

**Cons:**
- Every persist rewrites the entire database blob — no incremental
  writes; memory cost of a full serialized copy per flush.
- sql.js embeds a second SQLite build inside the WASM artifact —
  significant binary-size and startup cost.
- IndexedDB cannot index into the blob; every lookup loads everything.

### Option B: native IndexedDB object stores, per-record AES-256-GCM (shipped)

One object store per record family (via the `idb` crate). Sensitive
record content is encrypted per record with AES-256-GCM (the `aes-gcm`
crate, wasm-clean) under the account/device key from the
`SecureKeyStore`; primary-key and index fields stay plaintext so
IndexedDB can key and sort on them (D-storage-1).

**Pros:**
- Incremental per-record writes — no whole-DB serialization; fits a
  chat workload of many small writes.
- IndexedDB indexing/sorting works natively on the plaintext keys.
- No sql.js dependency: smaller WASM artifact, faster startup.
- Fits the existing storage seam ("the concrete store owns its
  encryption", `StorageProvider`/`Store`/`GlobalStore`) without
  touching native (which keeps whole-DB SQLCipher, ADR-0013).

**Cons:**
- **Weaker at-rest model than native:** record counts, store names,
  and key/index field values are observable to anything that can read
  the IndexedDB — a deliberate metadata-for-functionality trade.
- A second storage schema to maintain alongside SQLite.

## Part 3 — Browser bindings

### Option A: UniFFI-generated TypeScript bindings (the ADR-0005 sketch)

**Pros:**
- One generator for every foreign surface; mechanical parity with the
  Kotlin/Swift API.

**Cons:**
- UniFFI 0.29 ships no first-party TypeScript/WASM backend; the path
  ADR-0005 assumed was not practically available.
- A generated mirror of the full FFI surface would not be
  browser-idiomatic (naming, JS object shapes, async/callback model
  over the JS microtask queue).

### Option B: hand-written wasm-bindgen layer, parallel to ffi/ (shipped)

**Pros:**
- Standard, mature Rust→browser toolchain (`wasm-pack build --target
  web`), typed `.d.ts` output — coexistence with the unconditional
  `uniffi::setup_scaffolding!` proven in the Step 49 spike.
- A curated, browser-idiomatic surface: camelCase methods, plain JS
  payload objects (serde views), JS-callback-driven sync loop.
- The wasm surface can be narrower than the UniFFI surface where the
  browser has no use for a method (e.g. `save_to` without a
  filesystem).

**Cons:**
- **Manual parity:** a second hand-maintained binding surface that can
  drift from the core API — exactly the drift class the 2026-07 docs
  catch-up had to repair. Mitigated by the freshness rule
  (`docs/bindings.md` documents both surfaces) and the planned CI
  guardrail; not eliminated.
- No generated-code guarantee of signature correctness.

---

## Decision

1. **Delivery:** Web is WASM-only; no JavaScript port (unchanged from
   ADR-0005).
2. **Persistence:** Part 2 Option B — native IndexedDB object stores
   with per-record AES-256-GCM; key and index fields remain plaintext
   by design. The metadata exposure is accepted: the browser's
   IndexedDB is origin-isolated, the actual message/session content is
   ciphertext, and whole-blob rewrites are operationally untenable for
   a chat workload.
3. **Bindings:** Part 3 Option B — the Web surface is the hand-written
   wasm-bindgen layer in `kupferdraht-core/src/wasm/`, parallel to `ffi/`
   (which is cfg'd off on wasm32). UniFFI remains the generator for
   Kotlin/Swift only. The CLAUDE.md parity rule extends in spirit: a
   consumer-facing core change asks "does the wasm surface need
   this?" — divergence is allowed (the browser surface is curated),
   silence is not.

---

## Consequences

### Positive

- Working, incremental browser persistence and typed browser bindings
  shipped since PR #44; the architecture and its trade-offs are now
  recorded in-repo.
- One clean decision record for the whole browser story; ADR-0005 is
  retired without a confusing partial-supersession state.
- Freedom to shape the browser API for browser realities instead of
  mirroring FFI mechanics.

### Negative

- Observable metadata at rest in the browser (record counts, store/key
  names, index values) — a class of exposure native targets do not
  have. **THREAT_MODEL.md does not yet cover the browser at-rest
  model** — follow-up required (THREAT_MODEL edits go through their
  own review).
- Permanent manual-parity duty for a third binding surface; gaps
  versus Kotlin/Swift are deliberate but must stay documented in
  `docs/bindings.md`.
- Two at-rest designs to reason about in every storage change.

### Neutral

- The MLS state follows the same persistence pattern via IndexedDB
  snapshots (`KupferdrahtProvider`, W3). The browser keystore
  (`IndexedDbKeyStore`: IndexedDB + WebCrypto, Worker-capable since
  Step 58) is part of this architecture — code citations for
  Worker-capability point here.
- ADR-0005's memory-isolation rationale is untouched — Part 3 only
  changes how the boundary is generated, not where it is.

---

## Implementation Notes

Shipped state as of PR #44 (Steps 44–59): storage backend
`storage/indexeddb.rs` (`idb` crate, per-record `aes-gcm`); keystore
`keystore/indexeddb_keystore.rs` (WebCrypto via `js_sys::global()`,
Worker-capable); MLS snapshots via the wasm-only `KupferdrahtProvider`
re-export; binding layer `src/wasm/` (`Kupferdraht` / `PendingLogin` /
`Client` / `SyncHandle`), built with `wasm-pack build --target web`.
Surface inventory: `docs/bindings.md` "The WASM / web surface".

---

## References

- Supersedes: ADR-0005 "Web delivery via WebAssembly only" (in full;
  Part 1 carries its decision forward unchanged).
- Builds on: ADR-0002 (single-crate architecture), ADR-0013 (native
  whole-DB SQLCipher — untouched), ADR-0009 (the wasm surface follows
  matrix-rust-sdk naming at the JS level).
- Source decision: natrium `kupferdraht-wasm-storage.md` §5 (D-storage-1),
  imported 2026-07-17; audit trail in `open.md` §61 (resolved) and
  `docs/docs-freshness-audit.md`.
- THREAT_MODEL.md N8 / L2 (JS-boundary limits, unchanged); browser
  at-rest coverage pending.

---

## Revision History

- 2026-07-17: Created (imports D-storage-1 + the bindings decision;
  confirmed Volker Zöpfel / Sven Braune; supersedes ADR-0005).
- 2026-07-17: Renumbered from ADR-0014 to ADR-0015 on the same day —
  0014 was already claimed on another in-flight branch.
- 2026-09-24: Part 2 (Browser persistence) superseded by ADR-0022
  "Persist browser data in SQLite3MC over OPFS instead of IndexedDB
  per-record encryption"; Parts 1 (WASM-only delivery) and 3
  (hand-written wasm-bindgen surface) stay Accepted and unchanged
