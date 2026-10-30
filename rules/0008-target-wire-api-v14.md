# Target Wire API v14 only

Condensed from `docs/adrs/0008-target-wire-api-v14.md`.

- **Target Wire API v14 exclusively.** `CLIENT_SUPPORTED = &[14]`.
- **Do not target v5 or any version below v14** — the REST surface is v14-only.
- **Implement the event stream on the v9+ `ConsumableNotifications` model:** `GET /v14/events`, per-event `EventAcknowledgeRequest`, sync-marker cursor across reconnects. *Exception (ADR-0019):* the version-less legacy `/await` socket is the negotiated push fallback while ConsumableNotifications is upstream-disabled — see `0019-auto-negotiate-push-transport.md`; the v14 REST surface is unaffected.
- **Validate every HTTP call against `reference/wire.swagger.json` (v14).**
- Keep the `&[..]` shape so a later `&[14, 15]` is trivial; negotiation already picks `max(intersect(client, server))`.
