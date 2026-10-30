// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 14f — `Subscription` handle for `Timeline::subscribe` /
//! `Client::add_timeline_listener`.
//!
//! Replaces the previous "register-for-Client-lifetime, no
//! unsubscribe" semantic. Holding the `Subscription` keeps the
//! listener registered; dropping the handle (or calling
//! [`Subscription::unsubscribe`] explicitly) removes it from the
//! `Client`'s listener registry. matrix-rust-sdk analog:
//! `EventHandlerHandle` returned by `Client::add_event_handler`.
//!
//! Closes `v1-known-simplifications.md §2.7`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, Weak};

use crate::timeline::{Timeline, TimelineDiff, TimelineListener};

/// Crate-internal type alias for the listener registry. Keyed by
/// monotonic `u64` (issued by `Client`'s
/// `timeline_listener_next_id` counter) so individual entries can be
/// removed in O(1) without scanning. Replaces the previous
/// `Vec<Arc<dyn TimelineListener>>` storage that had no addressable
/// shape.
pub(crate) type ListenerRegistry = StdMutex<HashMap<u64, Arc<dyn TimelineListener>>>;

/// A registry a [`Subscription`] can remove its listener from — the
/// timeline registry above and the Step-95d device-change registry
/// share one handle type.
pub(crate) trait Unregister: Send + Sync {
    fn unregister(&self, id: u64);
}

impl<T: ?Sized + Send + Sync> Unregister for StdMutex<HashMap<u64, Arc<T>>> {
    fn unregister(&self, id: u64) {
        if let Ok(mut guard) = self.lock() {
            guard.remove(&id);
        }
    }
}

/// Opaque handle returned by `Timeline::subscribe` (and by the
/// lower-level `Client::add_timeline_listener`). The listener is
/// unregistered when this is dropped, or when
/// [`Self::unsubscribe`] is called explicitly. Marked `#[must_use]`
/// because the most common mistake is to discard the return value
/// — that would immediately drop the handle and silently unregister
/// the listener.
///
/// Holds a `Weak` reference to the registry: if the `Client` itself
/// is dropped first, the registry goes with it and `Drop` becomes a
/// no-op (nothing to remove).
#[must_use = "drop the Subscription to unsubscribe; call .unsubscribe() to make it explicit"]
pub struct Subscription {
    pub(crate) id: u64,
    pub(crate) registry: Weak<dyn Unregister>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        // Upgrade the `Weak` to the registry. `None` means the
        // owning `Client` (and its listener registry) was already
        // dropped — nothing to remove, no UB. On success, take the
        // mutex briefly and `remove(&self.id)`.
        if let Some(registry) = self.registry.upgrade() {
            registry.unregister(self.id);
        }
    }
}

impl Subscription {
    /// Explicit form of "drop this Subscription to unregister the
    /// listener". Consumes `self` so the action is visible at the
    /// call site, then `Drop` fires immediately on the dropped
    /// binding. Equivalent to `drop(subscription)`; mostly
    /// stylistic.
    pub fn unsubscribe(self) {
        // Drop fires here.
    }
}

impl Timeline {
    /// Register a [`TimelineListener`] that fires for every
    /// `TimelineDiff` emitted on **this room** (other rooms'
    /// diffs are filtered out by the wrapper installed here).
    /// Step 6-II-a wires only `TimelineDiff::Append`; future
    /// sub-steps will emit `UpdateAt` / `RemoveAt` for edit /
    /// delete / reaction toggles.
    ///
    /// Step 14f — returns a [`crate::timeline::Subscription`]
    /// handle. Drop the handle (or call `.unsubscribe()` explicitly)
    /// to unregister the listener; hold the handle to keep it
    /// alive. matrix-rust-sdk analog:
    /// `Client::add_event_handler -> EventHandlerHandle`.
    pub fn subscribe(&self, listener: Arc<dyn TimelineListener>) -> crate::timeline::Subscription {
        let filter = Arc::new(RoomScopedListener {
            room_id: self.room_id.clone(),
            inner: listener,
        });
        self.client.add_timeline_listener(filter)
    }
}

/// Filters `TimelineDiff`s for a single room before forwarding to
/// the inner listener. Used by [`Timeline::subscribe`] so per-room
/// subscriptions are easy to express even though the dispatch
/// registry is global.
struct RoomScopedListener {
    room_id: String,
    inner: Arc<dyn TimelineListener>,
}

impl TimelineListener for RoomScopedListener {
    fn on_update(&self, diff: TimelineDiff) {
        let matches = match &diff {
            TimelineDiff::Append { room_id, .. }
            | TimelineDiff::UpdateAt { room_id, .. }
            | TimelineDiff::RemoveAt { room_id, .. }
            | TimelineDiff::Reset { room_id, .. } => room_id == &self.room_id,
        };
        if matches {
            self.inner.on_update(diff);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;
    use crate::timeline::{TimelineDiff, TimelineListener};

    struct CountingListener {
        count: Arc<AtomicU32>,
    }
    impl TimelineListener for CountingListener {
        fn on_update(&self, _diff: TimelineDiff) {
            self.count.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn fake_diff() -> TimelineDiff {
        TimelineDiff::Reset {
            room_id: "room@test".to_string(),
            items: Vec::new(),
        }
    }

    fn dispatch_all(registry: &ListenerRegistry) {
        let guard = registry.lock().expect("registry mutex");
        for l in guard.values() {
            l.on_update(fake_diff());
        }
    }

    /// Helper: register `listener` under `id` and return the
    /// `Subscription` handle. Mirrors what
    /// `Client::add_timeline_listener` does at green-phase, in
    /// isolated form so the unit tests don't need a full Client.
    fn register(
        registry: &Arc<ListenerRegistry>,
        id: u64,
        listener: Arc<dyn TimelineListener>,
    ) -> Subscription {
        registry
            .lock()
            .expect("registry mutex")
            .insert(id, listener);
        let weak: Weak<ListenerRegistry> = Arc::downgrade(registry);
        Subscription { id, registry: weak }
    }

    #[test]
    fn subscription_drop_removes_listener_from_registry() {
        let registry: Arc<ListenerRegistry> = Arc::new(StdMutex::new(HashMap::new()));
        let count = Arc::new(AtomicU32::new(0));
        let listener: Arc<dyn TimelineListener> = Arc::new(CountingListener {
            count: Arc::clone(&count),
        });
        let sub = register(&registry, 1, listener);

        // Pre-drop: listener fires.
        dispatch_all(&registry);
        assert_eq!(
            count.load(Ordering::Relaxed),
            1,
            "listener must fire pre-drop"
        );

        drop(sub);

        // Post-drop: registry is empty, dispatch is a no-op.
        assert!(
            registry.lock().expect("registry").is_empty(),
            "Subscription::drop must remove the listener from the registry",
        );
        dispatch_all(&registry);
        assert_eq!(
            count.load(Ordering::Relaxed),
            1,
            "no further dispatch after Subscription is dropped",
        );
    }

    #[test]
    fn subscription_unsubscribe_method_is_equivalent_to_drop() {
        let registry: Arc<ListenerRegistry> = Arc::new(StdMutex::new(HashMap::new()));
        let count = Arc::new(AtomicU32::new(0));
        let listener: Arc<dyn TimelineListener> = Arc::new(CountingListener {
            count: Arc::clone(&count),
        });
        let sub = register(&registry, 42, listener);

        sub.unsubscribe();

        assert!(
            registry.lock().expect("registry").is_empty(),
            "Subscription::unsubscribe must remove the listener (equivalent to drop)",
        );
    }

    #[test]
    fn dropping_one_subscription_does_not_affect_another() {
        // Two distinct subscriptions of (separate) listeners under
        // separate ids — dropping one removes only its own entry.
        // Closes the plan-doc's "two listeners with the same Arc
        // do not collide" contract via the addressable-id model.
        let registry: Arc<ListenerRegistry> = Arc::new(StdMutex::new(HashMap::new()));
        let count_a = Arc::new(AtomicU32::new(0));
        let count_b = Arc::new(AtomicU32::new(0));
        let listener_a: Arc<dyn TimelineListener> = Arc::new(CountingListener {
            count: Arc::clone(&count_a),
        });
        let listener_b: Arc<dyn TimelineListener> = Arc::new(CountingListener {
            count: Arc::clone(&count_b),
        });
        let sub_a = register(&registry, 1, listener_a);
        let _sub_b = register(&registry, 2, listener_b);

        drop(sub_a);

        let guard = registry.lock().expect("registry");
        assert_eq!(guard.len(), 1, "only sub_a's entry must be removed");
        assert!(!guard.contains_key(&1), "sub_a's entry (id=1) must be gone");
        assert!(
            guard.contains_key(&2),
            "sub_b's entry (id=2) must remain registered",
        );
    }

    #[test]
    fn subscription_drop_after_registry_drop_is_safe() {
        // The Weak<registry> back-reference means the registry can
        // be dropped before the Subscription (e.g. Client dropped
        // first) without UB: the upgrade fails, Drop becomes a
        // no-op.
        let sub = {
            let registry: Arc<ListenerRegistry> = Arc::new(StdMutex::new(HashMap::new()));
            let count = Arc::new(AtomicU32::new(0));
            let listener: Arc<dyn TimelineListener> = Arc::new(CountingListener { count });
            register(&registry, 7, listener)
            // `registry` Arc drops here — the listener's strong ref
            // inside the registry also drops with it.
        };
        // Dropping the Subscription with a stale Weak must not panic.
        drop(sub);
    }
}
