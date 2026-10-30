// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 117 — an app that installed its own `tracing` subscriber keeps it:
//! `init_logging` refuses with `InvalidInput`. A process of its own. TDD
//! red.

use std::sync::Arc;

use kupferdraht::error::KupferdrahtError;
use kupferdraht::logging::{LogLevel, LogListener, LogRecord, init_logging};

struct Silent;

impl LogListener for Silent {
    fn on_log(&self, _record: LogRecord) {}
}

#[test]
fn another_subscriber_installed_first_is_invalid_input() {
    tracing::subscriber::set_global_default(tracing::subscriber::NoSubscriber::default())
        .expect("the app's own subscriber");

    let result = init_logging(LogLevel::Warn, Arc::new(Silent));

    assert!(
        matches!(result, Err(KupferdrahtError::InvalidInput(_))),
        "{result:?}"
    );
}
