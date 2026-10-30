// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 117 — the browser's log listener: `Kupferdraht.open({ logLevel, onLog })`
//! hands Kupferdraht's log events to the app's JS function, in Kupferdraht's worker.
//! Kupferdraht writes to no console itself; the app decides.

use send_wrapper::SendWrapper;
use wasm_bindgen::JsValue;

use crate::error::KupferdrahtError;
use crate::logging::{LogLevel, LogListener, LogRecord};

/// The app's `onLog({ level, target, message })`. Kupferdraht runs on one thread
/// in its worker, so the JS function never leaves it; `SendWrapper` lets it
/// sit in the `Send + Sync` subscriber.
pub(super) struct JsLogListener(SendWrapper<js_sys::Function>);

impl JsLogListener {
    pub(super) fn new(on_log: js_sys::Function) -> Self {
        Self(SendWrapper::new(on_log))
    }
}

/// The record as the JS function receives it.
#[derive(serde::Serialize)]
struct LogRecordView<'a> {
    level: &'static str,
    target: &'a str,
    message: &'a str,
}

impl LogListener for JsLogListener {
    fn on_log(&self, record: LogRecord) {
        let view = LogRecordView {
            level: level_name(record.level),
            target: &record.target,
            message: &record.message,
        };
        let Ok(value) = serde_wasm_bindgen::to_value(&view) else {
            return;
        };
        // The app's logger must not break Kupferdraht: what it throws is dropped.
        let _ = self.0.call1(&JsValue::NULL, &value);
    }
}

/// `"error"` … `"trace"`, as `logLevel` and `LogRecord.level` spell them.
fn level_name(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Error => "error",
        LogLevel::Warn => "warn",
        LogLevel::Info => "info",
        LogLevel::Debug => "debug",
        LogLevel::Trace => "trace",
    }
}

/// The `logLevel` option.
///
/// # Errors
/// `InvalidInput` for a name other than `"error"` … `"trace"`.
pub(super) fn parse_level(name: &str) -> Result<LogLevel, KupferdrahtError> {
    match name {
        "error" => Ok(LogLevel::Error),
        "warn" => Ok(LogLevel::Warn),
        "info" => Ok(LogLevel::Info),
        "debug" => Ok(LogLevel::Debug),
        "trace" => Ok(LogLevel::Trace),
        other => Err(KupferdrahtError::InvalidInput(format!(
            "logLevel: {other:?} is none of \"error\", \"warn\", \"info\", \"debug\", \"trace\""
        ))),
    }
}
