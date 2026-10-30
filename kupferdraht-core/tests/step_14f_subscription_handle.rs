// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 14f — `Timeline::subscribe` returns `Subscription` (integration-level pin).
//!
//! The behavioural Drop / unsubscribe / id-isolation contract lives
//! in `kupferdraht-core/src/timeline/subscription.rs`'s unit tests —
//! those use a stand-alone `ListenerRegistry` so they exercise the
//! Subscription lifecycle without needing a full Client (the
//! registry-mutation path is the same `Arc<ListenerRegistry>`
//! Client wraps). The integration-level pin here only enforces:
//!
//! * `Timeline::subscribe(listener) -> Subscription` (public
//!   signature).
//! * `Client::add_timeline_listener(listener) -> Subscription`
//!   (lower-level public signature).
//! * `Subscription` is re-exported from `kupferdraht`.
//! * `Subscription::unsubscribe(self)` exists and consumes the
//!   handle.
//!
//! At red-phase: the compile-pin compiles (Subscription stub + new
//! signatures exist); the unit tests in `subscription.rs` fail
//! because `Drop` is a no-op. Green-phase fills in `Drop` and
//! `unsubscribe`.

use std::sync::Arc;

use kupferdraht::Subscription;
use kupferdraht::timeline::TimelineListener;

/// Type-level pin for the new public surface. Never invoked at
/// runtime; its presence forces the green-phase signature on
/// `Timeline::subscribe` and `Client::add_timeline_listener`, plus
/// the existence of `Subscription::unsubscribe`.
#[allow(dead_code)]
async fn _signature_pin(
    client: Arc<kupferdraht::Client>,
    timeline: Arc<kupferdraht::Timeline>,
    listener: Arc<dyn TimelineListener>,
) {
    let sub: Subscription = timeline.subscribe(Arc::clone(&listener));
    sub.unsubscribe();

    let sub2: Subscription = client.add_timeline_listener(listener);
    drop(sub2);
}
