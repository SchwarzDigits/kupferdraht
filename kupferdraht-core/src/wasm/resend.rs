// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 130b — `resendFailed`'s summary as the browser sees it: the rows
//! re-driven, the snapshot rows skipped, where the pass stopped — `null`
//! when every attempted row went through, else the row with its error's
//! JS `name` and `message`, as a thrown error carries them — and the
//! failed rows behind the stop.

use serde::Serialize;
use wasm_bindgen::JsValue;

use crate::timeline::{MessageHandle, ResendSummary};

/// The summary `Client.resendFailed` hands back: `{ resent, skipped,
/// stoppedAt, remaining }`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResendSummaryView {
    resent: Vec<HandleView>,
    skipped: Vec<String>,
    stopped_at: Option<ResendStopView>,
    remaining: u32,
}

/// A re-driven row: `{ messageId, sentAtMs }`, the handle `resend` returns.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HandleView {
    message_id: String,
    sent_at_ms: i64,
}

/// Where the pass stopped: `{ messageId, name, message }`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResendStopView {
    message_id: String,
    name: String,
    message: String,
}

/// Project a pass's summary; `stop_name` is the JS name of the error the
/// pass stopped at — given exactly when the summary has a stop.
#[must_use]
pub fn summary_view(summary: &ResendSummary, stop_name: Option<&str>) -> ResendSummaryView {
    debug_assert_eq!(
        summary.stopped_at.is_some(),
        stop_name.is_some(),
        "summary_view: a stop and its error name come together"
    );
    ResendSummaryView {
        resent: summary.resent.iter().map(handle_view).collect(),
        skipped: summary.skipped.clone(),
        stopped_at: summary
            .stopped_at
            .as_ref()
            .map(|(message_id, message)| ResendStopView {
                message_id: message_id.clone(),
                name: stop_name.unwrap_or("Error").to_owned(),
                message: message.clone(),
            }),
        remaining: summary.remaining,
    }
}

fn handle_view(handle: &MessageHandle) -> HandleView {
    HandleView {
        message_id: handle.message_id.clone(),
        sent_at_ms: handle.sent_at_ms,
    }
}

/// Serialize the summary with no stop as `null`.
///
/// # Errors
/// An `Encoding` error when the value cannot be serialized (never for the
/// view above).
pub fn to_js(view: &ResendSummaryView) -> Result<JsValue, JsValue> {
    let serializer = serde_wasm_bindgen::Serializer::new().serialize_missing_as_null(true);
    view.serialize(&serializer)
        .map_err(|e| super::manager::named_error("Encoding", &e.to_string()))
}
