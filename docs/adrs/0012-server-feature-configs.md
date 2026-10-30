# ADR-0012: Server feature configs and user properties — fetch all, persist all, expose generically

**Status:** Accepted
**Date:** 2026-05-05
**Deciders:** Volker Zöpfel

---

## Context

Wire's backend exposes two distinct configuration surfaces that influence client behaviour:

1. **Team feature configs** (`GET /v14/feature-configs`) — server-administered policies that apply to every member of a Wire team. Examples: `fileSharing` (whether attachments are allowed; which mime types), `mls` (default protocol, default and allowed ciphersuites), `mlsE2EId` (whether end-to-end identity enrollment is required), `selfDeletingMessages` (timer constraints), `legalhold`, `classifiedDomains`, `conferenceCalling`, `appLock`, `digitalSignatures`. The set is open-ended — Wire adds new feature configs from time to time, and existing configs gain new fields.

2. **User properties** (`GET /v14/properties`, `GET /v14/properties/{key}`, `PUT /v14/properties/{key}`, `DELETE /v14/properties/{key}`) — per-user key-value settings that the user controls themselves. Examples: `WIRE_RECEIPT_MODE` (global read-receipts on/off), `WIRE_TYPING_INDICATOR_MODE`, plus client-specific blobs like `webapp` (Wire-Web's UI state) and `labels`. Property keys are open-ended; clients invent their own and Wire stores them as opaque strings.

Both surfaces also generate events on the notification stream that Kupferdraht drains in `sync::service::dispatch_event`:

- `feature-config.update` — a team-feature-config has changed.
- `user.properties-set` — a property was set or updated.
- `user.properties-delete` — a property was deleted.

Kupferdraht currently consumes none of this. The login pipeline does not call `/feature-configs` or `/properties`; the sync dispatch logs and skips the three event types above. This means the client operates blind to server-administered policy and to user-controlled settings.

The practical consequences today are mild because Kupferdraht's scope is narrow (text messaging, with hardcoded MLS ciphersuite and credential capabilities). But the next planned step — Step 11 (Asset content support, per ADR-0011) — needs `fileSharing` to know whether the user's team allows attachments at all. Sending a file into a team where `fileSharing.status == "disabled"` produces a Wire-side 403 with no client-side preview of the failure; that is fixable client-side if we read the config. (The text of revision 1.0 also assumed a `fileSharing.config.allowedTypes` allow-list; Wire's API defines none — see revision 1.3.)

Beyond Step 11, every plausible future addition (e2ei, Proteus → MLS migration awareness, ephemeral messages, federation-classified domains, legal hold UI banners) consumes server configs. Building this foundation once, generically, keeps each future step focused on its own concerns instead of re-discovering "we need to add a server-config endpoint" each time.

### Why "fetch all" rather than "fetch as needed"

The natural question is whether to fetch only the configs Kupferdraht actively reacts to, and add others as later steps need them. The user's preference is the opposite — fetch and persist every config Wire returns, even ones Kupferdraht does not react to today, and decide on a per-feature basis when (and whether) to add typed getters and reactive logic.

The reasoning is operational, not theoretical. The cost of fetching a config you do not yet read is one HTTP call's worth of bytes per login, plus a row in a small SQLite table — negligible. The cost of the alternative — incrementally adding endpoint coverage as features are needed — is that *every* future step has to first answer "is the config I need already in storage, or do I have to add the fetch?", and the answer is found by code archaeology rather than a single lookup. Worse, when Wire adds a new config field that Kupferdraht did not know to fetch, the question of "did we miss this?" can only be answered by a manual diff against Wire's current schema, which nobody will do.

The "fetch all" posture eliminates that class of question. If Kupferdraht ever needs a config, it is already persisted. Adding a typed getter is a one-line change against existing data, not a change to the login pipeline.

### Why an ADR for this

This is structurally similar to ADR-0008 (target Wire API v14): it pins a default behaviour about Wire's external surface that affects every future step. Without an ADR, the "fetch all" stance can quietly erode under pressure to keep `LoginPipeline` short — the fix would be a one-liner that drops a config, and no review would push back, because no document records the rationale.

---

## Options Considered

### Option A — Lazy: fetch configs on demand from the consuming step

The consuming feature (Step 11 Asset, future e2ei, etc.) adds the HTTP call when it needs the value. No persistence; the response is consumed inline.

- **Pro:** Zero up-front work. No new tables, no event-handling code.
- **Pro:** The set of fetched configs is exactly what the implemented features consume; no waste.
- **Con:** Each step that needs a config re-implements the fetch + parse + cache logic. Three steps in, this is a maintenance liability.
- **Con:** Reactive — when `feature-config.update` fires, every cached value across every consuming step is stale. Each consumer needs its own invalidation strategy.
- **Con:** No surface for diagnosing "what does the server actually say about this team?" — the only way is to read the consuming step's logs.
- **Con:** "Did we miss a config?" is unanswerable without auditing every step's HTTP layer.

### Option B — Typed-only: fetch only the configs Kupferdraht has typed getters for

For each config Kupferdraht intends to read (`fileSharing`, `mls`, `mlsE2EId` in V1), define a typed Rust struct, fetch the named endpoint or extract the field, persist into a typed column. Skip the rest.

- **Pro:** Schema is fully typed; no JSON-blob storage.
- **Pro:** Compile-time safety on every read.
- **Con:** Wire's feature-config schemas evolve. Adding a new field to `fileSharing` upstream requires a Kupferdraht schema migration before Kupferdraht can read the new field — even if the new field is one Kupferdraht would want.
- **Con:** Requires a Kupferdraht release per Wire feature addition, even for read-only usage.
- **Con:** Reverts to "did we miss this?" — restricted to typed configs means non-typed ones are invisible until someone asks.

### Option C — Generic-fetch + selective-typed-getters

Fetch the entire `/v14/feature-configs` response as a single JSON blob and persist it as one row per feature with `name`, `status`, `config` (JSON). Same pattern for `/v14/properties` — one row per property with `key`, `value`. Subscribe to all three notification-event types and re-fetch the affected entry on each.

On top of the generic store, add typed Rust getters only for the configs Kupferdraht actively reacts to today. In V1: `Client::file_sharing() -> FileSharingPolicy` for Step 11. Other configs are reachable as `Client::feature_config(name) -> Option<RawFeatureConfig>` returning the persisted JSON, parsed lazily by callers if and when they need it.

- **Pro:** "Fetch all" satisfied — every config Wire returns is persisted, regardless of whether Kupferdraht reads it today.
- **Pro:** Adding a typed getter for a previously-untyped config is a Rust-only change against existing data; no schema migration, no new endpoint coverage, no LoginPipeline edit.
- **Pro:** Wire schema evolution doesn't break Kupferdraht — new fields appear in the JSON blob automatically.
- **Pro:** Single canonical re-fetch path for every event (`feature-config.update` re-fetches that one entry; the typed-getter layer reads from the persisted blob).
- **Pro:** Diagnostics surface (`kupferdraht-cli features`, `kupferdraht-cli properties`) becomes a one-command answer to "what does the server say about this team / user?".
- **Con:** Loses compile-time field safety for non-typed configs. Mitigated by the convention that Kupferdraht only ever *reacts* to a config through a typed getter; raw access is for diagnostics and future work.
- **Con:** JSON-blob columns are slightly less queryable than typed columns (no `WHERE config.defaultProtocol = …` SQL). Acceptable — feature-config queries are rare, and the typed-getter layer covers reactive paths.

### Option D — Bundle into Step 11 (Asset content support)

Fetch only `fileSharing` as part of Step 11, scoped narrowly to its use case.

- **Pro:** Smaller diff in Step 11, no separate step needed.
- **Con:** Re-discovers exactly the structural problem this ADR addresses. The next step that needs a config (e2ei awareness, mls ciphersuite negotiation, etc.) duplicates the work.
- **Con:** Couples a generic concern (Wire feature configs) to a specific feature (assets), which violates the architecture-layer separation in CLAUDE.md.
- **Con:** Doesn't satisfy "fetch all".

---

## Decision

**Kupferdraht implements Option C: a generic feature-configs + user-properties foundation that fetches everything Wire returns, persists it as JSON blobs in two new SQLite tables, subscribes to the three relevant notification-event types for live updates, and exposes both a generic raw accessor and (for the configs Kupferdraht currently reacts to) typed getters layered over the raw store.**

This is implemented as a new step — **PLAN.md Step 10 — Server feature configs** — landing before Step 11 (Asset content support, per ADR-0011) so that Step 11's `fileSharing` reads against an already-populated store.

### Scope of typed getters in V1

Two:

1. `Client::file_sharing() -> FileSharingPolicy` where `FileSharingPolicy` is `enum { Disabled, Enabled }` (revision 1.3) keyed off `feature_configs.fileSharing.status` — required by Step 11.

2. `Client::receipt_mode() -> ReceiptMode` where `ReceiptMode` is `enum { Off, On }` keyed off `user_properties.WIRE_RECEIPT_MODE` (`"0"` → Off, `"1"` → On). Kupferdraht's outbound read-receipt path (Step 6-II-f, merged) currently always emits; respecting the user's own toggle is a one-line gate that this ADR makes possible.

Tier-2 configs (`mls`, `mlsE2EId`, `mlsMigration`) and all other configs are persisted but not typed in V1. Adding typed getters for them is a future commit per use-case; ADR-0012's foundation makes those commits trivial.

### Login pipeline integration

After `LoginPipeline::register_and_rebind` succeeds (i.e. the access-token is bound to the new client) and before the first KeyPackage upload, the pipeline calls a new method `LoginPipeline::fetch_server_configs` that:

1. Calls `GET /v14/feature-configs` — full response.
2. Calls `GET /v14/properties` — list of property keys.
3. For each key, calls `GET /v14/properties/{key}` — value.
4. Persists everything via the new `Store::replace_feature_configs` and `Store::replace_user_properties` methods (both replace-all-or-nothing).
5. On any HTTP error other than 200, logs at warn level and continues — feature-configs failure is not a login blocker. Kupferdraht operates with its hardcoded defaults until the next successful fetch.

The pipeline failure mode here is a deliberate choice: a Wire backend that briefly cannot serve `/feature-configs` should not prevent the user from logging in to receive messages. The trade-off is that a transient outage could land Kupferdraht in a state where it sends a file the team policy disallows; the resulting Wire 403 then surfaces as a clean error in the asset send path. That is the better failure mode than "user cannot log in".

**Acknowledged risk: login-critical-path latency.** This phase runs sequentially on the login critical path, with `1 + 1 + N` HTTP round-trips (typical N ≤ 5). The warn-and-continue posture above catches outright errors but does not bound latency for slow-but-eventually-2xx responses — a degraded `/v14/feature-configs` lengthens login by exactly that amount. V1 accepts this in exchange for code simplicity and predictable login-state ordering. The follow-up if production smoke shows a meaningful p99 hit is to spawn `fetch_server_configs` as a tokio task after the access-token rebind, return Login as soon as the KeyPackage upload completes, have getters serve cached/default values until the background fetch lands, and cancel the task on `Client::logout`. That follow-up is a small ADR-0012 amendment, not a separate ADR.

### Notification dispatch

`sync/dispatch.rs` gains three new arms:

- `feature-config.update` → re-fetch `GET /v14/feature-configs`, replace the persisted row for the named feature.
- `user.properties-set` → re-fetch `GET /v14/properties/{key}`, upsert the row.
- `user.properties-delete` → delete the persisted row.

All three follow the existing structural pattern: extract envelope, perform fetch + persist, advance the sync stream. Errors during refetch fold into `on_sync_error` and the drain continues.

### Storage shape (V18 migration)

```sql
CREATE TABLE feature_configs (
    name              TEXT NOT NULL PRIMARY KEY,
    status            TEXT,                     -- "enabled" | "disabled" | NULL when feature has no status field
    config_json       TEXT NOT NULL,            -- raw `config` object JSON, or "{}" if absent
    last_synced_at_ms INTEGER NOT NULL
);

CREATE TABLE user_properties (
    key               TEXT NOT NULL PRIMARY KEY,
    value             TEXT NOT NULL,            -- raw response body, opaque
    last_synced_at_ms INTEGER NOT NULL
);
```

Per ADR-0003, neither table receives field encryption. Feature configs are policy-public (every team member sees the same values); user properties are public-to-the-user (the user sets them and Wire stores them in plaintext). Encrypting them at rest would add complexity without a threat-model gain.

### Public API addition

```rust
impl Client {
    /// All team-feature-config entries the server has reported.
    /// Caller iterates and matches on `name` for diagnostics or
    /// for accessing configs Kupferdraht does not yet have a typed
    /// getter for.
    pub fn feature_configs(&self) -> Vec<FeatureConfigEntry>;

    /// One feature config by name (Wire's exact field key —
    /// "fileSharing", "mls", "mlsE2EId", ...).
    pub fn feature_config(&self, name: &str) -> Option<FeatureConfigEntry>;

    /// All user-property entries.
    pub fn user_properties(&self) -> Vec<UserPropertyEntry>;

    /// One user property by key (Wire's exact key —
    /// "WIRE_RECEIPT_MODE", "WIRE_TYPING_INDICATOR_MODE", ...).
    pub fn user_property(&self, key: &str) -> Option<UserPropertyEntry>;

    /// Typed getter — Step 11 entry point.
    pub fn file_sharing(&self) -> FileSharingPolicy;

    /// Typed getter — refines Step 6-II-f's mark_as_read behaviour
    /// to respect the user's own preference.
    pub fn receipt_mode(&self) -> ReceiptMode;
}

pub struct FeatureConfigEntry {
    pub name: String,
    pub status: Option<String>,    // "enabled" | "disabled" | None
    pub config_json: String,        // raw config object as JSON
    pub last_synced_at_ms: i64,
}

pub struct UserPropertyEntry {
    pub key: String,
    pub value: String,              // raw response body, opaque
    pub last_synced_at_ms: i64,
}

pub enum FileSharingPolicy {
    Disabled,
    Enabled,
}

pub enum ReceiptMode {
    Off,
    On,
}
```

The two typed getters return defaults when the underlying entry is missing (e.g. Wire didn't send the config, or login-time fetch failed): `FileSharingPolicy::Enabled` and `ReceiptMode::On`. These are the safest defaults — permitting an action that turns out to be server-disallowed surfaces as a clean Wire error; suppressing an action that is in fact allowed silently breaks features.

---

## Consequences

### Positive

- "Fetch all" eliminates the recurring "did we miss this?" question. Every config Wire returns is persisted, indexed by name, queryable from the CLI.
- Future steps add typed getters as one-line additions over the existing data — no schema migration, no LoginPipeline edit, no new endpoint coverage.
- Wire schema evolution (new fields on existing configs) is zero-effort to absorb. New configs entirely (Wire adds `featureFoo`) appear as a new row at the next login or `feature-config.update` event.
- Step 11 (Asset content support) becomes implementable cleanly, with a typed `Client::file_sharing()` that the CLI can also surface for diagnostics.
- Step 6-II-f (read receipts, merged) gains a one-line gate against `Client::receipt_mode()`, refining its outbound emission to respect the user's own preference.
- Wire-API-NOTES.md gains entries for `/feature-configs` and `/properties`, closing two of its existing "Endpoints Kupferdraht does not yet use" markers.

### Negative

- Two new SQLite tables in the per-account database. Their combined size is on the order of a few KB even on busy team accounts — negligible.
- The `user_properties` row may end up large for keys like `webapp` (Wire-Web's UI-state JSON, multiple KB). This is fine; SQLite handles it transparently. Kupferdraht does not interpret these values, only persists them.
- A small login-time cost: one HTTP call for feature-configs, one for the property-keys list, plus N for each property value. In practice ≤ 5 properties on a typical user; each call is small.
- New error class to surface in `Client::feature_configs` / `Client::user_properties`: returning the cached value vs. signalling staleness. Decision: getters return cached values without a freshness flag; consumers who care about freshness can read `last_synced_at_ms` on the entry.

### Neutral

- The login flow gains one more sequential phase. Already a five-phase pipeline; adding a sixth doesn't change the operational shape.
- No public-API breakage. Existing consumers of `Client` are unaffected; the new methods are additive.
- The ciphersuite-pin question (currently hardcoded in `MlsClient::MLS_CIPHERSUITE`) is not resolved by this ADR — but the data needed to answer it (`feature_configs.mls.config.defaultCipherSuite` and `allowedCipherSuites`) is now persistently available for a future ADR-0013 to operate against.

---

## Implementation Notes

### Module layout

New files:

- `kupferdraht-core/src/wire/http/feature_configs.rs` — `HttpClient::fetch_feature_configs`, `HttpClient::fetch_property_keys`, `HttpClient::fetch_property_value`. Pure transport.
- `kupferdraht-core/src/client/server_config.rs` — typed-getter logic (`FileSharingPolicy::from_raw`, `ReceiptMode::from_raw`).
- `kupferdraht-core/src/storage/migrations/V18__step_10_server_configs.sql` — the two new tables. (V17 is taken by Step 7b's `meta.schema_version` migration; the next free slot is V18.)

Extended files:

- `kupferdraht-core/src/storage/records.rs` — `FeatureConfigEntry`, `UserPropertyEntry`.
- `kupferdraht-core/src/storage/sqlite/...` — `Store` trait methods (`replace_feature_configs`, `replace_user_properties`, `upsert_user_property`, `delete_user_property`, getters).
- `kupferdraht-core/src/client/login_pipeline.rs` — new `fetch_server_configs` phase.
- `kupferdraht-core/src/sync/dispatch.rs` — three new event arms.
- `kupferdraht-core/src/client/builder.rs` — re-export new `Client` methods.
- `kupferdraht-core/src/lib.rs` — public-API re-exports for the new types.
- `kupferdraht-core/src/kupferdraht.udl` — UniFFI surface for the new methods + types.
- `kupferdraht-cli/src/main.rs` — `features` and `properties` subcommands.
- `docs/library.md` — public API documentation.
- `docs/wire/api-notes.md` — promote `/feature-configs` and `/properties` from "does not yet use" to documented.

### Validation strategy

- **Unit tests** in `feature_configs.rs` for HTTP request shape and response parsing against `httpmock` fixtures.
- **Round-trip tests** for the two new Store methods against an in-memory SQLite.
- **Live-smoke against a live Wire v14 backend**: `kupferdraht-cli login` followed by `kupferdraht-cli features` lists at least 5 feature configs (Wire's standard team config has more), `kupferdraht-cli features --name fileSharing` shows the parsed status. `kupferdraht-cli properties` lists the user's existing properties (typically 0–3 entries).
- **Notification-event smoke**: change a user property via Wire-Web (e.g. toggle read-receipts), observe Kupferdraht's sync stream picks up the `user.properties-set` event and re-fetches; `kupferdraht-cli properties --key WIRE_RECEIPT_MODE` reflects the new value within one sync cycle.

### Out of scope for this ADR

- **Reacting** to any feature-config beyond `fileSharing` and `WIRE_RECEIPT_MODE`. Other typed getters and reactive logic are added per future feature in their own ADRs.
- **Negotiating MLS ciphersuite** based on `feature_configs.mls`. The ciphersuite-pin question (currently hardcoded P-256) is the subject of a future ADR-0013, which will use the data this ADR makes available.
- **Refusing login** when a feature-config indicates Kupferdraht cannot operate in this team (e.g. `mlsE2EId.status == "enabled"` with `enrollmentRequired: true`). The defensive choice — log warn, continue, surface clean errors when actions hit the policy — is the V1 posture.
- **Setting and clearing** user properties from Kupferdraht. `PUT /v14/properties/{key}` and `DELETE /v14/properties/{key}` are **not** implemented in V1 — neither in the HTTP layer nor on `Client`. Kupferdraht reads; the user sets and clears via Wire's other clients. The HTTP layer can grow these methods in a future commit if a Kupferdraht consumer ever needs them. The `user.properties-delete` event handler removes only the local SQLite row; calling Wire's DELETE from the handler would re-trigger the same event (Wire is the source of truth for the deletion).

---

## References

- ADR-0003 — per-field encryption. Feature-configs and user-properties tables are unencrypted per this ADR's threat-model analysis.
- ADR-0008 — Wire API v14. The endpoints used here exist in v14; structural change is captured in `docs/wire/api-notes.md`.
- ADR-0009 — matrix-rust-sdk-shaped public API. The new typed getters follow the matrix-rust-sdk pattern of `Client::<feature>()` accessors.
- ADR-0011 — asset content protocol. Step 11 consumes `Client::file_sharing()` directly.
- `docs/wire/asset-protocol.md` Q7 — surveyed Kalium's `FeatureConfigModel` use, the proximate motivation for this ADR. The three-state file-sharing model that survey inspired was replaced in revision 1.3; see `docs/wire/kalium-readings.md`.
- `reference/wire.swagger.json` — `GET /v14/feature-configs`, `GET /v14/properties`, `GET /v14/properties/{key}`, `PUT /v14/properties/{key}`, `DELETE /v14/properties/{key}`, plus event definitions.

---

## Revision History

| Version | Date | Change |
|---|---|---|
| 1.0 | 2026-05-05 | Initial ADR. Decides "fetch all configs and properties, persist as JSON blobs, expose generic raw + selectively typed accessors". Sequenced as PLAN.md Step 10, before Step 11 (Asset content support). |
| 1.1 | 2026-10-08 | Wording: validation targets named as a live Wire v14 backend, not Wire's hosted service. Decision unchanged. |
| 1.2 | 2026-10-09 | Amendment, not a supersession — `Client::wire_receipt_mode()` renamed to `Client::receipt_mode()`. Why: the public API carries no Wire terminology (ADR-0009); the server property key `WIRE_RECEIPT_MODE` it projects is unchanged. The decision is untouched. |
| 1.3 | 2026-10-20 | `FileSharingPolicy` reduced to `Disabled \| Enabled`. Wire's `fileSharing` feature carries only a `status` (the v14 Swagger's `FileSharingConfig`: `status`, `lockStatus`, `ttl`); the former `EnabledSome(Vec<String>)` variant and the `config.allowedTypes` field it read exist on no Wire backend. The shape had been carried over from the Q7 survey of Kalium's `FileSharingStatus` (a build-time flag there), which the provenance audit after counsel's opinion identified as the one place where a Kalium reading shaped a Kupferdraht type. Decision otherwise unchanged. |
