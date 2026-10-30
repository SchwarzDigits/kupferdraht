// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 98c — the wasm timeline surface must carry what the core already has.
//!
//! Three gaps, one subject:
//!
//! - `messages()` took no cursor, so a browser could only ever read the
//!   newest N items and never page backwards, however long the conversation.
//! - `TimelineItemView` dropped `delivery_status` and reduced each reaction
//!   to `{emoji, count}`. A consumer could therefore not tell a sent message
//!   from a failed one, nor which reaction was its own — both of which the
//!   core carries and every other binding surfaces.
//! - `send_knock` / `send_location` were unbound, so two of Wire's nine
//!   content variants could be received but never sent from a browser.
//!
//! Source-level pins — see `wasm_surface/mod.rs`.

mod wasm_surface;

use wasm_surface::{assert_bound, surface};

#[test]
fn client_binds_knock_and_location_sends() {
    let s = surface();
    assert_bound(&s, "Client", "sendKnock");
    assert_bound(&s, "Client", "sendLocation");
}

/// Without a cursor a long conversation is unreadable past the first page.
#[test]
fn messages_takes_a_before_cursor() {
    let src = wasm_surface::manager_source();
    // The whole signature, not one line: rustfmt wraps a long parameter list.
    let start = src
        .find("pub async fn messages(")
        .expect("manager.rs binds messages()");
    let sig = &src[start..];
    let sig = &sig[..sig.find(" {").expect("signature ends")];
    assert!(
        sig.contains("before"),
        "messages() takes a `before` cursor, got: {sig}"
    );
}

/// `delivery_status` distinguishes sent from failed. A UI that cannot show
/// it has to guess, and guessing about delivery is worse than silence.
#[test]
fn timeline_item_carries_delivery_status() {
    let src = wasm_surface::manager_source();
    let view = view_struct(&src, "TimelineItemView");
    assert!(
        view.contains("delivery_status"),
        "TimelineItemView carries delivery_status"
    );
}

/// The core aggregates reactions as `{emoji, senders}`; collapsing that to a
/// count loses whether the local user is among them.
#[test]
fn reaction_view_carries_is_self() {
    let src = wasm_surface::manager_source();
    let view = view_struct(&src, "ReactionView");
    assert!(view.contains("is_self"), "ReactionView carries is_self");
}

/// Sending a location a consumer can never read back is half a feature: the
/// view flattened both variants to a bare kind label, dropping `hot_knock`
/// and the coordinates the core carries.
#[test]
fn knock_and_location_survive_the_projection() {
    let src = wasm_surface::manager_source();
    let view = view_struct(&src, "TimelineItemView");
    assert!(
        view.contains("hot_knock"),
        "TimelineItemView carries hot_knock"
    );
    assert!(
        view.contains("location"),
        "TimelineItemView carries location"
    );
    let loc = view_struct(&src, "LocationView");
    for field in ["latitude", "longitude", "name", "zoom"] {
        assert!(loc.contains(field), "LocationView carries `{field}`");
    }
}

/// Body of `struct <name> { … }` in the binding source.
fn view_struct(src: &str, name: &str) -> String {
    let start = src
        .find(&format!("struct {name} {{"))
        .unwrap_or_else(|| panic!("manager.rs defines {name}"));
    let body = &src[start..];
    let end = body.find("\n}").expect("struct closes");
    body[..end].to_string()
}
