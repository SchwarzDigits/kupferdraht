// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 117 — `init_logging` in a process of its own: it installs Kupferdraht's
//! subscriber, and a second call replaces level and listener. TDD red.
//! (The forwarding rules are unit-tested in `logging`; another subscriber
//! installed first is `step_117_foreign_subscriber`.)

use std::sync::{Arc, Mutex};

use kupferdraht::logging::{LogLevel, LogListener, LogRecord, init_logging};

#[derive(Default)]
struct Collect(Mutex<Vec<LogRecord>>);

impl LogListener for Collect {
    fn on_log(&self, record: LogRecord) {
        self.0.lock().expect("records").push(record);
    }
}

impl Collect {
    fn messages(&self) -> Vec<String> {
        self.0
            .lock()
            .expect("records")
            .iter()
            .map(|r| r.message.clone())
            .collect()
    }
}

/// One test: the subscriber is the process's, so the steps run in order.
#[test]
fn init_logging_installs_the_subscriber_and_a_second_call_replaces_the_listener() {
    let first = Arc::new(Collect::default());
    init_logging(LogLevel::Warn, first.clone()).expect("installed");
    tracing::warn!(target: "kupferdraht::itest", "to the first");
    tracing::info!(target: "kupferdraht::itest", "below warn");

    let second = Arc::new(Collect::default());
    init_logging(LogLevel::Info, second.clone()).expect("replaced");
    tracing::info!(target: "kupferdraht::itest", "to the second");

    assert_eq!(first.messages(), vec!["to the first".to_owned()]);
    assert_eq!(second.messages(), vec!["to the second".to_owned()]);
}
