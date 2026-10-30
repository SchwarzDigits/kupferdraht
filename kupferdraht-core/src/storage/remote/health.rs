// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! An account's storage health (Step 113, ADR-0038): whether its databases
//! on the storage server take operations (`online`) or wait for the server
//! to come back (`reconnecting`), the backoff between reconnect attempts,
//! one heal at a time, and the observers `startSync`'s `onStorage`
//! subscribes. Both databases of an account share one health. Compiled for
//! the browser and for tests; single-threaded like the browser worker.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;

/// Whether an account's storage takes operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StorageState {
    /// The databases take operations.
    Online,
    /// The server is unreachable: operations fail at once with
    /// `ServiceUnavailable` until the VFS has healed the databases.
    Reconnecting,
}

impl StorageState {
    /// The state as the browser API spells it.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Reconnecting => "reconnecting",
        }
    }
}

/// An observer's handle, to unsubscribe it.
pub(crate) type ObserverId = u64;

/// An observer of the state.
type Observer = Rc<dyn Fn(StorageState)>;

/// An account's storage health.
pub(crate) struct StorageHealth {
    /// The databases (by path) that do not take operations now.
    unreachable: RefCell<BTreeSet<String>>,
    next_observer: Cell<ObserverId>,
    observers: RefCell<Vec<(ObserverId, Observer)>>,
}

impl StorageHealth {
    /// A healthy storage.
    pub(crate) fn new() -> Self {
        Self {
            unreachable: RefCell::new(BTreeSet::new()),
            next_observer: Cell::new(0),
            observers: RefCell::new(Vec::new()),
        }
    }

    /// The current state: `reconnecting` while any database is unreachable.
    pub(crate) fn state(&self) -> StorageState {
        if self.unreachable.borrow().is_empty() {
            StorageState::Online
        } else {
            StorageState::Reconnecting
        }
    }

    /// Record whether `database` takes operations; the observers hear the
    /// account's state when it changes. They are called outside the borrow,
    /// so one may subscribe or unsubscribe.
    pub(crate) fn set_reachable(&self, database: &str, reachable: bool) {
        let before = self.state();
        if reachable {
            self.unreachable.borrow_mut().remove(database);
        } else {
            self.unreachable.borrow_mut().insert(database.to_owned());
        }
        let after = self.state();
        debug_assert_eq!(
            after == StorageState::Online,
            self.unreachable.borrow().is_empty(),
            "reconnecting while any database is unreachable"
        );
        if before == after {
            return;
        }
        let observers: Vec<_> = self
            .observers
            .borrow()
            .iter()
            .map(|(_, observer)| Rc::clone(observer))
            .collect();
        for observer in observers {
            observer(after);
        }
    }

    /// Call `observer` on every change of the state.
    pub(crate) fn subscribe(&self, observer: Box<dyn Fn(StorageState)>) -> ObserverId {
        let id = self.next_observer.get();
        self.next_observer.set(id + 1);
        self.observers.borrow_mut().push((id, Rc::from(observer)));
        id
    }

    /// Stop calling the observer `id`.
    pub(crate) fn unsubscribe(&self, id: ObserverId) {
        self.observers
            .borrow_mut()
            .retain(|(observer, _)| *observer != id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KUPFERDRAHT_DB: &str = "kupferdraht/accounts/a/kupferdraht.db";
    const MLS_DB: &str = "kupferdraht/accounts/a/mls.db";

    fn recorded(health: &StorageHealth) -> Rc<RefCell<Vec<StorageState>>> {
        let heard = Rc::new(RefCell::new(Vec::new()));
        let log = heard.clone();
        health.subscribe(Box::new(move |state| log.borrow_mut().push(state)));
        heard
    }

    #[test]
    fn a_new_storage_is_online() {
        assert_eq!(StorageHealth::new().state(), StorageState::Online);
        assert_eq!(StorageState::Online.as_str(), "online");
        assert_eq!(StorageState::Reconnecting.as_str(), "reconnecting");
    }

    #[test]
    fn an_unreachable_database_makes_the_account_reconnecting() {
        let health = StorageHealth::new();
        let heard = recorded(&health);

        health.set_reachable(KUPFERDRAHT_DB, false);

        assert_eq!(health.state(), StorageState::Reconnecting);
        assert_eq!(*heard.borrow(), vec![StorageState::Reconnecting]);
    }

    /// Both databases of the account fail and heal on their own; the
    /// account is online only when both take operations again — one event
    /// each way.
    #[test]
    fn the_account_is_online_only_when_every_database_is() {
        let health = StorageHealth::new();
        let heard = recorded(&health);

        health.set_reachable(KUPFERDRAHT_DB, false);
        health.set_reachable(MLS_DB, false);
        health.set_reachable(KUPFERDRAHT_DB, true);
        assert_eq!(
            health.state(),
            StorageState::Reconnecting,
            "mls.db still down"
        );
        health.set_reachable(MLS_DB, true);

        assert_eq!(health.state(), StorageState::Online);
        assert_eq!(
            *heard.borrow(),
            vec![StorageState::Reconnecting, StorageState::Online]
        );
    }

    /// Every operation reports its database; a report that changes nothing
    /// is silent.
    #[test]
    fn a_report_that_changes_nothing_is_silent() {
        let health = StorageHealth::new();
        let heard = recorded(&health);

        health.set_reachable(KUPFERDRAHT_DB, true);
        health.set_reachable(KUPFERDRAHT_DB, false);
        health.set_reachable(KUPFERDRAHT_DB, false);

        assert_eq!(*heard.borrow(), vec![StorageState::Reconnecting]);
    }

    #[test]
    fn an_unsubscribed_observer_hears_nothing() {
        let health = StorageHealth::new();
        let heard = Rc::new(RefCell::new(Vec::new()));
        let log = heard.clone();
        let id = health.subscribe(Box::new(move |state| log.borrow_mut().push(state)));

        health.unsubscribe(id);
        health.set_reachable(KUPFERDRAHT_DB, false);

        assert!(heard.borrow().is_empty());
    }
}
