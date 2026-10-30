// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The scope of one drained page (Step 109, ADR-0037).
//!
//! The drain runs a page inside [`page_scope`]. While a page transaction
//! is open, a store write from inside the scope joins it; a write from
//! any other task waits until the page is committed or rolled back. The
//! marker is task-local, so it follows the drain's future on every
//! target and never leaks into a task the drain spawns.

use std::future::Future;

tokio::task_local! {
    static IN_PAGE: ();
}

/// Run `fut` as the drain's page: its store writes join the open page
/// transaction instead of waiting for it.
pub(crate) async fn page_scope<F: Future>(fut: F) -> F::Output {
    IN_PAGE.scope((), fut).await
}

/// Whether the current task runs inside [`page_scope`].
pub(crate) fn in_page_scope() -> bool {
    IN_PAGE.try_with(|()| ()).is_ok()
}
