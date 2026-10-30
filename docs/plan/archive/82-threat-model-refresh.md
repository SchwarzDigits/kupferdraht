# Step 82 — THREAT_MODEL + charter stack-line refresh

**Status: MERGED 2026-09-25** — commit `2a95e49` (reviewed item by item by Volker, fast-forwarded to `main`); built on `step/82-threat-model-refresh`. Originally commissioned 2026-09-10 (transferred from open.md §54 +
§61.1/§61.3 — long-flagged doc debt whose every edit needs Volker's
review, which is exactly what a step's approval phase provides).
Docs-only step: no red tests, no smoke ("no smoke (docs)").

## Worklist (each item reviewed before landing)

1. **THREAT_MODEL header** still says "Step 1–6 / UniFFI 0.28".
2. **G2** still reads as if MLS keys live in the hardware keystore —
   they live in `mls.db` (SQLCipher, key in the OS keystore);
   SE-backed *signing* is a future enhancement, not current state.
3. **§Curve-Mismatch is moot** (open.md §54, 2026-06-09): the
   shipped ciphersuite is P-256, so the Ed25519-wrap workaround the
   section designs never applied. Replace with the real statement:
   MLS keys are SE-*capable* (P-256); today they are software keys
   in `mls.db`; an SE-backed `Signer` for the ECDSA key is the
   realistic future step (KEM/HPKE keys stay in `mls.db` — openmls
   decapsulates internally).
4. **Browser at-rest model missing** (ADR-0015 consequence):
   IndexedDB per-record AES-256-GCM, WebCrypto-wrapped keys,
   observable metadata (record counts/sizes) — write the section.
5. **Trust-boundary note from Step 80**: envelope ids
   (`message_id`, sender ids) are attacker-controlled data that
   historically reached the filesystem; state the boundary ("no
   foreign string becomes a path fragment; guards at dispatch,
   path sites, and materialization") in the THREAT_MODEL data-flow
   section.
6. **CLAUDE.md stack lines** (charter edit, explicit sign-off):
   "openmls + proteus (Cargo git deps)" → crates.io (0.9 line);
   the Wire-API section's "Event stream `GET /v14/events` …
   `/await` retired" contradicts shipped ADR-0019 (WebSocket
   auto-negotiates `/v14/events` vs legacy `/await`; polling
   default) — state the real transport matrix.

## Out of scope

- SSO option-β ADR (open.md §61.2) — separate decision, stays open.
- Any THREAT_MODEL goal/guarantee changes beyond truth-matching the
  shipped state.

## Added 2026-09-25 — Dehydrated Device dropped (Step 13)

Volker dropped Step 13; the Dehydrated Device was a design premise in
several homes that now need a review-gated pass (this step's class of
edit, not a chore): `docs/THREAT_MODEL.md` (the DD section, G7's
"two independent recovery paths" — with Step 12 still Wire-gated, the
honest statement is that no recovery path ships today, plus the
server-trust remarks), `docs/ARCHITECTURE.md` §"Server-side" line,
`docs/backend-spec.md` (Steps 12/13 note), `docs/library.md` ("will be
revisited when Step 9 (Dehydrated Device) lands"), and ADR-0006 +
`rules/0006` (the server-side-DD leg of the Rust-over-KMP rationale —
Revision History entry, rule bullet struck; the decision itself
stands on the client platforms).
