# Step 14 — matrix-rust-sdk parity polish

> **Status:** merged — all sub-steps merged 2026-05-21/22. 14a `d6340ae` (DM convenience, live 2026-05-21; CHANGELOG.md §41–§42); 14b `abca6ac` (sync-surface refactor + `sync_members` rename, live 2026-05-21; CHANGELOG.md §40, §46); 14c `b8e24a6` (filtered room-list accessors; typed event-handler half deferred); 14d (`Room::access()` + `RoomAccess`, last public `fetch_*` retired, live 2026-05-22 via `scripts/smoke-step14.sh`); 14e (mutation methods return `Arc<Room>`, live 2026-05-22, closes `v1-api-ergonomics-audit.md §1.5`); 14f (`Timeline::subscribe` → `Subscription` handle, drop-based unsubscribe, closes `v1-known-simplifications.md §2.7`); 14g (auto-hydration `room_or_load` / `send_text_auto_join`, closes audit §1.1 + §1.2, typed `KupferdrahtError::MlsGroupMissing`). Archived from PLAN.md.
>
> **Sub-steps:** 14a (DM convenience — `dm_room`, `create_dm`,
> `dm_with`; all take a single qualified `peer: &str` per
> Kupferdraht's existing `Client::room(id)` convention; CHANGELOG.md §42), 14b (sync-surface refactor — matrix-rust-sdk-shaped
> `sync_once` / `sync` / `sync_with_callback` / `sync_stream`
> family + `refresh_members` → `sync_members` rename), 14c (typed
> event handlers + filtered room listings), 14d (move room-scoped
> accessors onto `Room` and retire the last `fetch_*` symbols —
> e.g. `Client::fetch_room_access` → `Room::access()`; see
> CHANGELOG.md §41 for the verb taxonomy that motivates this),
> 14e (mutation methods return `Arc<Room>` — retrofit
> `invite_members` / `kick_members` / `set_room_access` /
> `rename_room` to match 14a's `create_dm` precedent; closes
> audit §1.5), 14f (`Timeline::subscribe` returns a `Subscription`
> handle for unsubscribe — matches matrix-rust-sdk's handler
> lifecycle pattern; closes v1-known-simplifications §2.7),
> 14g (auto-hydration convenience — `Client::room_or_load(id)`
> and `Timeline::send_text_auto_join`; generalises the
> `dm_with`-style pattern beyond DMs; closes audit §1.1 + §1.2).
> Added 2026-05-21 — finding §44 records the rationale (Step 14
> implementation already in progress; new sub-steps appended,
> not interleaved into 14a–d).
>
> **Historical names:**
> - "Step 14" — current.
>
> **Branch / commit prefix:** `step/14*-*`, `feat(step-14*): …`,
> `chore(step-14*): …`.

---

## Step 14 — matrix-rust-sdk parity polish

Post-Step-11 audit of matrix-rust-sdk's public API surface against
Kupferdraht's current public API surfaced four medium- and low-priority
ergonomics gaps that did not block any prior step but are worth
closing now that the functional scope (text + assets + config) is
complete.

The audit also surfaced two matrix-rust-sdk symbols whose Wire
analogues do not exist (see §"Out of scope — Wire-model mismatch"
below). Those are not gaps but model differences and explicitly
remain unimplemented.

### matrix-rust-sdk analog

- `Client::create_dm(&UserId) async -> Result<Room>` — create or
  fetch the canonical 1:1 room with a peer.
  Source: `crates/matrix-sdk/src/client/mod.rs::Client::create_dm`.
- `Client::get_dm_room(&UserId) -> Option<Room>` — local lookup
  for an existing DM room.
  Source: `crates/matrix-sdk/src/client/mod.rs::Client::get_dm_room`.
- `Client::sync(SyncSettings)`, `Client::sync_once(SyncSettings)`,
  `Client::sync_with_callback(SyncSettings, F)`,
  `Client::sync_stream(SyncSettings) -> impl Stream<…>` — the
  four-method sync family. matrix-rust-sdk takes a `SyncSettings`
  on each and returns `SyncResponse`; Kupferdraht parallels the shape
  with a `SyncSettings { poll_interval }` and a `SyncSummary`
  return (see CHANGELOG.md §40 for the `timeout` → `poll_interval`
  rename and the long-poll model difference).
- `Client::add_room_event_handler<T>(handler)` — typed handler
  generic over a specific event content type.
- `Client::joined_rooms()`, `Client::invited_rooms()`,
  `Client::rooms_filtered(…)` — filtered room-list accessors.
- `Room::sync_members() async` — server-side refresh of one
  room's members. matrix-rust-sdk owns the exact name; Kupferdraht
  today has `Client::refresh_members(room_id)` which 14b renames
  (`refresh_*` retires from the public verb vocabulary —
  CHANGELOG.md §41).
- `Room::*` accessor methods generally — matrix-rust-sdk holds
  room-scoped operations on `Room`, not on `Client`. Kupferdraht's
  `Room` is currently methodless. 14d begins the migration with
  `Room::access()` (replacing `Client::fetch_room_access`) and
  retires the last `fetch_*` symbol from the public API.
- `Client::invite_user` / `Client::kick_user` / `Room::set_name`
  in matrix-rust-sdk return the affected `Room` (or an updated
  variant). 14e brings Kupferdraht's `invite_members` /
  `kick_members` / `set_room_access` / `rename_room` to the
  same shape — they currently return `Result<()>` and force
  callers to re-look-up. Mirrors 14a's `create_dm` precedent.
- `EventHandlerHandle` in matrix-rust-sdk is the opaque handle
  `Client::add_event_handler` returns; dropping it removes the
  handler. 14f introduces the equivalent `Subscription` for
  `Timeline::subscribe`, addressing the lifecycle gap recorded
  in `v1-known-simplifications.md §2.7`.
- matrix-rust-sdk auto-hydrates room state from the store on
  client startup (consumers don't see a "cold cache"). Kupferdraht's
  store hydration is explicit (`Client::load_rooms_from_store`).
  14g doesn't change that posture but adds opt-in
  `room_or_load` / `send_text_auto_join` variants for callers
  who want the lazy-load behaviour without managing it
  themselves — closes audit §1.1 + §1.2.

### Sub-steps

| Sub-step | Scope | Wire API v14 | Value |
|---|---|---|---|
| 14a | DM convenience — `Client::create_dm(peer)` (GET, fetch-or-establish), `Client::dm_room(peer)` (local lookup), `Client::dm_with(peer)` (lookup+create-if-missing, Wire-extension). All three take a single `peer: &str` in qualified-string form `"uuid@domain"`, matching Kupferdraht's existing `Client::room(id)` convention. No `get_` prefix per Rust API Guidelines C-GETTER. Wire's 1:1 conversations are first-class server-side, unlike Matrix's `is_direct`-flagged room. **Merged 2026-05-21** — live-validated against Wire production. | `GET /v14/one2one-conversations/{usr_domain}/{usr}` (single endpoint; the swagger-documented POST returns nginx 404 in production — see `docs/wire/api-notes.md` §"Step 14a discovery") | medium |
| 14b | Sync-surface refactor: collapse `Client::sync_metadata` and `SyncService::drain_pending` into the matrix-rust-sdk-shaped family `Client::sync_once` / `sync` / `sync_with_callback` / `sync_stream`, all carrying `SyncSettings { poll_interval }`. Wires `GET /v14/conversations` + `GET /v14/notifications` get composed inside `sync_once` — callers see one operation, not Wires server-side split. Renames the CLI: drop `sync-fetch`, keep `sync` (= one iteration) and `sync-stream` (= loop). Also retires the last `refresh_*` verb in the public API: `Client::refresh_members` / `Room::refresh_members` → `sync_members` (matrix-rust-sdk uses this exact name). **Merged 2026-05-21** — live-validated against Wire production. | `GET /v14/conversations*` + `GET /v14/notifications` (composed internally), targeted `GET /v14/.../members` for `sync_members` | medium |
| 14c | Typed event handlers (`Client::add_room_event_handler<E: TypedEventContent>(…)`) + filtered room-list accessors (`Client::joined_rooms()`, `Client::invited_rooms()`, `Client::rooms_filtered(F)`). Both are thin wrappers over today's `EventHandler` trait and `Client::rooms()`. **Merged 2026-05-21** — filter-accessors only; typed event handlers deferred (open design question in §"Open questions" #2). | none (local wrappers) | low |
| 14d | Move room-scoped accessors that currently live on `Client` onto `Room`, where matrix-rust-sdk holds them, and retire the last `fetch_*` symbols from the public API. Concrete first target: `Client::fetch_room_access(room_id)` → `Room::access()`; `Room::access()` does the same `POST /v{N}/conversations/list` under the hood. `Room` today is a method-less data snapshot; 14d introduces methods on it. Sized to absorb later room accessors as they appear; landing target is post-14a/b/c. | `POST /v{N}/conversations/list` (for `Room::access`); other Wire calls as room methods land | low (cleanup, not blocking) |
| 14e | Mutation methods return the updated `Arc<Room>`. Retrofit of `Client::invite_members`, `Client::kick_members`, `Client::set_room_access`, `Client::rename_room` (and `Client::leave_room` / `Client::join_room` if symmetric) — each currently returns `Result<(), KupferdrahtError>`, forcing callers to re-look-up the room. Consistent with 14a's `create_dm` precedent and matrix-rust-sdk-shape (Matrix mutators return the affected entity). Additive in practice: existing callers ignore the return with `_ = client.invite_members(...).await?;`. Closes `v1-api-ergonomics-audit.md §1.5`. | none (no new endpoints; existing mutation HTTP calls return the same data; cache update + return) | medium |
| 14f | `Timeline::subscribe` returns a `Subscription` handle whose `Drop` impl unregisters the listener (or explicit `.unsubscribe()` method). Today the listener is registered for `Client` lifetime and cannot be released — a consumer that switches UI between rooms can't free its slot. matches matrix-rust-sdk's handler-lifecycle pattern (their `add_event_handler` returns a handle drop-cancelled). Closes `v1-known-simplifications.md §2.7`. Lands alongside 14c (same typed-handler territory). | none (pure library lifecycle work) | low |
| 14g | Auto-hydration convenience methods: `Client::room_or_load(room_id)` falls back to `load_rooms_from_store` on cache miss (then retries); `Timeline::send_text_auto_join(...)` probes for MLS state and calls `Client::join_room` first when absent. Generalises 14a's `dm_with` pattern beyond DMs. Explicit-lookup / explicit-join variants stay for callers who want non-blocking lookup or want to decline an invite. Closes `v1-api-ergonomics-audit.md §1.1` + `§1.2`. | none (composes existing methods) | medium (the audit's original §1.2 trigger-case lives here) |

### Public API

```rust
impl Client {
    // 14a — DM convenience. Single qualified-peer parameter
    // matches Kupferdraht's existing convention (`Client::room(id)`,
    // `Client::join_room(id)`, etc. — all take a qualified
    // string `"uuid@domain"`). Wire's native shape is two-part
    // (URL path segments + JSON `{ id, domain }`), translated
    // at the wire/ HTTP layer. See CHANGELOG.md §42 for the
    // decision rationale.
    pub async fn create_dm(&self, peer: &str)
        -> Result<Arc<Room>, KupferdrahtError>;

    pub fn dm_room(&self, peer: &str) -> Option<Arc<Room>>;

    /// Look up first, create if missing. Kupferdraht-specific
    /// convenience — matrix-rust-sdk has no analog.
    /// Documented as Wire extension in
    /// `docs/api-mapping-matrix-rust-sdk.md`.
    pub async fn dm_with(&self, peer: &str)
        -> Result<Arc<Room>, KupferdrahtError>;

    // 14b — Sync-surface refactor: matrix-rust-sdk-shaped family.
    // sync_once composes a conversations-metadata refresh
    // (Client::sync_metadata, public) with one notifications
    // drain (SyncService::drain_pending, crate-internal). The
    // public sync surface is the four-method matrix-rust-sdk-shaped
    // family PLUS the standalone sync_metadata for callers that
    // want metadata-only refresh without the decrypt loop —
    // see CHANGELOG.md §46 for the three-tier composability
    // rationale (load_rooms_from_store / sync_metadata / sync_once).
    pub async fn sync_once(&self, settings: SyncSettings)
        -> Result<SyncSummary, KupferdrahtError>;

    pub async fn sync(&self, settings: SyncSettings)
        -> Result<(), KupferdrahtError>;

    pub async fn sync_with_callback<F>(
        &self,
        settings: SyncSettings,
        callback: F,
    ) -> Result<(), KupferdrahtError>
    where
        F: Fn(SyncSummary) -> std::ops::ControlFlow<()> + Send + 'static;

    pub fn sync_stream(&self, settings: SyncSettings)
        -> impl Stream<Item = Result<SyncSummary, KupferdrahtError>>;
}

pub struct SyncSettings {
    /// Backoff between empty `/v14/notifications` polls in the
    /// loop variants. Ignored in `sync_once`. Default: 1 s.
    /// See CHANGELOG.md §40 — Wire is not a server-side long-poll,
    /// so this is client-side pacing, not server-side hold.
    pub poll_interval: Option<Duration>,
}

impl Client {
    // 14c — Filtered room listings
    pub fn joined_rooms(&self) -> Vec<Arc<Room>>;
    pub fn invited_rooms(&self) -> Vec<Arc<Room>>;
    pub fn rooms_filtered<F: Fn(&Room) -> bool>(&self, f: F)
        -> Vec<Arc<Room>>;

    // 14c — Typed event handlers (open question: signature TBD,
    // depends on how Kupferdraht models inbound event variants —
    // see "Open questions" below)

    // 14e — Mutation methods return updated Arc<Room>.
    // Existing signatures returning Result<(), _> retrofitted to
    // Result<Arc<Room>, _>. Callers can ignore the return value
    // with `_ = client.invite_members(...).await?;` so the change
    // is additive in practice.
    pub async fn invite_members(&self, room_id: &str, user_ids: &[String])
        -> Result<Arc<Room>, KupferdrahtError>;
    pub async fn kick_members(&self, room_id: &str, user_ids: &[String])
        -> Result<Arc<Room>, KupferdrahtError>;
    pub async fn set_room_access(&self, room_id: &str, modes: Vec<Access>, roles: Vec<AccessRole>)
        -> Result<Arc<Room>, KupferdrahtError>;
    pub async fn rename_room(&self, room_id: &str, new_name: &str)
        -> Result<Arc<Room>, KupferdrahtError>;

    // 14g — Auto-hydration convenience.
    pub async fn room_or_load(&self, room_id: &str) -> Option<Arc<Room>>;
}

impl Timeline {
    // 14f — Subscription handle. Drop the handle to unsubscribe.
    pub fn subscribe(&self, listener: Arc<dyn TimelineListener>) -> Subscription;

    // 14g — Auto-join + send variant.
    pub async fn send_text_auto_join(&self, body: &str) -> Result<(), KupferdrahtError>;
}

/// 14f — opaque handle returned by `Timeline::subscribe`.
/// Listener is unregistered when this is dropped, or when
/// `unsubscribe()` is called explicitly.
pub struct Subscription { /* opaque */ }
impl Subscription {
    pub fn unsubscribe(self);
}
```

Each item updates [`docs/api-mapping-matrix-rust-sdk.md`](../api-mapping-matrix-rust-sdk.md)
§"Pending parity items (Step 14)" with the appropriate status row
when it lands.

### Wire Message Types

None — Step 14 is API ergonomics, not new message content. The
underlying GenericMessage envelopes used by sub-steps 14b/14c are
the same as those already implemented in Steps 4–6-II + 11.

### Implementation notes

**14a — `peer: &str` qualified-string parameter, single value.**
All three 14a methods take `peer: &str` in the qualified form
`"uuid@domain"`. This matches Kupferdraht's existing convention
(`Client::room(id)`, `Client::join_room(id)`, `Client::leave_room(id)`,
etc.) — internal parse via `QualifiedId::from("uuid@domain")` at
the HTTP layer translates to Wire's native two-part shape for
the wire request. Wire's protocol-level shape is unambiguously
two-part (URL path `/v14/.../{domain}/{id}` and JSON
`{ id, domain }`); the flattening is a deliberate public-API
simplification, documented in CHANGELOG.md §42 and the
api-mapping doc Open Questions section. A future typed-identifier
pass (`kupferdraht::ids::UserId(String)`) would tighten the type
without changing the caller-facing shape.

**14a — `create_dm` is `GET /v14/one2one-conversations/{usr_domain}/{usr}`.**
**Live-validated 2026-05-21 against Wire production** — Wire
deploys only the GET route. The swagger-documented
`POST /v14/one2one-conversations` returns nginx 404 at the edge
(HTML body, not Wire's JSON error shape), meaning the path never
reaches the Wire app server. Recorded in
`docs/wire/api-notes.md` §"Step 14a discovery". Kalium's
`ConversationApiV7` drives the same single GET, confirming the
production behaviour.

The GET endpoint is **server-side idempotent**: the first call
against a fresh `(self, peer)` pair registers the conversation,
every subsequent call returns the existing one. Same call shape
for "create" and "fetch" — `create_dm` therefore models both
flows as a single
`HttpClient::fetch_one_to_one_conversation(domain, uuid)` call
internally.

Response wraps the conversation in
`MlsOne2OneConversation { conversation: OwnConversation,
public_keys: MlsKeysByPurpose }`. Kupferdraht discards the
`public_keys` echo (already fetched at login from
`/v14/mls/public-keys`) and surfaces the flat `OwnConversation`
to the caller as `Arc<Room>`.

The `peer: &str` parameter is parsed via `split_once('@')` at
the HTTP layer to fill the two URL path segments `{usr_domain}`
and `{usr}` (Wire's native two-part shape).

**14a — `create_dm` skips `MlsClient::create_group` when the
local MLS group already exists.** Live-smoke finding 2026-05-21:
because the GET is server-side idempotent, a returning caller on
the same device gets back a conversation whose `group_id` is
already in the local openmls store. Unconditionally calling
`MlsClient::create_group(group_id)` then fails with
`MlsGroup::new_with_group_id: GroupAlreadyExists`. Fix: gate
the call on `MlsClient::group_exists(group_id)` first; only
bootstrap the local group on a true cold start.

**14a — `dm_room` is a local lookup**, not a Wire call. Kupferdraht
already persists `room_type = "direct_message"` (`storage/records.rs:69`,
`rooms/room.rs:37`) and `counterpart: Option<RoomMember>`
(`rooms/room.rs:83`). Implementation iterates the in-memory room
list and returns the room whose `counterpart` matches the
qualified `peer` string (compared post-parse). No async, no
network.

**14a — `dm_with` is the "lookup, create if missing" convenience.**
Kupferdraht-specific — matrix-rust-sdk has no direct analog because
Matrix' DM model is `is_direct`-flagged room state, not a
first-class server endpoint. `dm_with` calls `dm_room(peer)`
first; on `Some`, returns it; on `None`, falls through to
`create_dm(peer)`. The convenience exists because the
lookup-then-create pattern is the dominant caller shape (see
audit §1.1 auto-hydration class). Documented as Wire-extension in
`docs/api-mapping-matrix-rust-sdk.md`.

**Concurrent `dm_with` against the same peer de-duplicates.**
Implementation uses a per-peer `tokio::sync::Mutex` map so two
parallel `dm_with(peer)` calls do not issue two GETs — the
second waiter observes the in-flight call's result. (Wire's GET
endpoint is idempotent, so a duplicate call would not corrupt
state, but it would waste a round-trip and could confuse local
state-update ordering.)

**14a — CLI handlers hydrate from the SqliteStore at process
start.** Live-smoke finding 2026-05-21: each CLI invocation is
a fresh process, so the in-memory `Client::rooms` cache starts
empty. `kupferdraht-cli dm-show` (strict local lookup) therefore
always reports "no cached DM" right after a successful
`dm-create`. `kupferdraht-cli dm-with` works but always falls
through to `create_dm` (one wasted GET on every cache-hit
case). Fix: call `Client::load_rooms_from_store().await?` at
the top of both handlers — hydrates the in-memory cache from
the SqliteStore mirror (last `kupferdraht-cli sync` snapshot), no
network. `dm-create` is unchanged: it always goes to Wire so
cache state is irrelevant on entry. This is the per-process
cold-cache pattern Kupferdraht's CLI hits across other
subcommands too; tracked as audit §1.1 / 14g (general
`Client::room_or_load`).

**14b — Sync-surface refactor + Stream variant.** Today the
public sync surface is two methods with overlapping prefixes
(`Client::sync_metadata`, `Client::sync()` → `Arc<SyncService>`)
plus a service-level method (`SyncService::drain_pending`) that
together expose Wires internal split between
`GET /v14/conversations` and `GET /v14/notifications`. 14b
collapses this into the matrix-rust-sdk-shaped four-method family
on `Client`:

- `sync_once(settings)` — one iteration. Internally composes
  the conversations refresh + one notifications drain + cursor
  advance via `EventAcknowledgeRequest`. Returns `SyncSummary`
  aggregating "N rooms updated, M events dispatched".
- `sync(settings)` — drives `sync_once` in a loop with
  `settings.poll_interval` backoff between empty iterations,
  returns when cancelled.
- `sync_with_callback(settings, F)` — same loop with a
  per-iteration callback returning `ControlFlow<()>`; this is
  the UDL-friendly entry point (Open Question 3 below).
- `sync_stream(settings)` — same loop forwarded over an
  `mpsc::channel`; dropping the stream cancels the underlying
  task. Rust-side ergonomic.

`SyncService` stays in the crate as an internal type — it owns
the per-iteration composition logic so all four `Client::sync*`
methods share state (cursor + EventHandler dispatch). No external
caller constructs or holds a `SyncService` anymore.

`SyncSettings` carries `poll_interval: Option<Duration>` and
nothing else for now. The name and semantics divergence from
matrix-rust-sdk's `timeout` is documented in CHANGELOG.md §40:
Wire's `/v14/notifications` is not a server-side long-poll, so
the field controls client-side pacing between empty polls, not
server-side hold.

The existing `Client::add_event_handler` (unchanged from Step 5c)
keeps working — dispatch happens inside `sync_once` on every
iteration, regardless of which entry point the caller used.

**14b — `refresh_members` → `sync_members` rename.** The last
`refresh_*` verb in the public API is retired in the same pass.
matrix-rust-sdk uses `Room::sync_members()` for the targeted
"re-fetch this room's members from the server" operation — the
exact same name, the exact same caller contract. Kupferdraht's
`Client::refresh_members(room_id)` and the `Room::refresh_members`
wrapper both rename to `sync_members`; the underlying
`/v14/.../members` GET is unchanged. After this rename the
public API has exactly one verb for "pull from server":
`sync_*`. The audit row in `docs/api-mapping-matrix-rust-sdk.md`
line 210 flips from "Aligned" to "Pending — Step 14b" until the
rename lands. Verb-taxonomy rationale recorded in CHANGELOG.md §41.

**14b — CLI rename**. The four-method library family replaces
three CLI subcommands today (`sync`, `sync-fetch`, `sync-stream`):

- `kupferdraht-cli sync` → calls `Client::sync_once(SyncSettings::default())`.
  Was: `Client::sync_metadata()`. New semantics: also drains
  pending notifications in one shot, not just metadata refresh.
- `kupferdraht-cli sync-fetch` → **removed**. Was redundant with the
  new `sync` semantics (one iteration, includes notifications).
- `kupferdraht-cli sync-stream` → calls `Client::sync(settings)` with
  `poll_interval` from the existing `--idle-backoff` flag.

`cli.md` is updated in the same commit per the Freshness rule.

**14c — Filtered room lists are local cache projections.** Today's
`Client::rooms()` returns `Vec<Arc<Room>>`. 14c adds three
specialised filters; the implementation is one filter expression
each (`r.is_joined()` / `r.is_invited()` / `f(r)`). No new state.

**14c — Typed event handlers are deferred design.** matrix-rust-sdk's
`add_room_event_handler<T>` uses Matrix's typed event content
registry (`m.room.message`, `m.room.member`, etc.). Kupferdraht's inbound
event model dispatches via a single `EventHandler` trait with a
match on `InboundEvent`. Whether Kupferdraht gains a parallel
`add_event_handler<T>(…)` surface — and how to plumb it without
duplicating the dispatch logic — is an open design question
included in 14c's TDD-precursor discussion.

**14d — Move room-scoped accessors onto `Room`; retire `fetch_*`.**
Kupferdraht's `Room` is currently a method-less data snapshot — every
operation on a room goes through a `Client::*_room(room_id, ...)`
function. matrix-rust-sdk does the opposite: room-scoped methods
live on `Room`. 14d starts closing this gap, taking
`Client::fetch_room_access(room_id) -> Result<RoomAccess>` as the
first concrete target because it is also the last `fetch_*` symbol
on the public API surface (every other `fetch_*` is `pub(crate)`
HTTP plumbing).

After 14d:
- `Client::fetch_room_access(room_id)` is removed.
- `Room::access() async -> Result<RoomAccess>` lands in its place,
  doing the same `POST /v{N}/conversations/list` under the hood.
  Caller pattern goes from `client.fetch_room_access(&id).await?`
  to `client.room(&id).unwrap().access().await?`.

This is the seed of a larger `Room`-method migration: future
candidates as room methods land include `Room::leave()` (delegates
to `Client::leave_room`), `Room::invite(&[user_ids])`,
`Room::rename(&str)`, and similar. 14d is intentionally scoped
narrow — one removal, one new method — to validate the migration
pattern before broader rollout. Cleanup item, no functional gain
for callers; the win is verb-taxonomy purity (the `fetch_*` family
disappears entirely from the public API) and matrix-rust-sdk-shape
adherence.

Tracked separately from 14a/b/c because it lands **after** them —
the sync rename in 14b touches more code, and 14a's DM methods
should land at the `Client` level first before any room-method
migration begins. Verb-taxonomy rationale: CHANGELOG.md §41.

**14e — Mutation methods return updated `Arc<Room>`.** Today
`invite_members`, `kick_members`, `set_room_access`, `rename_room`
all return `Result<(), KupferdrahtError>`. After the mutation succeeds
on the server, the local cache is refreshed (via `sync_metadata`
post-call). Callers who want to read the new state then do
`client.room(&room_id).await.ok_or_else(...)` — a small re-lookup
ceremony at every call site.

14a's `create_dm(peer) -> Result<Arc<Room>, _>` set the precedent.
14e brings the existing mutations to the same shape. The post-
mutation cache refresh stays where it is; the only change is that
the affected `Arc<Room>` is returned instead of discarded.

**Migration is additive in practice**: existing callers can
ignore the new return value with
`_ = client.invite_members(&room_id, &users).await?;` — no
behavioural change for code that didn't read the room state.
New / updated callers get the room handle directly.

If symmetry pulls in `leave_room` and `join_room` too, the same
shape applies (`leave_room` returns the room state at point of
leave; `join_room` returns the freshly-joined room). Decision
deferred to TDD discussion: do those two also fit the "return
the affected room" pattern, or do they belong in a separate
category (`leave` returns nothing because the room is gone;
`join` returns the new room — these are different)?

Closes `v1-api-ergonomics-audit.md §1.5` — drop the entry from
the audit doc in the landing commit.

**14f — `Timeline::subscribe` returns a `Subscription` handle.**
Today `Timeline::subscribe(listener)` registers the listener for
`Client` lifetime — there is no way to unregister. A consumer
that swaps UI between rooms accumulates listeners across the
session; a chat-view close that should release its slot can't.

The matrix-rust-sdk shape is `add_event_handler(handler) ->
EventHandlerHandle` — dropping the handle unregisters. 14f adapts
this to Kupferdraht's `Timeline::subscribe`:

```rust
pub struct Subscription { /* opaque */ }
impl Drop for Subscription { fn drop(&mut self) { /* unregister */ } }
impl Subscription { pub fn unsubscribe(self) { /* explicit form */ } }

impl Timeline {
    pub fn subscribe(&self, listener: Arc<dyn TimelineListener>) -> Subscription;
}
```

The dropped-handle pattern is RAII-clean and matches how Rust
consumers expect subscriptions to behave. Explicit `.unsubscribe()`
is for code that wants the action visible at the call site.

The underlying listener registry (`client.timeline_listeners`
mutex from Step 5c, currently `Vec<Arc<dyn TimelineListener>>`)
becomes a slab or `HashMap<SubscriptionId, ...>` so individual
entries can be removed without scanning. Constant-time
unregister.

Lands alongside 14c — both touch the typed-handler / listener
territory.

Closes `v1-known-simplifications.md §2.7` — drop the entry from
that audit-sibling doc in the landing commit.

**14g — Auto-hydration convenience.** Two new method variants
that probe for missing state and trigger the appropriate load:

- `Client::room_or_load(room_id) -> Option<Arc<Room>>`: tries
  `room(id)`; on `None`, calls `load_rooms_from_store` and
  retries. The explicit `room(id)` stays — for callers that
  want a non-blocking lookup with the explicit `None` semantic.
- `Timeline::send_text_auto_join(body) -> Result<(), _>`: probes
  for MLS state on the bound room; if the local openmls store
  has no group, calls `Client::join_room` first, then proceeds
  with `send_text`. Explicit `send_text` stays — for callers
  who want to handle the missing-MLS-state case themselves
  (e.g. to decline an invite gracefully).

Both methods are **opt-in convenience variants** — they don't
replace the explicit forms. The audit's framing (§1.1, §1.2) was
"the default fails surprisingly; an auto-variant should be
available". 14g adds the auto-variants without changing the
defaults.

The DM-path equivalent is 14a's `dm_with`; 14g generalises to
arbitrary rooms and to the send path. Same pattern, broader
reach.

Closes `v1-api-ergonomics-audit.md §1.1` and `§1.2` — drop both
entries from the audit doc in the landing commit.

### Out of scope — Wire-model mismatch

These matrix-rust-sdk symbols look like gaps but are not:

- **`Client::get_dm_rooms(user_id) -> Iterator<Room>`** — Wire's
  1:1 model permits at most one DM per `(self, peer)` pair (the
  `GET /one2one-conversations/{d}/{u}` endpoint returns *the* MLS
  1:1, singular). An iterator is meaningless under Wire's
  semantics. `Client::dm_room` (singular) covers the full
  surface.
- **`Room::set_room_topic(&str)`** — Wire's `Conversation` schema
  has only `name`; no `topic` or `description` field, and no
  `PUT /conversations/.../topic` endpoint exists in the v14
  swagger. The matrix concept of a separate "topic" line does
  not exist server-side and so cannot be persisted.

Plus the items explicitly out of scope per
[CLAUDE.md](../../CLAUDE.md) §"Functional Scope" (calling,
threads, typing indicators, power-level moderation, Spaces,
notification routing, …) — those remain out of scope.

### TDD starter tests

14a:
- `create_dm("uuid@domain")` calls
  `GET /v14/one2one-conversations/domain/uuid`
  (parse via `split_once('@')` at the HTTP layer fills the two
  URL path segments) and unwraps the
  `MlsOne2OneConversation { conversation, public_keys }` response
  into an `Arc<Room>`, discarding the `public_keys` echo.
- `create_dm` is idempotent: two consecutive calls against the
  same peer return the same room (Wire's GET endpoint registers
  on first call, returns existing on later calls).
- `create_dm` on a returning caller with the MLS group already
  in the local store **does not** re-invoke
  `MlsClient::create_group` (would fail with
  `GroupAlreadyExists`); gated on `group_exists(group_id)`.
- `create_dm` on an unqualified `peer` (no `@`) returns
  `KupferdrahtError::InvalidInput("peer must be qualified
  '<uuid>@<domain>'")`.
- After `create_dm`, `dm_room(peer)` returns the same
  `Arc<Room>` (object identity via `Arc::ptr_eq`).
- `dm_room` on an unknown user returns `None`.
- `dm_room` ignores rooms with `room_type != DirectMessage`
  even when the counterpart matches (defensive — a self-room
  with the queried user as the only other member must not be
  returned).
- `dm_with(peer)` returns the cached `Arc<Room>` when one
  exists (object identity proof) without issuing any HTTP
  call.
- `dm_with(peer)` on a cache miss falls through to `create_dm`
  exactly once and returns the freshly-created `Arc<Room>`.
- Two concurrent `dm_with(peer)` calls against the same peer
  do not POST twice — the second call observes the in-flight
  creation and returns the same room (test via two
  `tokio::spawn` tasks).

14b:
- `sync_once` returns a `SyncSummary` whose room-count matches a
  fresh `GET /v14/conversations` and whose event-count matches
  the notification queue drained in one iteration.
- `sync_once` advances the persisted `last_event_id` cursor by
  the events it processed; a second `sync_once` against an
  empty queue returns `SyncSummary { events: 0, .. }`.
- `sync_once` ignores `SyncSettings::poll_interval` (verified by
  measuring elapsed time — must not include the backoff).
- `sync(settings)` calls `sync_once` repeatedly and sleeps
  `poll_interval` between empty-result iterations.
- `sync_with_callback` invokes the callback exactly once per
  iteration; returning `ControlFlow::Break(())` exits the loop.
- `sync_stream` yields one `SyncSummary` per poll iteration.
- Dropping the stream cancels the underlying polling task.
- Concurrent calls to two `sync*` variants on the same cursor
  return `KupferdrahtError::SyncAlreadyRunning` (second call).
- The Step-2 callers that previously invoked `sync_metadata`
  continue to work via a deprecation alias for one release
  cycle, then are migrated to `sync_once` in a follow-up commit.
- `Client::sync_members(room_id)` (renamed from `refresh_members`)
  fetches `/v14/.../members`, updates the room's member cache,
  and returns the updated `Vec<RoomMember>`. Caller contract
  unchanged from `refresh_members`; old name removed in the
  same commit (no deprecation alias — internal-only callsite
  count is small and migrated atomically).

14c:
- `joined_rooms()` returns only rooms with `state == 'joined'`.
- `invited_rooms()` returns only rooms with `state == 'invited'`.
- `rooms_filtered(|r| r.room_type() == RoomType::Group)`
  returns groups, never DMs / channels / self-rooms.

14d:
- `Room::access().await?` returns the same `RoomAccess` shape
  that today's `Client::fetch_room_access(room_id).await?`
  returns. Same HTTP call (`POST /v{N}/conversations/list`),
  same data.
- `Client::fetch_room_access` is removed from the public API
  surface (`grep -n 'pub async fn fetch_' kupferdraht-core/src/`
  returns zero hits).
- `Room` methods compile against an internal `Weak<Client>`
  back-reference (the same pattern matrix-rust-sdk uses) — they
  do not own the HTTP client.

14e:
- `invite_members(...).await?` returns an `Arc<Room>` whose
  member list contains the freshly-invited users.
- `kick_members(...).await?` returns an `Arc<Room>` whose
  member list no longer contains the kicked users.
- `set_room_access(...).await?` returns an `Arc<Room>` with the
  updated access fields.
- `rename_room(...).await?` returns an `Arc<Room>` with the new
  name.
- Existing call sites that discard the result with
  `_ = client.invite_members(...).await?;` continue to compile
  and behave identically — additive in practice.
- The returned `Arc<Room>` is identity-equal (via `Arc::ptr_eq`)
  to a subsequent `client.room(&room_id)` call — same handle,
  not a fresh allocation.

14f:
- `Timeline::subscribe(listener)` returns a `Subscription` handle
  whose `Drop` removes the listener from the internal registry.
- Dropping the `Subscription` while a sync iteration is mid-
  dispatch does not deadlock and does not skip events already
  in-flight to the listener (registry mutex held only during
  registration/removal, not during dispatch).
- Explicit `subscription.unsubscribe()` is equivalent to `drop`.
- Concurrent subscribe/unsubscribe on the same `Timeline` is
  safe — no leaked listeners, no dangling registrations.
- Two listeners with the same `TimelineListener` Arc do not
  collide — each `subscribe()` call yields a distinct
  `Subscription` and removes only its own slot.

14g:
- `room_or_load(id)` returns `Some(Arc<Room>)` when the room is
  in cache; identical to `room(id)`.
- `room_or_load(id)` on a cache-miss calls
  `load_rooms_from_store` exactly once and retries the lookup;
  if the store has the room, returns `Some`.
- `room_or_load(id)` on a true miss (neither cache nor store)
  returns `None` without raising an error.
- `room(id)` (explicit) continues to return `None` immediately
  on cache miss — no behavioural change.
- `Timeline::send_text_auto_join(body)` probes for MLS state;
  if absent, calls `Client::join_room(room_id)` first, then
  `send_text(body)`.
- `send_text_auto_join` on a room with MLS state already loaded
  behaves identically to `send_text` (no extra join attempt, no
  spurious HTTP).
- `send_text_auto_join` failure during `join_room` surfaces as
  the existing `join_room` error variant; failure during the
  subsequent `send_text` surfaces as the existing `send_text`
  error.

### CLI commands

```
kupferdraht-cli dm-create --peer <uuid@domain>
kupferdraht-cli dm-show   --peer <uuid@domain>   # 14a dm_room
kupferdraht-cli dm-with   --peer <uuid@domain>   # 14a dm_with — create-if-missing
```

CLI keeps the existing **flat** subcommand convention
(`dm-create`, `dm-show`, `dm-with` — same shape as
`room-create`, `room-invite`, `mark-read`, etc.). A future CLI
subgroup pass (audit §7.3) may collapse these into
`kupferdraht-cli dm <create|show|with>`, but that is a global CLI
restructure and explicitly out of scope for 14a — see
CHANGELOG.md §42.

14c is library-only — the CLI already uses the underlying
machinery (`Client::rooms()` directly) and gains no new commands.

14b touches the CLI: `sync-fetch` is removed, `sync` and
`sync-stream` keep their names but switch to the new
`sync_once` / `sync` entry points (semantics expand — see the
14b implementation note above). `--idle-backoff` on `sync-stream`
becomes the input for `SyncSettings::poll_interval`.

14e/14f/14g are library-only — no new CLI subcommands. Existing
mutation subcommands (`room-invite`, `room-kick`, `room-access-set`,
`room-rename`) accept the new return value transparently; CLI
handler code that discards the result needs no change.

### Audit closures

Step 14's sub-step landings drop the following entries from
`docs/v1-api-ergonomics-audit.md` and
`docs/v1-known-simplifications.md` — per the project convention
of dropping resolved items in the same commit as the code change
(prior closures `de471c4`, `1a60704`, the §9.1/§9.2 drops in
`f76c9bc`):

**14a closes** (committed in plan as of 2026-05-20):
- §7.1 — `sync` vs `sync-fetch` vs `sync-stream` CLI naming
  collapses via the 14b CLI rename (`sync-fetch` removed).
  Actually 14b — see below; tracked here as 14a-adjacent.

**14b closes:**
- §7.1 — `sync` / `sync-fetch` / `sync-stream` naming collision
  resolved.

**14e closes:**
- §1.5 — Mutation methods don't return the updated `Arc<Room>`.
  `invite_members`, `kick_members`, `set_room_access`,
  `rename_room` all retrofitted.

**14f closes:**
- `v1-known-simplifications.md §2.7` — `Timeline::subscribe` has
  no `unsubscribe`. Subscription-handle pattern lands; the
  listener registry becomes addressable.

**14g closes:**
- §1.1 — `Client::room(id)` returns `None` when cache is empty.
  `room_or_load(id)` ships as the lazy-load variant; explicit
  `room(id)` stays.
- §1.2 — `Timeline::send_text` / `send_file` fails when MLS
  state is absent. `send_text_auto_join` ships as the
  auto-join variant; explicit `send_text` stays for callers
  that want to decline invites.

Each landing commit references the audit-doc edits in its
commit message and removes the closed entries from the audit
sibling docs in the same commit.

### Open questions

1. **~~`dm_room`: rely on local cache only, or fall back to
   `GET /one2one-conversations`?~~** **Resolved 2026-05-21 via
   live-validation.** The Wire GET endpoint IS server-side
   "fetch-or-establish" — first call registers the 1:1
   conversation, every subsequent call returns the existing
   one. `dm_room` stays strict local-cache-only (matches
   matrix-rust-sdk's `get_dm_room` semantics). The "cache miss
   with possibly-existing-server-side-DM" case is handled by
   `dm_with`, which falls through to `create_dm`, which is the
   single GET that creates if absent or returns if present.
   Full write-up in `docs/wire/api-notes.md` §"Step 14a
   discovery"; per-process cold-cache concern (cache empty on
   fresh CLI invocation) tracked separately as audit §1.1 /
   sub-step 14g.
2. **Typed event handler surface (14c):** how to expose typed
   variants without breaking the existing `EventHandler` trait.
   Possible shapes: (a) a parallel `add_event_handler<T>` API
   with a derive macro registering content types; (b) a runtime
   match wrapper that filters by `discriminant` and downcasts;
   (c) defer indefinitely if the trait is good enough. To be
   decided as a TDD precursor for 14c.
3. **`sync_stream` returns `impl Stream<Item = Result<SyncSummary>>`
   today; UniFFI cannot expose `impl Stream` directly.** UDL
   surface decision deferred to a workspace-wide UDL pass per
   ADR-0009 — the 10e precedent applies here too. Note that
   `sync_with_callback` is the matrix-rust-sdk-shaped UDL-friendly
   counterpart and ships as part of 14b precisely so bindings
   have an exportable entry point that does not need `Stream`.

### Smoke test

```bash
# 14a
kupferdraht-cli dm-create --peer "$PEER_UUID@wire.com"
kupferdraht-cli rooms                # newly created 1:1 appears
kupferdraht-cli send  --id "$DM_ID" -m "hello"
kupferdraht-cli dm-show --peer "$PEER_UUID@wire.com"
# (should return the same room id)
kupferdraht-cli dm-with --peer "$OTHER_PEER@wire.com"
# first run: creates + returns. Re-run: returns the same room
# without a second POST (verify in network log).

# 14b live-smoke via CLI:
kupferdraht-cli sync                 # one iteration, returns counts
kupferdraht-cli sync-stream --idle-backoff 2s  # loop, Ctrl-C to exit
# verify sync-fetch is no longer in `--help`

# 14c live-smoke is integration test only — no CLI surface.

# 14e — verify mutation return values land via the existing CLI
# subcommands. No new CLI flags; the integration tests assert
# the Arc<Room> contract directly.

# 14f — integration tests only (Timeline::subscribe / Subscription
# lifecycle is a library concern). No CLI surface.

# 14g — verify auto-hydration via the CLI's existing flow:
kupferdraht-cli login --backend production --email "$EMAIL"
kupferdraht-cli send --id "$ROOM_ID" -m "hello"  # default still fails
                                            # cleanly if MLS state
                                            # absent.
# (no new CLI subcommand for send_text_auto_join in V1; the
# library variant ships, CLI exposure deferred to a later
# convenience pass.)
```
