// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 77 — `FileKeyStore` announces itself with one plaintext warning.
//!
//! Its own test binary, so this is the only test in its process: `tracing`
//! caches a callsite's interest for every thread, and a test thread that
//! met the warning's callsite with no subscriber made the capture below
//! miss it about once in thirty runs (seen 2026-10-02) while it shared the
//! unit-test binary.

use std::sync::{Arc, Mutex};

use kupferdraht::keystore::FileKeyStore;
use tempfile::TempDir;

/// Minimal capturing `tracing::Subscriber` — records every event's level
/// and rendered fields so a test can pin the activation warn without
/// pulling `tracing-subscriber` into the library's dev-dependencies.
#[derive(Default)]
struct CaptureSubscriber {
    events: Arc<Mutex<Vec<(tracing::Level, String)>>>,
}

impl tracing::Subscriber for CaptureSubscriber {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut rendered = String::new();
        event.record(&mut FieldRenderer(&mut rendered));
        self.events
            .lock()
            .expect("capture lock")
            .push((*event.metadata().level(), rendered));
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

struct FieldRenderer<'a>(&'a mut String);

impl tracing::field::Visit for FieldRenderer<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write as _;
        let _ = write!(self.0, "{}={value:?} ", field.name());
    }
}

#[test]
fn construction_emits_one_plaintext_warning_naming_the_base_dir() {
    let dir = TempDir::new().expect("tempdir");
    let subscriber = CaptureSubscriber::default();
    let events = Arc::clone(&subscriber.events);
    tracing::subscriber::with_default(subscriber, || {
        let _store = FileKeyStore::new(dir.path());
    });
    let events = events.lock().expect("capture lock");
    let warns: Vec<&String> = events
        .iter()
        .filter(|(level, _)| *level == tracing::Level::WARN)
        .map(|(_, rendered)| rendered)
        .collect();
    assert_eq!(warns.len(), 1, "exactly one activation warn: {events:?}");
    let base = dir.path().display().to_string();
    assert!(
        warns[0].contains(&base),
        "warn names the base dir: {}",
        warns[0]
    );
    assert!(
        warns[0].contains("development only") && warns[0].contains("plaintext"),
        "warn says dev-only + plaintext: {}",
        warns[0]
    );
}
