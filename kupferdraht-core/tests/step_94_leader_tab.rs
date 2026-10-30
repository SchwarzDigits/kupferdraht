// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 94 — leader tab + thin-client tabs (ADR-0026): the reference module
//! `bindings/web/kupferdraht-tabs/` in a real browser (`scripts/wasm-test.sh`,
//! headless Firefox), driven from the **main thread** — the thin client —
//! while candidate workers hold a stub `Kupferdraht` whose `open` performs the
//! same lease steal the Rust open does. The stub keeps the scenarios free
//! of a Wire backend; the real `Kupferdraht.open` behind the module is the
//! consumer smoke's part (`bindings/web/opfs-smoke/tabs.html`).
//!
//! The module files are loaded as wasm-bindgen local snippets (copied next
//! to the test glue, relative imports intact — spike 2026-09-25); the
//! scenarios live in `tests/web/step_94_driver.js` and return plain result
//! objects this file asserts on. Scenarios run one after another on one
//! origin, so each closes its hosts (terminating their workers releases
//! the lease) before the next starts.
#![cfg(target_arch = "wasm32")]

use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

// Every file of the module is referenced here so wasm-bindgen copies it
// beside the glue; the driver imports them relatively from there.
#[wasm_bindgen(module = "/../bindings/web/kupferdraht-tabs/index.js")]
extern "C" {
    #[wasm_bindgen(js_name = createTabHost)]
    fn create_tab_host(options: JsValue) -> JsValue;
}
#[wasm_bindgen(module = "/../bindings/web/kupferdraht-tabs/protocol.js")]
extern "C" {
    #[wasm_bindgen(js_name = responseChannelName)]
    fn response_channel_name(tab_id: &str) -> String;
}
#[wasm_bindgen(module = "/../bindings/web/kupferdraht-tabs/host.js")]
extern "C" {
    #[wasm_bindgen(js_name = createTabHost)]
    fn create_tab_host_direct(options: JsValue) -> JsValue;
}
#[wasm_bindgen(module = "/../bindings/web/kupferdraht-tabs/worker.js")]
extern "C" {
    #[wasm_bindgen(js_name = serveTab)]
    fn serve_tab(options: JsValue) -> JsValue;
}
#[wasm_bindgen(module = "/../bindings/web/kupferdraht-tabs/proxy.js")]
extern "C" {
    type KupferdrahtProxy;
}
#[wasm_bindgen(module = "/../bindings/web/kupferdraht-tabs/leader.js")]
extern "C" {
    #[wasm_bindgen(js_name = createLeaderDispatcher)]
    fn create_leader_dispatcher(kupferdraht: JsValue, options: JsValue) -> JsValue;
}
#[wasm_bindgen(module = "/tests/web/step_94_stub_kupferdraht.js")]
extern "C" {
    #[wasm_bindgen(thread_local_v2, js_name = FIXTURE_ACCOUNT)]
    static FIXTURE_ACCOUNT: JsValue;
}

fn fixture_account() -> String {
    FIXTURE_ACCOUNT
        .with(JsValue::as_string)
        .expect("the stub exports its fixture account id")
}
#[wasm_bindgen(module = "/tests/web/step_94_driver.js")]
extern "C" {
    #[wasm_bindgen(js_name = rpcRoundTrip)]
    fn rpc_round_trip() -> js_sys::Promise;
    #[wasm_bindgen(js_name = eventFanOut)]
    fn event_fan_out() -> js_sys::Promise;
    #[wasm_bindgen(js_name = electionAndFailover)]
    fn election_and_failover() -> js_sys::Promise;
    #[wasm_bindgen(js_name = noLeaderFailsTyped)]
    fn no_leader_fails_typed() -> js_sys::Promise;
    #[wasm_bindgen(js_name = handOffHoldsLease)]
    fn hand_off_holds_lease() -> js_sys::Promise;
    #[wasm_bindgen(js_name = takeOverOverride)]
    fn take_over_override() -> js_sys::Promise;
    #[wasm_bindgen(js_name = aStaleLeaderMessageMovesNoTab)]
    fn a_stale_leader_message_moves_no_tab() -> js_sys::Promise;
    #[wasm_bindgen(js_name = evictionAfterTheNewLeaderIsKnown)]
    fn eviction_after_the_new_leader_is_known() -> js_sys::Promise;
    #[wasm_bindgen(js_name = storageReachesTheLeadersOpen)]
    fn storage_reaches_the_leaders_open() -> js_sys::Promise;
    #[wasm_bindgen(js_name = resumeCarriesTheTakeOver)]
    fn resume_carries_the_take_over() -> js_sys::Promise;
    #[wasm_bindgen(js_name = aDeviceEvictionReachesEveryTab)]
    fn a_device_eviction_reaches_every_tab() -> js_sys::Promise;
    #[wasm_bindgen(js_name = aTabTakeOverIsReportedToTheEvictedTabOnly)]
    fn a_tab_take_over_is_reported_to_the_evicted_tab_only() -> js_sys::Promise;
    #[wasm_bindgen(js_name = recoveryThroughATab)]
    fn recovery_through_a_tab() -> js_sys::Promise;
    #[wasm_bindgen(js_name = recoveryHandleSurvivesTheLeader)]
    fn recovery_handle_survives_the_leader() -> js_sys::Promise;
    #[wasm_bindgen(js_name = storageStateReachesEveryTab)]
    fn storage_state_reaches_every_tab() -> js_sys::Promise;
}

async fn outcome<T: serde::de::DeserializeOwned>(scenario: js_sys::Promise) -> T {
    let value = JsFuture::from(scenario)
        .await
        .unwrap_or_else(|e| panic!("scenario failed: {e:?}"));
    serde_wasm_bindgen::from_value(value).expect("scenario returns the outcome shape")
}

#[wasm_bindgen_test]
fn module_files_are_served_beside_the_glue() {
    assert_eq!(response_channel_name("tab-1"), "kupferdraht:tabs/tab-1");
    assert!(!fixture_account().is_empty());
    let _ = (
        create_tab_host,
        create_tab_host_direct,
        serve_tab,
        create_leader_dispatcher,
    );
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcOutcome {
    accounts_length: usize,
    account_id: String,
    user_id: String,
    display_name: String,
    rooms_length: usize,
    messages_length: usize,
    last_message_id: String,
    dedup_responses: usize,
    stray_responses_on_b: usize,
    survived_forged_response: bool,
    home_id: String,
    cookie_label: String,
}

#[wasm_bindgen_test]
async fn rpc_round_trip_through_the_leader() {
    let o: RpcOutcome = outcome(rpc_round_trip()).await;
    assert_eq!(
        o.accounts_length, 1,
        "the thin tab reads the leader's registry"
    );
    assert_eq!(o.account_id, fixture_account());
    assert_eq!(
        o.user_id,
        fixture_account(),
        "Client.userId is snapshotted on the proxy"
    );
    assert_eq!(o.display_name, "Stub");
    assert_eq!(o.rooms_length, 2, "rooms() through the proxy");
    assert_eq!(o.messages_length, 2, "messages(room, 2) honours the limit");
    assert_eq!(o.last_message_id, "m-3", "oldest-first, newest two");
    assert_eq!(o.dedup_responses, 1, "a duplicate request is answered once");
    assert_eq!(
        o.stray_responses_on_b, 0,
        "responses go to the requesting tab only"
    );
    assert!(
        o.survived_forged_response,
        "a forged response with an unknown id is ignored"
    );
    assert_eq!(
        o.home_id, "stub-home-0123456789abcdef",
        "the leader's Kupferdraht getters ride on its announcement (Step 93 mirror)"
    );
    assert_eq!(o.cookie_label, "kupferdraht-stub-home-0123456789abcdef");
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct FanOutOutcome {
    message_id: String,
    rooms_updated: u32,
    host_event_message_id: String,
    other_tab_message_id: String,
}

#[wasm_bindgen_test]
async fn events_fan_out_to_every_tab() {
    let o: FanOutOutcome = outcome(event_fan_out()).await;
    assert_eq!(o.message_id, "live-1", "the subscriber's onMessage fires");
    assert_eq!(
        o.rooms_updated, 1,
        "the per-pass sync summary reaches the subscriber"
    );
    assert_eq!(
        o.host_event_message_id, "live-1",
        "the host dispatches the message event"
    );
    assert_eq!(
        o.other_tab_message_id, "live-1",
        "the leader's own tab sees the broadcast too"
    );
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct FailoverOutcome {
    new_leader_is_b: bool,
    b_state: String,
    send_outcome: String,
    rooms_length: usize,
    epoch_increased: bool,
}

#[wasm_bindgen_test]
async fn terminating_the_leader_promotes_the_next_tab() {
    let o: FailoverOutcome = outcome(election_and_failover()).await;
    assert!(
        o.new_leader_is_b,
        "the queued candidate is granted the lease"
    );
    assert_eq!(o.b_state, "leader");
    assert_eq!(
        o.send_outcome, "LeaderChanged",
        "an in-flight mutation fails typed"
    );
    assert_eq!(
        o.rooms_length, 2,
        "an in-flight read re-issues on the new leader"
    );
    assert!(o.epoch_increased, "the new leader's epoch is higher");
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct NoLeaderOutcome {
    name: String,
    is_no_leader: bool,
}

#[wasm_bindgen_test]
async fn a_call_without_any_leader_fails_typed_after_the_wait() {
    let o: NoLeaderOutcome = outcome(no_leader_fails_typed()).await;
    assert_eq!(o.name, "NoLeader");
    assert!(o.is_no_leader);
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LeaseOutcome {
    held_before: bool,
    held_while_leading: bool,
    state: String,
    held_after_close: bool,
}

#[wasm_bindgen_test]
async fn the_hand_off_leaves_the_lease_with_the_leader() {
    let o: LeaseOutcome = outcome(hand_off_holds_lease()).await;
    assert!(!o.held_before, "no leader before the first host");
    assert!(
        o.held_while_leading,
        "the leader's worker holds kupferdraht:instance after the self-steal"
    );
    assert_eq!(o.state, "leader");
    assert!(!o.held_after_close, "closing the host releases the lease");
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TakeOverOutcome {
    b_leads: bool,
    a_state: String,
    a_follows_b: bool,
    accounts_length: usize,
}

#[wasm_bindgen_test]
async fn take_over_evicts_the_leader_and_the_evicted_tab_re_queues() {
    let o: TakeOverOutcome = outcome(take_over_override()).await;
    assert!(o.b_leads, "takeOver() makes the thin tab the leader");
    assert_eq!(
        o.a_state, "thin",
        "the evicted tab re-queued as a thin client"
    );
    assert!(o.a_follows_b);
    assert_eq!(o.accounts_length, 1, "and reads through the new leader");
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LateEvictionOutcome {
    state: String,
    leader_tab_id: String,
    candidates: usize,
}

/// Step 100: the takeover race in its bad order — the new leader's
/// announcement arrives before this tab's worker reports the eviction.
/// The tab must follow the known leader (`thin`), not stay `electing`
/// (no further `leader-changed` would ever correct it). The random order
/// made `take_over_evicts_the_leader…` flaky.
#[wasm_bindgen_test]
async fn an_eviction_after_the_new_leader_is_known_leaves_the_tab_thin() {
    let o: LateEvictionOutcome = outcome(eviction_after_the_new_leader_is_known()).await;
    assert_eq!(o.leader_tab_id, "tab-b");
    assert_eq!(
        o.state, "thin",
        "the evicted tab follows the leader it already knows"
    );
    assert_eq!(o.candidates, 2, "a fresh candidate re-queued");
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct StorageOutcome {
    opened: bool,
    take_over: bool,
    storage_mode: Option<String>,
    storage_url: Option<String>,
    token_exchange_url: Option<String>,
}

/// Step 111c: the tabs layer runs the server mode — `serveTab({ storage })`
/// passes the option to the leader's `Kupferdraht.open`, beside the hand-off's
/// `takeOver: true`.
#[wasm_bindgen_test]
async fn serve_tab_passes_the_storage_option_to_the_leaders_open() {
    let o: StorageOutcome = outcome(storage_reaches_the_leaders_open()).await;
    assert!(o.opened, "the leader opened Kupferdraht");
    assert!(o.take_over, "the lease hand-off still opens with takeOver");
    assert_eq!(o.storage_mode.as_deref(), Some("server"));
    assert_eq!(
        o.storage_url.as_deref(),
        Some("wss://storage.example.test/v1/ws")
    );
    assert_eq!(
        o.token_exchange_url.as_deref(),
        Some("https://exchange.example.test")
    );
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResumeOutcome {
    user_id: String,
    take_over: bool,
}

/// Step 111c (ADR-0035): the takeover of another browser's databases is the
/// call's — a thin tab's `resume(id, { takeOver: true })` reaches the
/// leader with the option.
#[wasm_bindgen_test]
async fn resume_passes_its_take_over_to_the_leader() {
    let o: ResumeOutcome = outcome(resume_carries_the_take_over()).await;
    assert_eq!(o.user_id, fixture_account());
    assert!(o.take_over, "the option reaches the leader's resume");
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceEvictionOutcome {
    a_reasons: Vec<String>,
    b_reasons: Vec<String>,
    accounts_length: usize,
}

/// Step 111c: another browser took the databases over (`"device"`): every
/// tab hears it once with the reason — the leader's host from its worker, a
/// thin tab's from the leader's notice — and a leader serves again.
#[wasm_bindgen_test]
async fn a_device_eviction_reaches_every_tab_with_its_reason() {
    let o: DeviceEvictionOutcome = outcome(a_device_eviction_reaches_every_tab()).await;
    assert_eq!(o.a_reasons, vec!["device"], "the leader's tab, once");
    assert_eq!(o.b_reasons, vec!["device"], "a thin tab, once");
    assert_eq!(o.accounts_length, 1, "a leader serves again afterwards");
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TabEvictionOutcome {
    a_reason: Option<String>,
    b_evictions: usize,
}

/// Step 111c: a tab takeover keeps its old meaning — the evicted tab hears
/// `"tab"`, the tab that took over hears nothing.
#[wasm_bindgen_test]
async fn a_tab_take_over_is_reported_as_tab_to_the_evicted_tab_only() {
    let o: TabEvictionOutcome =
        outcome(a_tab_take_over_is_reported_to_the_evicted_tab_only()).await;
    assert_eq!(o.a_reason.as_deref(), Some("tab"));
    assert_eq!(o.b_evictions, 0, "the new leader is not evicted");
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryOutcome {
    state: String,
    exported_at: u64,
    is_bytes: bool,
    data_length: usize,
    error_name: Option<String>,
    attempts_remaining: Option<u32>,
    pin_check: String,
}

/// Step 112b: `client.recovery()` in a thin tab — the calls reach the
/// leader, the bytes come back as bytes, and a recovery error keeps its
/// name and its detail across the channel.
#[wasm_bindgen_test]
async fn recovery_calls_reach_the_leader_with_their_errors() {
    let o: RecoveryOutcome = outcome(recovery_through_a_tab()).await;
    assert_eq!(o.state, "enabled");
    assert_eq!(o.exported_at, 1);
    assert!(o.is_bytes, "enable resolves to a Uint8Array");
    assert_eq!(o.data_length, 3);
    assert_eq!(o.error_name.as_deref(), Some("WrongPin"));
    assert_eq!(
        o.attempts_remaining,
        Some(3),
        "the detail crosses the channel"
    );
    assert_eq!(o.pin_check, "tooShort");
}

#[derive(serde::Deserialize)]
struct RecoveryHandleOutcome {
    state: String,
}

/// Step 112b: a `Recovery` handle reopens through its client on the next
/// leader, as a `Client` handle re-resumes.
#[wasm_bindgen_test]
async fn a_recovery_handle_reopens_on_the_next_leader() {
    let o: RecoveryHandleOutcome = outcome(recovery_handle_survives_the_leader()).await;
    assert_eq!(o.state, "enabled");
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct StorageStateOutcome {
    subscriber_state: String,
    host_a_state: String,
    host_b_state: String,
}

/// Step 113 (ADR-0038): the leader's sync loop reports the storage state;
/// a thin tab's `startSync` hears it through its sixth callback, and every
/// host fires `storage`.
#[wasm_bindgen_test]
async fn the_storage_state_reaches_every_tab() {
    let o: StorageStateOutcome = outcome(storage_state_reaches_every_tab()).await;
    assert_eq!(
        o.subscriber_state, "reconnecting",
        "the thin tab's onStorage"
    );
    assert_eq!(o.host_a_state, "reconnecting", "the leader's tab");
    assert_eq!(o.host_b_state, "reconnecting", "the thin tab's host");
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct StaleLeaderOutcome {
    moved_back: Vec<String>,
    stale_event_delivered: bool,
    b_state: String,
    b_follows: String,
    a_follows: String,
    a_tab_id: String,
    b_tab_id: String,
    new_epoch_higher: bool,
}

/// Step 116: a late announcement or event of an evicted leader (its old
/// epoch) moves no tab back to it — the new leader stays the leader, and
/// every tab keeps following it.
#[wasm_bindgen_test]
async fn a_stale_leader_message_moves_no_tab_back() {
    let o: StaleLeaderOutcome = outcome(a_stale_leader_message_moves_no_tab()).await;
    assert!(o.new_epoch_higher, "the takeover announced a higher epoch");
    assert_ne!(o.a_tab_id, o.b_tab_id);
    assert!(
        o.moved_back.is_empty(),
        "no tab may follow the evicted leader again, not even for a moment: {:?}",
        o.moved_back
    );
    assert!(
        o.stale_event_delivered,
        "the evicted leader's event is still delivered — its data was stored"
    );
    assert_eq!(o.b_state, "leader", "the new leader stays the leader");
    assert_eq!(o.b_follows, o.b_tab_id, "the new leader follows itself");
    assert_eq!(
        o.a_follows, o.b_tab_id,
        "the evicted tab keeps following the new leader"
    );
}
