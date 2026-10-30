# ADR-0008: Target Wire API v14 directly

**Status:** Accepted
**Date:** 2026-04-22
**Deciders:** Volker Zöpfel

---

## Context

Wire's backend versions its HTTP and WebSocket API surface.
`GET /api-version` advertises a server's supported set; the client
picks one from the intersection of what it has tested and what the
server offers. At the time of this ADR:

- Wire's production backend advertises **v0 through v14** as
  supported.
- The Wire Swagger specification checked into `reference/wire.swagger.json`
  documents **v14** as the active surface.
- Wire's Kotlin Multiplatform SDK (Kalium) carries version-specific
  adapter classes for **v0 through v15** — suggesting v14 is the
  current stable documented version and v15 is in development.

Kupferdraht's first implementation pass targeted **v5**. The rationale
was defensive: v5 was the version we had read Kalium's source for,
and Wire keeps older versions backwards-compatible, so we assumed
we could stay on older-stable.

That assumption collapsed in live testing against a live Wire v14 backend
(2026-04-21 / 2026-04-22):

1. **Event-stream endpoint.** `/v5/await` returns a generic nginx
   `403 Forbidden` — the versioned route for the v0–v8 event
   stream is retired at the nginx level. `/await` (unversioned)
   succeeds at handshake but the upstream delivers only
   undecodable binary frames and no MLS application events, even
   while a companion iOS client is actively sending. The v0–v8
   event-stream protocol is no longer functional end-to-end.
2. **HTTP surface drift.** Other endpoints continued to work on
   v5, but Kalium's version-specific adapters (V6, V7, V8, …)
   show incremental shape changes throughout. Wire maintains v5
   at the nginx level but the actively-tested code path inside
   the backend is the current version.
3. **Feature gap.** From v9 onward, Wire introduced
   `ConsumableNotifications` — a new event-stream model with
   per-event acknowledgment (`EventAcknowledgeRequest`) and a
   new `/v{N}/events` endpoint. This is fundamentally different
   from the v0–v8 `/await` + `/notifications` model. Every
   forward-direction messaging feature (read receipts, reliable
   delivery) depends on it.

A decision is required: how does Kupferdraht pick its target API version
going forward?

---

## Options Considered

### Option A: Stay on v5, add compatibility shims for the broken parts

Keep `CLIENT_SUPPORTED = &[5]` and work around v5-specific
breakages case by case.

- **Pro:** Minimal disruption. Existing HTTP-call validations
  (Login, Conversations, MLS-Crypto-CRUD, Send) remain valid.
- **Con:** The event stream is the first broken piece. Fixing it
  requires implementing `ConsumableNotifications` (a v9+ feature)
  on top of an otherwise-v5 client — mixing version semantics
  inside a single implementation.
- **Con:** Every subsequent Wire feature we touch risks similar
  version-specific gotchas. Kupferdraht would be perpetually patching
  v5 behaviour against a v14-shaped server.
- **Con:** No end date. Wire may retire v5 entirely in 12–24
  months; we'd then do the full upgrade under time pressure.

### Option B: Incremental upgrade v5 → v8 → v14

Bump `CLIENT_SUPPORTED` one version at a time, validating every
HTTP call between versions.

- **Pro:** Each bump is small and independently testable.
- **Con:** v5 → v8 is **three rounds of full HTTP-surface
  validation** (each call's request/response shape checked
  against the new version) before we reach the version that
  fixes the event stream.
- **Con:** v8 is not a long-term destination either; we'd
  immediately need to plan v8 → v14.
- **Con:** Intermediate versions (v6, v7) have their own quirks
  Kalium records. We'd pay attention to transitional-only
  breakage with no long-term benefit.

### Option C: Jump directly to v14

Set `CLIENT_SUPPORTED = &[14]`. Re-validate every HTTP call
against the v14 Swagger before shipping. Implement the event
stream on v14's `ConsumableNotifications` model.

- **Pro:** Matches the actively-maintained server path. The Wire
  Swagger we have is v14-specific — no interpolation needed.
- **Pro:** Single validation pass: Login, Conversations, MLS,
  Send, Receive all against one version. No intermediate churn.
- **Pro:** Accepts `ConsumableNotifications` as the event-stream
  reality immediately — no transitional simpler-event-stream
  detour.
- **Pro:** Aligns with the long-term posture. v14 (or v15) will
  remain current for years; we land on stable ground.
- **Con:** Every HTTP call we have shipped on v5 needs one
  re-validation pass against v14. Estimated 1–2 focused days.
- **Con:** `ConsumableNotifications` is more complex than the
  v0–v8 model (state tracking for sync-marker, ack per event,
  reconnect-with-cursor semantics). More code in Step 6.

### Option D: Dual-version support (v5 + v14)

`CLIENT_SUPPORTED = &[5, 14]`; pick at runtime based on what the
server advertises.

- **Pro:** Resilient against Wire retiring versions unpredictably.
- **Con:** Every endpoint has two shapes to maintain. Code
  duplication explodes.
- **Con:** The v5 event stream is dead; dual support there is
  academic.
- **Con:** Not a PoC-phase problem. Multi-version matters for a
  production SDK shipped to many backend deployments; Kupferdraht
  talks to one backend (Wire) at a time.

---

## Decision

**Kupferdraht targets Wire API v14 directly (Option C). `CLIENT_SUPPORTED
= &[14]`; every HTTP call re-validated against the v14 Swagger;
event stream implemented on the v9+ `ConsumableNotifications`
model.**

The decisive factor is that v5 is not reachable-from-here as a
viable target: its event stream is server-side retired, not just
"older-but-working." Any forward-looking Kupferdraht implementation
must speak v9+ on the event stream. Once we accept that, staging
through v5 → v8 → v14 buys nothing — we'd still be doing the
event-stream work at v9+, and the intermediate HTTP-surface
validation is redundant with the v14 validation we'll have to do
anyway.

Jumping directly to v14 is the only path where **one validation
pass** produces a complete, shippable client against the current
Wire server.

---

## Consequences

### Positive

- Kupferdraht lives on the actively-maintained server code path.
  Fewer compatibility surprises, faster convergence to stable.
- `ConsumableNotifications` is implemented correctly the first
  time, not as a retrofit over a simpler v0–v8 model.
- `reference/wire.swagger.json` maps one-to-one onto Kupferdraht's HTTP
  surface — no version-translation overhead reading the spec.
- Kalium's V9–V15 adapter classes (when consulted only for API
  protocol observation per ADR-0009) are the directly applicable
  reference point.

### Negative

- All HTTP calls we have previously implemented on v5 must be
  re-validated against v14 before the next live-testing pass.
  Estimated 1–2 focused days; concrete calls listed in
  `docs/wire/api-notes.md`.
- Step 6's implementation is **larger** than the original v0–v8
  plan allowed for. Per-event acknowledgment, sync-marker cursor
  state, and reconnect handling add roughly 200–400 lines of
  Rust over the simple-stream model.
- The existing Step 6 code on `step/6-event-stream` (landed
  2026-04-22) is partially discardable: the WebSocket-connect
  layer and event-frame parser are v0–v8-shaped. The MLS-crypto
  layer (`decrypt_message`, `process_welcome_message`) and the
  observer API stay; the transport layer gets rewritten. See
  observations.md §15 for the asset-preservation breakdown.

### Neutral

- `select_api_version` in `kupferdraht-core/src/version_info.rs`
  already picks `max(intersect(client, server))`; no change to
  the version-negotiation machinery. Only `CLIENT_SUPPORTED`
  moves.
- A future move to v15 (or beyond) is a small-version bump
  repeatable on the same pattern. The difficult part (migrating
  off v5's assumptions) only happens once, now.

---

## Implementation Notes

1. **Swap `CLIENT_SUPPORTED`** from `&[5]` to `&[14]`. Keep the
   multi-element `&[...]` shape so later additions (`&[14, 15]`
   etc.) are trivial.
2. **Re-validate HTTP surface**. Each call in
   `kupferdraht-core/src/http.rs` gets one request/response
   round-trip against a v14 deployment. Request shapes, response
   shapes, status codes, header requirements all compared
   against `reference/wire.swagger.json`. Reference list lives in
   `docs/wire/api-notes.md`.
3. **Replace event-stream transport.** `connect_events_ws` in
   `kupferdraht-core/src/events.rs` moves to `/v14/events` with
   query-param sync marker. New `EventAcknowledgeRequest` flow
   wraps each consumed event. Old `/await`-based code is
   replaced, not extended.
4. **Keep version-agnostic Step 6 code.** `MlsClient::decrypt_message`,
   `process_welcome_message`, `Session::listen_events`,
   `MessageObserver`, the `listen` CLI command — all survive
   the transport-layer rewrite.
5. **CI / live validation** goes against v14 exclusively
   once this ADR is accepted. Contributors can't fork a
   "please-still-test-v5" path.

---

## References

- `reference/wire.swagger.json` — v14 API spec.
- `observations.md` §14 — original v5-hold-for-Step-6 decision and
  the version-landscape survey.
- `observations.md` §15 — v5 event-stream dead-end diagnosis that
  forced this decision.
- `LEARNINGS.md` §1 — session retrospective on what v5
  actually surfaced in live testing.
- Kalium `NotificationApiV9.kt` — reference for the
  `ConsumableNotifications` protocol shape (consulted for API
  protocol observation only; see ADR-0009).

---

## Revision History

| Version | Date | Change |
|---|---|---|
| 1.0 | 2026-04-22 | Initial ADR |
| 1.1 | 2026-10-08 | Wording: validation targets named as a live Wire v14 backend, not Wire's hosted service. Decision unchanged. |
