// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 115 — the doorbell the browser's sync loop waits on.
//!
//! `wasm/live_sync.rs` sleeps a fixed `IDLE_BACKOFF` (2.5 s) between passes, so
//! an inbound message waits up to that long. These tests pin the seam that lets
//! a socket frame cut the wait short — and, just as importantly, that a silent
//! socket leaves today's timed behaviour intact.
//!
//! The socket itself is not faked here: the bell is the contract, the socket is
//! only one thing that rings it. Transport is covered by the step's live smoke.
//!
//! Run: `GECKODRIVER=… CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
//!       cargo test -p kupferdraht-core --target wasm32-unknown-unknown --test step_115_browser_push_doorbell`
#![cfg(target_arch = "wasm32")]

use std::time::Duration;

use kupferdraht::wasm::push_socket::{
    Doorbell, PushFrame, Wake, await_url_with_token, parse_push_frame, wait_for_bell_or,
};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

/// Milliseconds since the epoch, for measuring how long a wait actually took.
fn now_ms() -> f64 {
    js_sys::Date::now()
}

/// The point of the whole step: a frame must not wait out the idle backoff.
#[wasm_bindgen_test]
async fn a_rung_bell_wakes_the_loop_without_waiting_out_the_backoff() {
    let bell = Doorbell::new();
    bell.ring();
    let started = now_ms();
    let wake = wait_for_bell_or(&bell, Duration::from_secs(30)).await;
    let waited = now_ms() - started;
    assert_eq!(wake, Wake::Rang);
    assert!(
        waited < 1000.0,
        "a ring already pending must return at once, waited {waited} ms"
    );
}

/// No socket, no frames: receive keeps working on the timer exactly as it does
/// today. This is what makes the socket a pure improvement rather than a risk.
#[wasm_bindgen_test]
async fn an_unrung_bell_falls_back_to_the_timer() {
    let bell = Doorbell::new();
    let started = now_ms();
    let wake = wait_for_bell_or(&bell, Duration::from_millis(300)).await;
    let waited = now_ms() - started;
    assert_eq!(wake, Wake::TimedOut);
    assert!(
        waited >= 250.0,
        "must actually wait the backoff out, waited {waited} ms"
    );
}

/// A frame that arrives while a drain is running must survive it — otherwise its
/// message is stranded until the timer fires, which is the bug this step exists
/// to remove.
#[wasm_bindgen_test]
async fn the_bell_is_re_armable_not_one_shot() {
    let bell = Doorbell::new();
    bell.ring();
    assert_eq!(
        wait_for_bell_or(&bell, Duration::from_secs(30)).await,
        Wake::Rang
    );
    bell.ring();
    assert_eq!(
        wait_for_bell_or(&bell, Duration::from_secs(30)).await,
        Wake::Rang,
        "a second frame must ring again after the first was consumed"
    );
}

/// Consumed rings must not pile up. One drain covers every frame that rang
/// before it (ADR-0030: the frame's payload is never dispatched, the drain is),
/// so a second wait has nothing left to report.
#[wasm_bindgen_test]
async fn rings_coalesce_and_do_not_accumulate() {
    let bell = Doorbell::new();
    bell.ring();
    bell.ring();
    bell.ring();
    assert_eq!(
        wait_for_bell_or(&bell, Duration::from_millis(250)).await,
        Wake::Rang
    );
    assert_eq!(
        wait_for_bell_or(&bell, Duration::from_millis(250)).await,
        Wake::TimedOut,
        "three frames before one drain are one wake, not three"
    );
}

// ── the URL: the only way a browser can authenticate a socket ──────────────

/// A browser cannot set an upgrade header, so both credentials ride the query
/// string. This is the one shape the native path does not build.
#[wasm_bindgen_test]
fn await_url_carries_client_and_access_token() {
    let url = await_url_with_token("wss://ws.example", "client-7", "tok-abc").expect("valid base");
    assert_eq!(url.path(), "/await", "the legacy route is versionless");
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    assert!(
        pairs.contains(&("client".to_owned(), "client-7".to_owned())),
        "client scopes the queue to this device: {pairs:?}"
    );
    assert!(
        pairs.contains(&("access_token".to_owned(), "tok-abc".to_owned())),
        "the token must be in the query, not a header: {pairs:?}"
    );
}

/// Wire publishes its websocket host as `https://`, so that is what consumers
/// hold and hand to `ServerConfig`. Refusing it left the browser silently on its
/// timer — the bug Step 115's smoke found.
#[wasm_bindgen_test]
fn await_url_upgrades_wires_published_https_base() {
    let url = await_url_with_token("https://ws.example", "c", "t").expect("https is accepted");
    assert_eq!(url.scheme(), "wss", "https must become wss, not be refused");
    let plain = await_url_with_token("http://ws.example", "c", "t").expect("http is accepted");
    assert_eq!(plain.scheme(), "ws", "and http must become ws");
}

/// Anything that is not a web URL is still refused: the token rides in the query,
/// so the scheme has to be one a browser will actually open.
#[wasm_bindgen_test]
fn await_url_rejects_a_scheme_a_browser_cannot_open() {
    assert!(await_url_with_token("ftp://ws.example", "c", "t").is_err());
    assert!(await_url_with_token("kupferdraht://sso", "c", "t").is_err());
}

/// A session between token refreshes must not build a socket that authenticates
/// with nothing — it falls back to the timer instead.
#[wasm_bindgen_test]
fn await_url_refuses_an_empty_token() {
    assert!(
        await_url_with_token("wss://ws.example", "c", "").is_err(),
        "an empty token would connect to nothing useful"
    );
}

/// Wire's configured base may carry a trailing slash; the route must not double it.
#[wasm_bindgen_test]
fn await_url_tolerates_a_trailing_slash_on_the_base() {
    let url = await_url_with_token("wss://ws.example/", "c", "t").expect("valid base");
    assert_eq!(url.path(), "/await");
}

// ── Step 115b: transient frames (typing) ───────────────────────────────────
//
// A durable frame rings and the drain that follows carries its content. A
// transient one cannot work that way: typing is live-only, so a REST drain can
// never deliver it (ADR-0030). It has to travel as data, and it must not cost a
// drain — which is what these tests pin.

/// A typing frame must reach the loop as data, and must **not** ask for a drain:
/// there is nothing in the queue for a drain to find.
#[wasm_bindgen_test]
async fn a_transient_frame_is_delivered_without_asking_for_a_drain() {
    let bell = Doorbell::new();
    bell.ring_transient(transient_typing_frame("room-1"));
    assert_eq!(
        wait_for_bell_or(&bell, Duration::from_secs(30)).await,
        Wake::Transient,
        "a transient must wake the loop but not schedule a drain"
    );
    let held = bell.take_transients();
    assert_eq!(held.len(), 1, "the frame itself must reach the loop");
    assert!(
        held[0].is_transient(),
        "and it must still be marked transient"
    );
}

/// Both kinds in one window: the durable one decides that a drain runs, and the
/// transient one still gets delivered. Losing either would be a bug.
#[wasm_bindgen_test]
async fn a_durable_frame_wins_the_wake_without_dropping_a_transient() {
    let bell = Doorbell::new();
    bell.ring_transient(transient_typing_frame("room-1"));
    bell.ring();
    assert_eq!(
        wait_for_bell_or(&bell, Duration::from_secs(30)).await,
        Wake::Rang,
        "a durable frame outranks a transient: the drain must run"
    );
    assert_eq!(
        bell.take_transients().len(),
        1,
        "the transient must not be swallowed by the drain"
    );
}

/// Transients are live-only, so a flooding socket must not grow memory while a
/// slow drain runs. The native loop caps them at 64 and drops the oldest.
#[wasm_bindgen_test]
async fn transients_are_capped_and_the_oldest_is_dropped() {
    let bell = Doorbell::new();
    for i in 0..80 {
        bell.ring_transient(transient_typing_frame(&format!("room-{i}")));
    }
    let held = bell.take_transients();
    assert_eq!(held.len(), 64, "capped at the native cap, not unbounded");
    assert_eq!(
        held[0].id(),
        "room-16",
        "the oldest are the ones dropped, not the newest"
    );
}

/// Taking them empties the queue: the loop dispatches each frame once.
#[wasm_bindgen_test]
async fn taking_transients_empties_the_queue() {
    let bell = Doorbell::new();
    bell.ring_transient(transient_typing_frame("room-1"));
    assert_eq!(bell.take_transients().len(), 1);
    assert!(
        bell.take_transients().is_empty(),
        "a dispatched frame must not be dispatched again"
    );
    assert_eq!(
        wait_for_bell_or(&bell, Duration::from_millis(250)).await,
        Wake::TimedOut,
        "and an emptied queue must not keep waking the loop"
    );
}

/// The doorbell contract survives a frame we cannot read: an unparseable frame
/// still means "something happened", so it must ask for a drain rather than be
/// dropped. This is why the socket rings before it parses.
#[wasm_bindgen_test]
async fn an_unparseable_frame_still_asks_for_a_drain() {
    let bell = Doorbell::new();
    bell.ring_frame(b"this is not json");
    assert_eq!(
        wait_for_bell_or(&bell, Duration::from_secs(30)).await,
        Wake::Rang,
        "a frame we cannot parse must still drain — never be swallowed"
    );
    assert!(
        bell.take_transients().is_empty(),
        "and it must not be mistaken for a transient"
    );
}

/// A durable frame arriving as bytes takes the drain path, not the data path.
#[wasm_bindgen_test]
async fn a_durable_frame_read_from_bytes_asks_for_a_drain() {
    let bell = Doorbell::new();
    bell.ring_frame(br#"{"id":"n-1","payload":[],"transient":false}"#);
    assert_eq!(
        wait_for_bell_or(&bell, Duration::from_secs(30)).await,
        Wake::Rang
    );
    assert!(bell.take_transients().is_empty());
}

/// A transient frame arriving as bytes takes the data path.
#[wasm_bindgen_test]
async fn a_transient_frame_read_from_bytes_takes_the_data_path() {
    let bell = Doorbell::new();
    bell.ring_frame(br#"{"id":"n-2","payload":[{"type":"conversation.typing"}],"transient":true}"#);
    assert_eq!(
        wait_for_bell_or(&bell, Duration::from_secs(30)).await,
        Wake::Transient
    );
    assert_eq!(bell.take_transients().len(), 1);
}

/// A minimal transient notification, shaped like cannon's.
fn transient_typing_frame(id: &str) -> PushFrame {
    parse_push_frame(
        format!(r#"{{"id":"{id}","payload":[{{"type":"conversation.typing"}}],"transient":true}}"#)
            .as_bytes(),
    )
    .expect("a well-formed transient frame")
}
