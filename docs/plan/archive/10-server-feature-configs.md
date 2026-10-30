# Step 10 — Server feature configs

> **Status:** merged — `b5ea572` (live 2026-05-09, 33 configs + 2 properties; smoke-baseline green). ADR-0012. Sub-steps: 10a `0850b27` · 10b `e6cd22a` · 10c `1a7a109` (live 2026-05-08) · 10d `a026b64` · 10e `e2a39e7` · 10f `3841204` (live-smoke 2026-05-09). Archived from PLAN.md.
>
> **Sub-steps:** 10a, 10b, 10c, 10d, 10e, 10f.
>
> **Historical names:**
> - "Step 10 — Server feature configs" — 2026-05-05 onward (current).
>
> **Branch / commit prefix:** `step/10-*`, `feat(step-10): …`,
> `chore(step-10): …`. No prior commits use this number — the step
> is newly introduced after ADR-0012.

---

## Step 10 — Server feature configs

Fetch and persist Wire's full set of server-administered configurations:
team-feature-configs (`GET /v14/feature-configs`) and user-properties
(`GET /v14/properties`). Subscribe to live updates via the three
notification events Wire emits when these change. Expose a generic
raw accessor on `Client` plus typed getters for the two configs Step 10
itself reacts to.

The protocol details, design alternatives considered, and the
"fetch all, decide later" rationale are pinned by **ADR-0012**.
This file is the implementation roadmap.

### matrix-rust-sdk analog

`Client::account()::data_settings()` and the various
team-feature accessors in `matrix-sdk` follow a similar shape:
opaque structured config fetched once, refreshed on event, exposed
both generically and through typed convenience methods.

The drift entry for Kupferdraht's `Client::feature_configs()` /
`Client::file_sharing()` lands in
`docs/api-mapping-matrix-rust-sdk.md` once 10e merges.

### Wire API v14 endpoints

- `GET /v14/feature-configs` — entire team-feature-config object
  in one response. Map of feature name → `{status, config}`. New
  features and new fields appear here automatically as Wire
  evolves.
- `GET /v14/properties` — list of user-property keys
  (`["WIRE_RECEIPT_MODE", "labels", ...]`). Body is a JSON array
  of strings.
- `GET /v14/properties/{key}` — value for one property. Body is
  the opaque value (often a JSON-encoded string, sometimes a raw
  numeric string like `"0"` / `"1"`). Kupferdraht does not interpret
  the value beyond storage — typed getters parse on read.
`PUT /v14/properties/{key}` and `DELETE /v14/properties/{key}`
exist on the Wire backend but are **not** implemented in V1 of this
step. Kupferdraht reads server-administered configuration; setting and
clearing properties is done via Wire's other clients. The
`user.properties-delete` event handler in 10d removes only the local
SQLite row — it does not call Wire's DELETE (Wire is the source of
truth for the deletion in the first place; calling DELETE again
would create an event loop). See ADR-0012 §Out of scope.

### Wire Message Types

None. This step does not touch MLS or `GenericMessage`.

### Notification event types handled

`sync/dispatch.rs` gains three new arms — see ADR-0012 §Notification
dispatch for the rationale.

- `feature-config.update` — re-fetch `/v14/feature-configs` (full
  response, not per-feature; Wire's API does not expose
  per-feature read endpoints under v14), upsert the named row.
- `user.properties-set` — re-fetch `/v14/properties/{key}`, upsert
  the persisted row.
- `user.properties-delete` — delete the local row keyed by the
  event's `key`.

All three errors fold into `on_sync_error` and the drain continues,
matching the existing pattern from Steps 5 and 6-II.

### Public API

```rust
impl Client {
    pub fn feature_configs(&self) -> Vec<FeatureConfigEntry>;
    pub fn feature_config(&self, name: &str) -> Option<FeatureConfigEntry>;

    pub fn user_properties(&self) -> Vec<UserPropertyEntry>;
    pub fn user_property(&self, key: &str) -> Option<UserPropertyEntry>;

    /// Typed getter — Step 11 entry point.
    pub fn file_sharing(&self) -> FileSharingPolicy;

    /// Typed getter — refines Step 6-II-f's outbound mark_as_read
    /// behaviour to respect the user's own read-receipt preference.
    pub fn wire_receipt_mode(&self) -> ReceiptMode;
}

pub struct FeatureConfigEntry {
    pub name: String,
    pub status: Option<String>,
    pub config_json: String,
    pub last_synced_at_ms: i64,
}

pub struct UserPropertyEntry {
    pub key: String,
    pub value: String,
    pub last_synced_at_ms: i64,
}

pub enum FileSharingPolicy {
    Disabled,
    EnabledAll,
    EnabledSome(Vec<String>),
}

pub enum ReceiptMode {
    Off,
    On,
}
```

UDL additions land in 10e.

### Sub-steps

#### 10a — HTTP layer (`wire/http/feature_configs.rs`)

Three methods on `HttpClient`:

```rust
pub async fn fetch_feature_configs(
    &self,
    api_version: u32,
    access_token: &str,
) -> Result<RawFeatureConfigs, KupferdrahtError>;

pub async fn fetch_property_keys(
    &self,
    api_version: u32,
    access_token: &str,
) -> Result<Vec<String>, KupferdrahtError>;

pub async fn fetch_property_value(
    &self,
    api_version: u32,
    access_token: &str,
    key: &str,
) -> Result<String, KupferdrahtError>;
```

PUT and DELETE on `/v14/properties/{key}` are intentionally absent
in V1 — see the Wire-endpoints note above and ADR-0012 §Out of scope.

`RawFeatureConfigs` is a thin wrapper around `serde_json::Value`
plus iteration helpers — no per-feature deserialisation at this
layer. Wire's actual response shape is `{ "fileSharing": {...},
"mls": {...}, ... }`, and `RawFeatureConfigs::iter()` yields
`(name, raw_status, raw_config)` tuples.

TDD checklist (against `httpmock`):

- `fetch_feature_configs` parses a representative Wire response
  (snapshot the live response from a test login, anonymise, drop
  in as fixture).
- `fetch_property_keys` parses a JSON array body.
- `fetch_property_value` returns the raw response body for both
  JSON-encoded strings (`"\"on\""`) and plain numeric strings
  (`"1"`).
- 404 on `fetch_property_value` surfaces as `Ok(_)` from a
  higher layer that interprets it as "deleted" — but at this
  HTTP layer, 404 is `Err(BackendRejected)` like every other
  endpoint.

#### 10b — Storage migration V18 + records

New SQL migration `V18__step_10_server_configs.sql`. (V17 was taken
by Step 7b's `meta.schema_version` migration; the next free slot is
V18. ADR-0012 §Storage shape and earlier drafts of this file
referenced V17 — both have been corrected.)

```sql
CREATE TABLE feature_configs (
    name              TEXT NOT NULL PRIMARY KEY,
    status            TEXT,                   -- nullable; not every config has status
    config_json       TEXT NOT NULL,
    last_synced_at_ms INTEGER NOT NULL
);

CREATE TABLE user_properties (
    key               TEXT NOT NULL PRIMARY KEY,
    value             TEXT NOT NULL,
    last_synced_at_ms INTEGER NOT NULL
);
```

Plus `Store` trait additions:

```rust
async fn replace_feature_configs(
    &self,
    entries: &[FeatureConfigEntry],
) -> Result<(), KupferdrahtError>;

async fn upsert_feature_config(
    &self,
    entry: &FeatureConfigEntry,
) -> Result<(), KupferdrahtError>;

async fn list_feature_configs(
    &self,
) -> Result<Vec<FeatureConfigEntry>, KupferdrahtError>;

async fn get_feature_config(
    &self,
    name: &str,
) -> Result<Option<FeatureConfigEntry>, KupferdrahtError>;

async fn replace_user_properties(
    &self,
    entries: &[UserPropertyEntry],
) -> Result<(), KupferdrahtError>;

async fn upsert_user_property(
    &self,
    entry: &UserPropertyEntry,
) -> Result<(), KupferdrahtError>;

async fn delete_user_property(
    &self,
    key: &str,
) -> Result<(), KupferdrahtError>;

async fn list_user_properties(
    &self,
) -> Result<Vec<UserPropertyEntry>, KupferdrahtError>;

async fn get_user_property(
    &self,
    key: &str,
) -> Result<Option<UserPropertyEntry>, KupferdrahtError>;
```

Per ADR-0003 + ADR-0012, neither table receives field encryption.

TDD checklist:

- Round-trip per record: insert, list, get-by-name, replace-all
  semantics (replace_feature_configs deletes any rows not in the
  new set in a single transaction).
- `delete_user_property` is idempotent (deleting a non-existent
  key returns Ok).

#### 10c — Login pipeline integration

`LoginPipeline` gains a new phase between `register_and_rebind` and
`upload_initial_key_packages`:

```rust
async fn fetch_server_configs(&self) -> Result<(), KupferdrahtError> {
    // 1. Fetch feature-configs.
    match self.http.fetch_feature_configs(api_v, token).await {
        Ok(raw) => self.store.replace_feature_configs(
            &raw.into_entries(now_ms())
        ).await?,
        Err(e) => warn!(error = ?e,
            "feature-configs fetch failed; using defaults"),
    }

    // 2. Fetch property keys, then per-key values.
    match self.http.fetch_property_keys(api_v, token).await {
        Ok(keys) => {
            let mut entries = Vec::with_capacity(keys.len());
            for k in keys {
                match self.http.fetch_property_value(
                    api_v, token, &k
                ).await {
                    Ok(v) => entries.push(UserPropertyEntry {
                        key: k, value: v,
                        last_synced_at_ms: now_ms(),
                    }),
                    Err(e) => warn!(key = %k, error = ?e,
                        "property fetch failed; skipping"),
                }
            }
            self.store.replace_user_properties(&entries).await?;
        }
        Err(e) => warn!(error = ?e,
            "property keys fetch failed; using defaults"),
    }
    Ok(())
}
```

The error posture is **warn-and-continue** at every level — feature
configs are an enrichment, not a login blocker. Per ADR-0012, the
trade-off is that a transient Wire outage could leave Kupferdraht in a
state where it sends a request the team policy disallows; the
resulting 403 surfaces cleanly in the action's error path, which
is preferable to "user cannot log in".

TDD: integration test against `httpmock` covers (a) all-success
path, (b) feature-configs 500 → properties still fetched + Store
updated, (c) properties keys-fetch 500 → feature-configs still
persisted.

##### 10c addendum — raw `Client` accessors + CLI listing

Pulled forward from 10e/10f after the first 10c review. Without a
public-API surface and a CLI command, the only way to verify "login
populates the V18 tables" is raw `sqlite3` inspection — not a valid
acceptance test for a Wire-compatible client. The deliverable is
extended with the four read-side accessors from 10e and the two
listing subcommands from 10f, but **not** the typed getters or the
live-smoke.

Public API added in 10c (raw read only — no typed getters yet):

```rust
impl Client {
    pub async fn feature_configs(&self) -> Result<Vec<FeatureConfigEntry>, KupferdrahtError>;
    pub async fn feature_config(&self, name: &str)
        -> Result<Option<FeatureConfigEntry>, KupferdrahtError>;
    pub async fn user_properties(&self) -> Result<Vec<UserPropertyEntry>, KupferdrahtError>;
    pub async fn user_property(&self, key: &str)
        -> Result<Option<UserPropertyEntry>, KupferdrahtError>;
}
```

CLI subcommands added in 10c:

```
kupferdraht-cli features                          # brief list of every config
kupferdraht-cli features --name fileSharing       # detail for one config
kupferdraht-cli properties                        # brief list of every property
kupferdraht-cli properties --key WIRE_RECEIPT_MODE  # detail for one property
```

`docs/library.md` §Server-administered configuration and
`docs/cli.md` §features / §properties are updated in the same
commit per CLAUDE.md Freshness rule. UDL surface for the new
accessors stays in 10e (no prior step has populated `kupferdraht.udl`
either).

10c integration tests gain a fifth case
(`client_accessors_surface_persisted_configs_and_properties`) that
exercises the four accessors against a populated store after login.

#### 10d — Inbound event dispatch

Three new arms in `sync/dispatch.rs`. The pattern mirrors Step
6-II's per-content-variant dispatchers — extract envelope, perform
network + persist, emit no `TimelineDiff` (these are not message
events; consumers query `Client::feature_config()` on demand).

```rust
pub(super) async fn dispatch_feature_config_update(
    &self,
    client: &Arc<Client>,
    ev: &serde_json::Value,
) -> Result<(), KupferdrahtError> {
    // Wire ships the event without the new value — re-fetch the
    // entire configs response and let replace_feature_configs
    // sync the local table.
    let raw = client.http().fetch_feature_configs(
        client.api_version(),
        &client.access_token(),
    ).await?;
    if let Some(store) = client.store_handle() {
        store.replace_feature_configs(&raw.into_entries(current_ms())).await?;
    }
    Ok(())
}

pub(super) async fn dispatch_user_property_set(
    &self,
    client: &Arc<Client>,
    ev: &serde_json::Value,
) -> Result<(), KupferdrahtError> {
    let key = ev.get("key").and_then(|k| k.as_str())
        .ok_or_else(|| KupferdrahtError::Encoding(
            "user.properties-set event missing key".into()))?;
    let value = client.http().fetch_property_value(
        client.api_version(),
        &client.access_token(),
        key,
    ).await?;
    if let Some(store) = client.store_handle() {
        store.upsert_user_property(&UserPropertyEntry {
            key: key.to_string(),
            value,
            last_synced_at_ms: current_ms(),
        }).await?;
    }
    Ok(())
}

pub(super) async fn dispatch_user_property_delete(
    &self,
    client: &Arc<Client>,
    ev: &serde_json::Value,
) -> Result<(), KupferdrahtError> {
    let key = ev.get("key").and_then(|k| k.as_str())
        .ok_or_else(|| KupferdrahtError::Encoding(
            "user.properties-delete event missing key".into()))?;
    if let Some(store) = client.store_handle() {
        store.delete_user_property(key).await?;
    }
    Ok(())
}
```

The dispatch_event main router (in `sync/service.rs`) gains three
new match arms before the catch-all `debug!(event_type = other,
"skipping non-MLS event")` and routes to the methods above.

TDD checklist:

- Event with valid payload triggers a re-fetch (asserted via
  httpmock hit count).
- Event with missing `key` produces `KupferdrahtError::Encoding` and
  is logged via `on_sync_error`; sync drain continues.
- `feature-config.update` followed by a re-read returns the new
  value without a fresh login.

#### 10e — Typed getters

The four raw `Client` accessors (`feature_configs`,
`feature_config`, `user_properties`, `user_property`) shipped in
10c — see the addendum above. 10e adds the **two typed getters**
(`Client::file_sharing` / `Client::wire_receipt_mode`) and the
`FileSharingPolicy` / `ReceiptMode` enums layered over the existing
raw entries.

UDL surface deferred — `kupferdraht.udl` is currently empty
(`namespace kupferdraht {};`); no prior step has populated it. The
UniFFI surface is bootstrapped but unconsumed, awaiting a
workspace-wide UDL pass after the Rust public API stabilises. Step
10's typed getters will land in that pass alongside every other
public type from Steps 1-10.

`FileSharingPolicy::from_raw` parses the JSON config object:

```rust
impl FileSharingPolicy {
    pub(crate) fn from_entry(entry: &FeatureConfigEntry) -> Self {
        let status = entry.status.as_deref();
        match status {
            Some("disabled") => Self::Disabled,
            Some("enabled") | None => {
                // Look at config.allowedTypes.
                serde_json::from_str::<serde_json::Value>(
                    &entry.config_json,
                ).ok()
                    .and_then(|c| c.get("allowedTypes").cloned())
                    .and_then(|t| serde_json::from_value::<Vec<String>>(t).ok())
                    .map(Self::EnabledSome)
                    .unwrap_or(Self::EnabledAll)
            }
            Some(other) => {
                warn!(status = %other,
                    "unknown fileSharing status; assuming EnabledAll");
                Self::EnabledAll
            }
        }
    }
}
```

`ReceiptMode::from_raw` parses the value field — Wire's
`WIRE_RECEIPT_MODE` is `"0"` / `"1"`, sometimes `"\"0\""` /
`"\"1\""` depending on the client that set it. The parser strips
optional surrounding quotes before matching.

The two typed getters return defaults (`EnabledAll`, `On`) when
the underlying entry is missing — see ADR-0012 §Decision.

CLAUDE.md §Functional Scope is amended in this sub-step:

```diff
- **In scope: Wire's full text-messaging feature set...**
+ **In scope: Wire's full text-messaging feature set, plus
+  server-administered configuration awareness.**
```

This is a small expansion — it does not change what messages
Kupferdraht sends or receives, only that Kupferdraht becomes aware of the
team's policy on those messages. Asset content support (Step 11)
will further expand this scope per ADR-0011.

#### 10f — Live-smoke + typed-getter integration

The `features` / `properties` CLI subcommands shipped in 10c — see
the 10c addendum above. 10f remains the **live-smoke gate** plus
the one-line `Timeline::mark_as_read` gate that consumes
`Client::wire_receipt_mode()` (added in 10e).

Live-smoke against Wire production:

1. Login with a fresh device. Run `kupferdraht-cli features` — the list
   should contain at least 5 named features. Full Wire production
   teams expose 30+ entries (live-smoke 2026-05-08 against
   `wire.com`: 33 entries — every config from `allowedGlobalOperations`
   through `validateSAMLemails`); staging or trimmed-down team plans
   may show fewer. `fileSharing` must be present and parseable.
2. Run `kupferdraht-cli features --name fileSharing`. Output shows the
   parsed mime-type allow-list (or `EnabledAll` / `Disabled`).
3. Run `kupferdraht-cli properties`. Output reflects whatever the user
   has set via Wire-Web. Typical accounts have `WIRE_RECEIPT_MODE`
   plus 0–2 other keys.
4. Toggle `WIRE_RECEIPT_MODE` via Wire-Web. Wait for sync. Re-run
   `kupferdraht-cli properties --key WIRE_RECEIPT_MODE`. New value is
   reflected within one sync cycle (≤ 30 s).
5. Check that `Client::wire_receipt_mode()` matches; if `Off`,
   subsequent `Timeline::mark_as_read` calls now skip emission
   (verify via wire production not seeing the read-receipt event).
   This last point is a Step-10 *follow-on* polish, not a 10f
   gate: implement the gate as a one-line addition in
   `Timeline::mark_as_read` and assert via integration test, but
   the live-smoke for Step 10 itself stops at "value is reflected
   in storage".

### Store additions

`feature_configs` and `user_properties` tables (V18) — see 10b.

### Implementation notes

- **Concurrent fetches.** Login-time properties fetching could
  parallelise the per-key calls. V1 uses sequential awaits to
  keep `LoginPipeline` simple and predictable; if login latency
  becomes a real concern (typical accounts have ≤ 5 properties,
  so latency is bounded), parallelise via `futures::future::join_all`
  in a follow-up.

- **Event ordering.** `feature-config.update` events arrive on the
  same notification stream as MLS events. Step 5d's polling loop
  drains them in order. There is no MLS event that depends on a
  feature-config event ordering, so no synchronisation required.

- **Stale data on logout.** `Client::logout(--purge-device)` (Step
  6e-2) wipes `~/.kupferdraht/`, which removes the SQLite database
  including these tables. Soft logout retains them, which is
  correct — the next resume reuses the cached configs until the
  next event-driven update.

- **Schema-evolution on Wire's side.** Wire periodically adds new
  feature-config keys and new fields to existing values. The
  generic JSON-blob storage absorbs both transparently; only typed
  getters need updating, and only when Kupferdraht decides to react to
  the new field.

- **Login-critical-path latency risk (kept simple in V1).**
  `LoginPipeline::fetch_server_configs` runs sequentially on the
  login critical path. A slow or partially degraded `/v14/feature-configs`
  response slows login by exactly that amount. The "warn-and-continue"
  posture (ADR-0012 §Login pipeline integration) catches outright
  errors but does not bound latency for slow-but-eventually-2xx
  responses. If Wire's p99 on these endpoints turns out to be
  meaningful in production smoke, the follow-up is to move
  `fetch_server_configs` off the critical path: spawn it as a tokio
  task after the access-token rebind, return Login as soon as the
  KP upload completes, have getters serve cached/default values
  until the background fetch lands, and cancel the task on
  `Client::logout`. This is intentionally deferred — V1 picks the
  simpler sequential model and accepts the risk.

- **Live-fixture capture (follow-up to first 10c live-smoke).**
  10a's tests use a synthesised representative response derived from
  ADR-0012 §Context plus the Swagger schema. After the first
  successful 10c live-smoke against Wire production, capture the
  real `/v14/feature-configs` and `/v14/properties[/{key}]` response
  bodies, anonymise team / user identifiers, and commit them under
  `kupferdraht-core/tests/fixtures/feature_configs/` as the canonical
  parser fixtures. Replace the synthesised payloads in
  `wire/http/feature_configs.rs::tests` with `include_str!()` against
  the captured files. The synthesised fixture is allowed only until
  this swap.

- **`mls.config.allowedCipherSuites` is persisted but not consumed
  in V1 (TODO future ADR-0013).** Step 10's generic store accepts
  every feature config Wire returns, including `mls.config.defaultCipherSuite`
  and `mls.config.allowedCipherSuites`. Kupferdraht currently hardcodes
  `MlsClient::MLS_CIPHERSUITE` to P-256. If a team's MLS feature
  config requires a different ciphersuite, KeyPackages and group
  joins generated by Kupferdraht will be rejected by the MLS layer
  silently. ADR-0012 explicitly defers ciphersuite negotiation to a
  future ADR-0013; this is acknowledged here so the next reader
  finds the path. A `// TODO(ADR-0013): consume mls.config.*`
  marker is added in `wire/http/feature_configs.rs` at the parsing
  boundary so the future-self lands on it via grep.

### TDD starter tests

Cross-cutting integration tests:

- `tests/step_10_login_populates_configs.rs` — asserts the V18
  tables are non-empty after a `ClientBuilder::login` against an
  httpmock'd Wire backend.
- `tests/step_10_event_refetch.rs` — synthetic
  `feature-config.update` event in the mocked notification stream
  triggers a refetch and updates the table; same for user-property
  events.
- `tests/step_10_login_fault_tolerant.rs` — login proceeds when
  `/feature-configs` returns 500; downstream getters return
  defaults.
- `tests/step_10_typed_getters.rs` — `FileSharingPolicy::from_entry`
  and `ReceiptMode::from_raw` round-trip Wire-shape fixtures
  including `Disabled`, `EnabledAll`, `EnabledSome`, missing
  entry, malformed JSON.

### Dependencies

- All earlier Steps (1–9) merged.
- `LoginPipeline` infrastructure from Step 1.
- `sync/dispatch.rs` from Step 5d / 6-II.
- No new external crates. `serde_json::Value` (already in tree),
  the existing storage stack, and `tracing` cover everything.

### Validation gates

This step is "complete" when:

1. All sub-step TDD lists are green.
2. Live-smoke 1 + 2 + 3 + 4 above pass against Wire production.
3. `docs/library.md` carries the new public-API surface.
4. `docs/api-mapping-matrix-rust-sdk.md` carries the new drift
   entry.
5. `docs/wire/api-notes.md` promotes `/feature-configs` and
   `/properties` from "does not yet use" to documented.
6. The CLAUDE.md §Functional Scope amendment described in 10e is
   committed.

### CLI commands

| Command | Options |
|---|---|
| `features` | (no flags — list all feature configs with status) |
| `features --name <name>` | show one feature config in detail |
| `properties` | (no flags — list all user properties) |
| `properties --key <key>` | show one user property in detail |
