# Step 169 — API v18 beside v14, negotiated

**Status: commissioned 2026-11-25** (Volker). Large; after Step 168 and the Rust
server's SCIM-groups work. Revises ADR-0008.

## Problem

`CLIENT_SUPPORTED = &[14]` (`wire/version.rs`): Kupferdraht speaks v14 and nothing
else. Wire 5.38.9 and the Rust server serve v14 and v18; the Rust server's plan
switches every other version off. A client pinned to one version cannot follow the
servers, and no test says what v18 changes for the routes Kupferdraht uses.

## Scope

1. **Phase 1: the differences.** For every route Kupferdraht uses, v14 against v18:
   paths, bodies, answers, errors, events. Sources: the Rust server's
   `15-api-versionen-plan.md` and the Swagger golden files under its
   `tests/stack/golden/` (v14 and v18), read against `reference/wire.swagger.json`.
   The list becomes `docs/wire/api-v14-v18-differences.md` and the input to ADR-0008's
   revision (which versions, how negotiated, how a difference is handled in code).
2. **Negotiation.** `CLIENT_SUPPORTED = &[14, 18]`; `GET /api-version` picks the
   highest version both sides support, v14 when the server has no newer one, and the
   choice is logged. Every route carries `/v{N}/` from the negotiated version already
   (`api_version` threads through `wire/http`); where v18 differs, the `wire/` layer
   branches on the version — never the layers above it.
3. **Both versions against the stack.** `kupferdraht-smoke.sh` (the Rust server's
   repository) gets a version switch (default: negotiated) and `run-all.sh` runs it
   once with v14 and once with v18.
4. **Docs:** ADR-0008 revised + `rules/0008-…`, CLAUDE.md's "Wire API: v14 Only"
   becomes "v14 and v18", `docs/wire/api-notes.md`, `docs/backend-spec.md` §3.1,
   `README.md`, `docs/cli.md` (`status` shows the negotiated version), `CHANGELOG.md`
   (BREAKING if a public type changes; expected not).

## Out of scope

- Versions other than 14 and 18; `/events` (consumable notifications; a non-goal of
  the coordinator's list, ADR-0019 keeps the negotiation of the push transport as it
  is).
- Federation.

## TDD sketch

- `negotiate`: `[14, 18]` against a server with `[14, 18]` → 18, with `[14]` → 14, with
  `[18]` → 18, with `[15, 16]` → the existing no-overlap error — red;
- every difference of phase 1 with a route test per version (httpmock on `/v18/…`),
  the v14 tests untouched — red;
- the version in the logs and in `status` — red.

Smoke: the regular smoke twice on wire-dev, v14 and v18; on server 4 negotiated (18).
