// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Writes of the browser keystore still in flight (Step 104).
//!
//! The `SecureKeyStore` trait is synchronous; the browser keystore
//! persists to `IndexedDB` in the background. Each background write
//! registers here and reports its result; [`flush`] awaits every write
//! still in flight, so a caller can make a fresh key durable before it
//! creates the database the key opens. Thread-local: the browser is
//! single-threaded, and native keystores write synchronously and never
//! register (a native test registers on its own test thread).

use std::cell::RefCell;

use tokio::sync::oneshot;

use crate::error::KupferdrahtError;

thread_local! {
    static PENDING: RefCell<Vec<oneshot::Receiver<Result<(), KupferdrahtError>>>> =
        const { RefCell::new(Vec::new()) };
}

/// Register a background write; the returned sender reports its result.
pub(crate) fn track_write() -> oneshot::Sender<Result<(), KupferdrahtError>> {
    let (done, pending) = oneshot::channel();
    PENDING.with(|writes| writes.borrow_mut().push(pending));
    done
}

/// Await every registered write — including writes registered while it
/// waits — and fail with the first error. A write whose sender is
/// dropped without a result counts as failed.
pub(crate) async fn flush() -> Result<(), KupferdrahtError> {
    let mut first_error = None;
    loop {
        let pending = PENDING.with(|writes| std::mem::take(&mut *writes.borrow_mut()));
        if pending.is_empty() {
            break;
        }
        for write in pending {
            let outcome = write.await.unwrap_or_else(|_| {
                Err(KupferdrahtError::Generic(
                    "a keystore write ended without reporting its result".into(),
                ))
            });
            if let Err(e) = outcome {
                first_error.get_or_insert(e);
            }
        }
    }
    debug_assert!(PENDING.with(|writes| writes.borrow().is_empty()));
    first_error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::{flush, track_write};
    use crate::error::KupferdrahtError;

    #[tokio::test]
    async fn the_flush_waits_for_a_write_still_in_flight() {
        let done = track_write();
        let finished = Arc::new(AtomicBool::new(false));
        let writer = finished.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            writer.store(true, Ordering::SeqCst);
            let _ = done.send(Ok(()));
        });
        flush().await.expect("the write succeeded");
        assert!(
            finished.load(Ordering::SeqCst),
            "the flush returned before the write finished"
        );
    }

    #[tokio::test]
    async fn a_failed_write_fails_the_flush_once() {
        let done = track_write();
        done.send(Err(KupferdrahtError::Generic("quota exceeded".into())))
            .expect("receiver registered");
        let first = flush().await;
        assert!(
            matches!(&first, Err(e) if e.to_string().contains("quota exceeded")),
            "the write's error: {first:?}"
        );
        assert!(flush().await.is_ok(), "a reported failure is drained");
    }

    #[tokio::test]
    async fn a_write_that_never_reports_fails_the_flush() {
        drop(track_write());
        let flushed = flush().await;
        assert!(
            flushed.is_err(),
            "an unreported write is not durable: {flushed:?}"
        );
    }
}
