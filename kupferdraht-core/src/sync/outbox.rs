// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! What the embedder hears about a drained page, held back until the page
//! commits (Step 109, ADR-0037).
//!
//! While the drain dispatches a page, the page's data is not committed
//! yet; a failure rolls it back and a later drain processes it again.
//! Announcing it now would tell the embedder of data that may vanish, and
//! the replay would tell it twice. So inside the drain's page every
//! notification to the embedder is held; after the commit the drain
//! emits the held ones, and a rolled-back notification drops its own.
//! Outside a page (sends, other tasks) nothing is held.

use std::cell::RefCell;
use std::future::Future;

use crate::rooms::RoomDevicesChange;
use crate::sync::event_handler::Message;
use crate::timeline::TimelineDiff;

/// One notification to the embedder.
pub(crate) enum Held {
    Message(Message),
    RoomInvited(String),
    Timeline(TimelineDiff),
    Devices(RoomDevicesChange),
}

tokio::task_local! {
    static OUTBOX: RefCell<Vec<Held>>;
}

/// Run `fut` with an outbox: notifications are held, not emitted.
pub(crate) async fn scope<F: Future>(fut: F) -> F::Output {
    OUTBOX.scope(RefCell::new(Vec::new()), fut).await
}

/// Hold `item` when the current task runs inside [`scope`]; otherwise
/// hand it back to be emitted now.
pub(crate) fn hold(item: Held) -> Option<Held> {
    let mut item = Some(item);
    let held = OUTBOX.try_with(|outbox| {
        if let Some(item) = item.take() {
            outbox.borrow_mut().push(item);
        }
    });
    if held.is_ok() { None } else { item }
}

/// How many notifications are held — a notification's savepoint.
pub(crate) fn mark() -> usize {
    OUTBOX.try_with(|outbox| outbox.borrow().len()).unwrap_or(0)
}

/// Drop the notifications held since `mark`: their notification was
/// rolled back.
pub(crate) fn truncate(mark: usize) {
    let _ = OUTBOX.try_with(|outbox| outbox.borrow_mut().truncate(mark));
}

/// Take every held notification, to emit after the commit.
pub(crate) fn take() -> Vec<Held> {
    OUTBOX
        .try_with(|outbox| std::mem::take(&mut *outbox.borrow_mut()))
        .unwrap_or_default()
}
