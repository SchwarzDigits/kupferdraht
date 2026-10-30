// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 98b — the wasm surface must carry room admin and guest links.
//!
//! `rename_room`, `delete_room`, `create_join_link`, `revoke_join_link` and
//! `join_via_link` all exist on `Client` in the core and are wrapped in
//! `ffi/`, but never reached the browser. Without them a web client cannot
//! rename or delete a conversation, and cannot take part in guest links at
//! all — neither issuing one nor redeeming one.
//!
//! Source-level pins, for the reason `step_94_tab_proxy_parity.rs` gives:
//! the surface is the artefact, and the behaviour underneath is already
//! covered by the native tests of the wrappers these delegate to.

mod wasm_surface;

use wasm_surface::{assert_bound, surface};

#[test]
fn client_binds_room_admin() {
    let s = surface();
    assert_bound(&s, "Client", "renameRoom");
    assert_bound(&s, "Client", "deleteRoom");
}

#[test]
fn client_binds_guest_links() {
    let s = surface();
    assert_bound(&s, "Client", "createJoinLink");
    assert_bound(&s, "Client", "revokeJoinLink");
}

/// Binding `createJoinLink` alone leaves it unusable on most rooms.
///
/// A Kupferdraht-created conversation defaults to `access=["invite"]`, and Wire
/// answers `403 access-denied` to a code request against it — so opening the
/// access modes is the *prerequisite* for a guest link, not an extra. The CLI
/// says so in `room-access-allow-guests`; the browser needs the same reach.
#[test]
fn client_binds_room_access() {
    let s = surface();
    assert_bound(&s, "Client", "roomAccess");
    assert_bound(&s, "Client", "setRoomAccess");
    assert_bound(&s, "Client", "allowGuests");
}

/// Redeeming a link is the half a REST bridge cannot stand in for: joining a
/// conversation is an MLS external commit, so it has to come from Kupferdraht.
#[test]
fn client_binds_join_via_link() {
    let s = surface();
    assert_bound(&s, "Client", "joinViaLink");
}

/// `JoinLink` carries whether the link is password-protected; dropping that
/// would leave a consumer unable to tell the two kinds apart.
#[test]
fn join_link_payload_mirrors_core() {
    let src = wasm_surface::manager_source();
    assert!(
        src.contains("struct JoinLinkView"),
        "manager.rs projects JoinLink into a view struct"
    );
    for field in ["url", "password_protected"] {
        assert!(src.contains(field), "JoinLinkView carries `{field}`");
    }
}
