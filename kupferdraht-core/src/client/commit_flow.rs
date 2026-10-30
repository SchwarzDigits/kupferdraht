// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Own commit flows (Step 97d, ADR-0031): stage → upload → merge/clear
//! of an own commit runs under its group's async lock, so this process's
//! drain can wait for the merge instead of healing around a message of
//! the epoch that commit starts. In process only — the pending commit
//! itself (in `mls.db`) is what another `Client` or process sees.

use std::sync::Arc;

use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::client::Client;

impl Client {
    /// Take the group's commit-flow lock for one stage → upload →
    /// merge/clear. Hold the guard until the pending commit is merged
    /// or cleared, and no longer.
    pub(crate) async fn lock_commit_flow(&self, group_id: &[u8]) -> OwnedMutexGuard<()> {
        debug_assert!(!group_id.is_empty(), "lock_commit_flow: group_id non-empty");
        self.commit_flow_mutex(group_id).lock_owned().await
    }

    /// Wait for this process's in-flight commit flow on the group to end.
    /// `false` right away when none is in flight. No own timeout: the
    /// flow ends with its upload, which the HTTP timeout bounds.
    pub(crate) async fn await_commit_flow(&self, group_id: &[u8]) -> bool {
        let Some(mutex) = self.existing_commit_flow(group_id) else {
            return false;
        };
        if mutex.try_lock().is_ok() {
            return false;
        }
        drop(mutex.lock().await);
        true
    }

    /// Whether a commit flow on the group holds its lock right now.
    pub(crate) fn commit_flow_in_flight(&self, group_id: &[u8]) -> bool {
        self.existing_commit_flow(group_id)
            .is_some_and(|mutex| mutex.try_lock().is_err())
    }

    fn existing_commit_flow(&self, group_id: &[u8]) -> Option<Arc<Mutex<()>>> {
        self.commit_flows
            .lock()
            .expect("commit_flows lock")
            .get(group_id)
            .cloned()
    }

    fn commit_flow_mutex(&self, group_id: &[u8]) -> Arc<Mutex<()>> {
        self.commit_flows
            .lock()
            .expect("commit_flows lock")
            .entry(group_id.to_vec())
            .or_default()
            .clone()
    }
}
