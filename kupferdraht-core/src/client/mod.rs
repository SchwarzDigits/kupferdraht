// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `client` — authentication and session lifecycle.
//!
//! `ClientBuilder::login` / `::resume` (in [`builder`]) construct the
//! [`Client`]; the operational surface — sync, rooms, membership,
//! timeline read, profile, typing, token refresh — is decomposed
//! across this module's children (Step 29). The `Client` struct, its
//! constructor, and the cross-module shared accessors live here next
//! to the struct so every child `impl Client` block reaches the
//! private fields.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, RwLock, Weak};

use zeroize::Zeroizing;

use crate::encryption::MlsClient;
use crate::error::KupferdrahtError;
use crate::identity::UserId;
use crate::rooms::Room;
use crate::storage::{KupferdrahtHome, Store};
use crate::wire::HttpClient;

mod account;
mod builder;
mod commit_flow;
mod device;
mod device_watch;
mod drain_lock;
mod epoch_gap;
mod http_calls;
pub(crate) mod input;
mod key_packages;
mod login_pipeline;
mod membership;
mod mentions;
mod message_lookup;
mod mls_gate;
mod pending;
mod pending_login;
mod profile;
mod resume;
mod room_freshness;
mod rooms;
mod self_room;
mod sent_signal;
mod server_configs;
mod session;
mod sync;
mod timeline_read;
mod token_refresh;
mod typing;

pub use account::Account;
pub use builder::{ClientBuilder, ServerConfig};
pub use device::DeviceInfo;
pub use pending::PendingClient;
pub use pending_login::PendingLogin;
pub use server_configs::{FileSharingPolicy, ReceiptMode};

/// Outcome tally from [`Client::join_all_rooms`]. The values are
/// flattened counters (not per-room pairs) for the common case where
/// the caller only wants an "N of M joined" summary; failures keep
/// their `(room_id, error_detail)` pair for diagnostics.
#[derive(Debug, Default, Clone)]
pub struct JoinSummary {
    /// Total number of rooms iterated, including ones that were
    /// already joined locally.
    pub attempted: u32,
    /// Rooms that needed an external commit and successfully joined
    /// during this call — including the self conversation this call
    /// established from epoch 0 (Step 86).
    pub joined: u32,
    /// Rooms whose `group_id` was already known to the local openmls
    /// storage — no commit attempted, no network traffic.
    pub already_joined: u32,
    /// Per-room failures, preserving the room id and the error text
    /// so the CLI can print a diagnostic line per entry.
    pub failed: Vec<(String, String)>,
}

/// Outcome of [`Client::resync_existing_rooms`] (Step 7e). Reported as
/// flattened counters plus a per-room failure list, mirroring
/// [`JoinSummary`].
#[derive(Debug, Default, Clone)]
pub struct ResyncSummary {
    /// Total number of cached rooms inspected — including ones the
    /// current device has no local MLS state for (those are skipped,
    /// not counted under the other fields).
    pub attempted: u32,
    /// Rooms whose persisted leaf already carries the current
    /// device's identity — no work, no network traffic.
    pub already_current: u32,
    /// Rooms where the persisted leaf belonged to a prior `device_id`
    /// of the same `user_id` and were successfully rejoined via a
    /// fresh external commit during this call.
    pub resynced: u32,
    /// Per-room failures, preserving the room id and the error text
    /// so the CLI can print a diagnostic line per entry.
    pub failed: Vec<(String, String)>,
}

/// Authenticated session against a Wire backend.
///
/// Public surface mirrors `matrix_sdk::Client`. See `docs/library.md`
/// and `docs/api-mapping-matrix-rust-sdk.md` for the full rationale
/// behind every getter and any deliberate drift from matrix-rust-sdk.
///
/// The matrix-rust-sdk-shaped native API; the `UniFFI` wrapper that
/// exposes it to Kotlin / Swift lives in `crate::ffi` (Step 20c).
pub struct Client {
    user_id: UserId,
    device_id: String,
    server: ServerConfig,
    api_version: u32,
    /// Access token + `zuid` refresh-cookie + acquisition time, behind
    /// a brief `std::sync::RwLock` so [`Client::refresh_access_token`]
    /// can swap all three atomically. Held only for a snapshot-clone
    /// (read) or a swap (write) — never across an `.await` — so a sync
    /// lock is correct and WASM-clean (like `rooms` below). `Zeroizing`
    /// wipes the old buffers when a refresh replaces them (ADR-0006,
    /// `THREAT_MODEL.md` G4).
    tokens: RwLock<TokenState>,
    /// Serialises concurrent in-session refreshes so N simultaneous
    /// 401s collapse to a single `/access` call. `tokio::sync::Mutex`
    /// because it is held across the `/access` `.await` (like
    /// `sync_lock`).
    refresh_lock: tokio::sync::Mutex<()>,
    account: Arc<Account>,
    backend_removal_key_ecdsa_p256: Vec<u8>,
    mls_signature_public_key: Vec<u8>,
    /// Wire transport kept alive for post-login calls (rooms sync,
    /// message send, …). Built with the same `permit_http` opt-in the
    /// `ClientBuilder` was configured with, so the scheme policy
    /// stays consistent across the lifetime of this `Client`.
    http: HttpClient,
    /// In-memory snapshot of the last `sync_metadata`. Authoritative
    /// while the process lives; cycle 2d also mirrors it into the
    /// `SqliteStore` below when one is attached.
    rooms: RwLock<Vec<Arc<Room>>>,
    /// Optional persistent backing store. When attached via
    /// `ClientBuilder::base_dir`, every `sync_metadata` call replaces
    /// the `rooms` + `room_members` snapshot in `kupferdraht.db`.
    store: Option<Arc<dyn Store>>,
    /// Registered event handlers (Step 5c). The `SyncService`
    /// snapshots this list on every frame so consumers added
    /// mid-run are picked up by the next frame. `std::sync::Mutex`
    /// is sufficient — registration is rare, every dispatch holds
    /// the lock just long enough to clone the `Vec`.
    event_handlers: Arc<StdMutex<Vec<Arc<dyn crate::sync::EventHandler>>>>,
    /// Registered timeline listeners (Step 6-II-a). Distinct from
    /// `event_handlers`: listeners observe `TimelineDiff`s after a
    /// row has been persisted (both inbound and outbound — outbound
    /// fires from `Timeline::send_text`, inbound from
    /// `SyncService::dispatch_text_message`). Snapshot-and-dispatch
    /// shape mirrors `event_handlers`.
    /// Step 14f: keyed registry replacing the previous
    /// `Vec<Arc<dyn TimelineListener>>` so individual entries can be
    /// removed in O(1) when their `Subscription` handle is dropped.
    /// Listeners are issued monotonic ids by
    /// `timeline_listener_next_id`.
    timeline_listeners: Arc<crate::timeline::ListenerRegistry>,
    /// Monotonic counter feeding `Subscription::id` so each
    /// `Client::add_timeline_listener` call yields a distinct slot
    /// even when the same `Arc<dyn TimelineListener>` is subscribed
    /// twice.
    timeline_listener_next_id: std::sync::atomic::AtomicU64,
    /// Step 95d: [`crate::rooms::RoomDevicesListener`]s, ids from the
    /// same `timeline_listener_next_id` counter.
    device_listeners: Arc<crate::client::device_watch::DeviceListenerRegistry>,
    /// Held-open MLS identity so `Client::join_room` (Step 3c) and
    /// later send / receive paths do not need to re-open the
    /// `SqliteStorageProvider` on every call. `None` when `login` ran
    /// without persistence, or when `resume` was called without a
    /// storage path — both are legitimate metadata-only sessions.
    /// `std::sync::Mutex` is sufficient because every openmls API
    /// Kupferdraht drives is synchronous.
    #[allow(clippy::struct_field_names)]
    mls_client: Option<Arc<StdMutex<MlsClient>>>,
    /// Weak self-pointer set via `Arc::new_cyclic` in [`Client::new`].
    /// Consumed by `Room`-producing paths so each cached `Room` can
    /// call back into the owning `Client` for `Room::join` /
    /// `Room::sync_members` without the caller plumbing the handle.
    weak_self: Weak<Self>,
    /// Kupferdraht-home layout the account persists under. `None` when the
    /// builder ran without a `base_dir` (in-memory tests / scratch).
    pub(super) kupferdraht_home: Option<KupferdrahtHome>,
    /// The Wire login cookie label this session was logged in with
    /// (Step 93, ADR-0025): the home's label at login, the persisted
    /// session row's on resume — `None` for a session persisted before
    /// Step 93, which still holds the old shared label on Wire's side.
    cookie_label: Option<String>,
    /// Per-peer mutex map used by [`Client::dm_with`] to de-duplicate
    /// concurrent 1:1 creation against the same peer (finding §42).
    /// The outer `std::sync::Mutex` only guards `HashMap` inserts /
    /// reads; the inner `tokio::sync::Mutex` is held across the
    /// `create_dm` `.await` so a second caller queues up behind the
    /// in-flight POST and observes the freshly-cached `Arc<Room>`
    /// when it re-checks `dm_room`. Entries are never removed —
    /// they're tiny (a parked `Arc<Mutex<()>>`) and keep the next
    /// invocation against the same peer cheap.
    dm_create_lock: StdMutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Step 97d (ADR-0031): the in-process layer of the account's
    /// drain lock (`client/drain_lock.rs`) — held while one page or
    /// push frame is dispatched, in front of `sync.lock`.
    drain_lock: tokio::sync::Mutex<()>,
    /// Step 97d (ADR-0031): one async lock per MLS group, held by an own
    /// commit flow from stage to merge/clear (`client/commit_flow.rs`).
    /// Entries are never removed — one tiny `Arc` per group touched.
    commit_flows: StdMutex<HashMap<Vec<u8>, Arc<tokio::sync::Mutex<()>>>>,
    /// Step 97e (ADR-0032): the MLS write gate (`client/mls_gate.rs`) —
    /// held by the drain per notification window and by every MLS write
    /// outside inbound dispatch.
    mls_write_gate: tokio::sync::Mutex<()>,
    /// Step 14b — at-most-one-iteration guard across the four
    /// `Client::sync*` entry points. `try_lock` returns
    /// [`KupferdrahtError::SyncAlreadyRunning`] when contended so two
    /// concurrent callers never advance the cursor in parallel.
    /// The lock is held for the duration of one iteration; loop
    /// variants release between iterations so dispatch handlers
    /// can call `sync_once` recursively (rare but legal — Step
    /// 5c's `EventHandler` already supports it).
    sync_lock: tokio::sync::Mutex<()>,
    /// Step 118 — whether the cached room list may be out of date
    /// (`client/room_freshness.rs`); a sync pass refreshes it only then.
    room_freshness: room_freshness::RoomFreshness,
    /// Step 122 — a message upload succeeded since the browser's sync loop
    /// last looked (`client/sent_signal.rs`).
    sent: sent_signal::SentSignal,
    /// Step 21 — ephemeral typing-indicator state: the receive-side
    /// per-room set of currently-typing users plus the send-side
    /// in-flight dedupe. Never persisted; see [`TypingTracker`].
    typing: crate::client::typing::TypingTracker,
    /// Step 21 — serializes outbound typing POSTs (`started`/`stopped`),
    /// held across the POST itself so they cannot reorder on the wire.
    /// Distinct from `sync_lock`.
    typing_send_lock: tokio::sync::Mutex<()>,
    /// Step 74 — rooms whose MLS groups have queued external-sender
    /// Remove proposals awaiting the batched end-of-cycle commit.
    /// Populated by the dispatch pipeline on `ProposalQueued`,
    /// drained by `SyncService::flush_removal_commits`. Client-owned
    /// (not service-owned) so the retry-until-committed contract
    /// survives per-call `SyncService` instances (`sync_once`).
    /// In-memory only: a restart loses the set, but the proposals
    /// persist in openmls storage and re-signal on the room's next
    /// inbound event (plan 74, documented residual).
    pending_removal_flushes: StdMutex<std::collections::HashSet<String>>,
    /// Step 97a: message ids of the own sends whose upload is in flight
    /// in this process. A `sending` row not in here was abandoned
    /// (cancelled, or the process restarted) and reads as `Failed`.
    /// `Arc` so a send's drop guard can remove its id without a borrow
    /// of the `Client` (`timeline::send_state`).
    in_flight_sends: crate::timeline::InFlightSends,
    /// Step 110a: whether this client's device replaced the one recorded
    /// for the account (Step 40's device reuse found it gone). Set once,
    /// right after the login that registered the device.
    device_replaced: std::sync::atomic::AtomicBool,
}

/// Mutable token triple swapped atomically on an in-session refresh.
struct TokenState {
    access_token: Zeroizing<String>,
    refresh_token: Zeroizing<String>,
    /// Wall-clock (ms) when the current access token was acquired
    /// (login or the most recent refresh).
    issued_at_ms: i64,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("user_id", &self.user_id)
            .field("device_id", &self.device_id)
            .field("api_version", &self.api_version)
            .finish_non_exhaustive()
    }
}

impl Client {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        user_id: UserId,
        device_id: String,
        server: ServerConfig,
        api_version: u32,
        access_token: String,
        refresh_token: String,
        account: Arc<Account>,
        backend_removal_key_ecdsa_p256: Vec<u8>,
        mls_signature_public_key: Vec<u8>,
        http: HttpClient,
        store: Option<Arc<dyn Store>>,
        mls_client: Option<Arc<StdMutex<MlsClient>>>,
        kupferdraht_home: Option<KupferdrahtHome>,
        cookie_label: Option<String>,
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak| {
            // The transport layer calls back into this `Client` to
            // refresh a 401'd token; a `Weak` handle breaks the
            // `Client` → `HttpClient` → `Client` ownership cycle. Keep
            // the clone typed as `Weak<Self>` (so `weak` stays
            // `&Weak<Client>` for `weak_self` below) and let the
            // unsizing coercion to `Weak<dyn …>` happen at the arg.
            let refresher: Weak<Self> = weak.clone();
            http.set_refresher(refresher);
            Self {
                user_id,
                device_id,
                server,
                api_version,
                tokens: RwLock::new(TokenState {
                    access_token: Zeroizing::new(access_token),
                    refresh_token: Zeroizing::new(refresh_token),
                    issued_at_ms: crate::time::now_ms(),
                }),
                refresh_lock: tokio::sync::Mutex::new(()),
                account,
                backend_removal_key_ecdsa_p256,
                mls_signature_public_key,
                http,
                rooms: RwLock::new(Vec::new()),
                store,
                mls_client,
                event_handlers: Arc::new(StdMutex::new(Vec::new())),
                timeline_listeners: Arc::new(StdMutex::new(std::collections::HashMap::new())),
                timeline_listener_next_id: std::sync::atomic::AtomicU64::new(0),
                device_listeners: Arc::new(StdMutex::new(std::collections::HashMap::new())),
                weak_self: weak.clone(),
                kupferdraht_home,
                cookie_label,
                dm_create_lock: StdMutex::new(HashMap::new()),
                drain_lock: tokio::sync::Mutex::new(()),
                commit_flows: StdMutex::new(HashMap::new()),
                mls_write_gate: tokio::sync::Mutex::new(()),
                sync_lock: tokio::sync::Mutex::new(()),
                room_freshness: room_freshness::RoomFreshness::default(),
                sent: sent_signal::SentSignal::default(),
                typing: crate::client::typing::TypingTracker::default(),
                typing_send_lock: tokio::sync::Mutex::new(()),
                pending_removal_flushes: StdMutex::new(std::collections::HashSet::new()),
                in_flight_sends: crate::timeline::InFlightSends::default(),
                device_replaced: std::sync::atomic::AtomicBool::new(false),
            }
        })
    }

    /// Weak back-reference to this `Client`. Handed to every `Room`
    /// so `Room::join` / `Room::sync_members` can call back into
    /// the owner without the caller plumbing an `Arc<Client>`.
    pub(crate) fn weak(&self) -> Weak<Self> {
        self.weak_self.clone()
    }

    /// The own sends in flight in this process (Step 97a,
    /// `timeline::send_state`).
    pub(crate) fn in_flight_sends(&self) -> &crate::timeline::InFlightSends {
        &self.in_flight_sends
    }

    /// Crate-scoped accessor for the optional `Arc<dyn Store>`.
    /// Used by the timeline send / receive paths to skip persistence
    /// when the `Client` was built without a store.
    pub(crate) fn store_handle(&self) -> Option<&Arc<dyn Store>> {
        self.store.as_ref()
    }

    /// The cookie label this session was logged in with (Step 93) — the
    /// manager records it in the session row at login.
    pub(crate) fn cookie_label(&self) -> Option<&str> {
        self.cookie_label.as_deref()
    }

    /// The per-account asset-cache directory
    /// (`base/accounts/<id>/assets`) — the single source of the asset
    /// path, via [`KupferdrahtHome`]. `None` when the client was built
    /// without `base_dir` (in-memory test path).
    pub(crate) fn assets_dir(&self) -> Option<std::path::PathBuf> {
        self.kupferdraht_home
            .as_ref()
            .map(|home| home.account_assets(self.user_id.as_str()))
    }

    /// Crate-scoped accessor for the held `MlsClient` mutex. Returns
    /// a clear `InvalidInput` if the client was built without a
    /// `base_dir` (and therefore has no MLS state). The label
    /// argument is the calling site for the error message
    /// (`"send_text"`, `"join_room"`, …).
    pub(crate) fn mls_client_required(
        &self,
        operation: &str,
    ) -> Result<&Arc<StdMutex<MlsClient>>, KupferdrahtError> {
        self.mls_client.as_ref().ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "{operation}: Client has no MLS state; \
                 base_dir must be set on the ClientBuilder"
            ))
        })
    }

    /// Look up the MLS `group_id` (raw bytes) for a cached room.
    /// Crate-private convenience used by the internal `SyncService` —
    /// `Room::group_id_bytes` returns the same data via the public API.
    pub(crate) async fn group_id_for_room(&self, room_id: &str) -> Option<Vec<u8>> {
        let room = self.room(room_id).await?;
        room.group_id_bytes()
    }

    /// Crate-internal probe used by `Timeline::send_text_auto_join`
    /// (Step 14g) and any other auto-hydration site that needs to
    /// know whether the local openmls store has a usable group for
    /// a given Wire `group_id`. Returns `false` on any error
    /// (missing MLS state, store error) — the callers treat
    /// "absent" and "error" the same way: trigger `join_room`.
    pub(crate) fn has_mls_group(&self, group_id: &[u8]) -> bool {
        let Some(mls) = self.mls_client.as_ref() else {
            return false;
        };
        crate::encryption::lock_mls(mls)
            .group_exists(group_id)
            .unwrap_or(false)
    }
}
