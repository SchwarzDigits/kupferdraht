// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Task-list room marker (Step 75, ADR-0020): `TaskListMarker`
//! (`GenericMessage` field 262002) declares a room a dedicated task
//! list. Any member sets or clears it; the fold is last-writer-wins
//! in event order like tasks — no admin gate, no revision. Rows
//! persist as `kind='task_list_marker'` (the audit trail of the
//! deliberately forgeable flag); the fold happens on read.
//!
//! Binding rules:
//! `rules/0020-task-list-marker-and-invite-state-rebroadcast.md`.

use super::{Client, KupferdrahtError, TimelineItemKind};

/// JSON shape packed into `timeline_events.body` for a marker row
/// (the task/`Location` small-payload precedent).
#[derive(serde::Serialize, serde::Deserialize)]
struct MarkerBodyPayload {
    marked: bool,
}

/// Pack one marker state into the `timeline_events.body` JSON.
pub(crate) fn marker_body(marked: bool) -> String {
    serde_json::to_string(&MarkerBodyPayload { marked }).unwrap_or_else(|_| "{}".to_string())
}

/// Parse a persisted marker body. `None` on malformed JSON — the row
/// then degrades to Text in the timeline and contributes nothing to
/// the fold.
pub(crate) fn parse_marker_body(body: &str) -> Option<bool> {
    serde_json::from_str::<MarkerBodyPayload>(body)
        .ok()
        .map(|p| p.marked)
}

/// `timeline_events.kind == "task_list_marker"` rows viewed as
/// timeline items.
pub(crate) fn marker_kind_from_body(body: &str) -> Option<TimelineItemKind> {
    parse_marker_body(body).map(|marked| TimelineItemKind::TaskListMarker { marked })
}

impl Client {
    /// Fold the room's task-list marker on read (Step 75): the latest
    /// non-redacted `kind='task_list_marker'` row wins — event order,
    /// any member; no rows means unmarked. See
    /// [`crate::rooms::Room::is_task_list`] for the caller-facing
    /// contract.
    pub(crate) async fn folded_task_list_marker(
        &self,
        room_id: &str,
    ) -> Result<bool, KupferdrahtError> {
        let store = self.store_handle().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "is_task_list: no Store attached — set ClientBuilder::base_dir(...) first".into(),
            )
        })?;
        let rows = store.load_timeline_events(room_id).await?;
        let mut marked = false;
        for row in &rows {
            if row.kind != "task_list_marker" || row.redacted_at_ms.is_some() {
                continue;
            }
            if let Some(state) = parse_marker_body(&row.body) {
                marked = state;
            }
        }
        Ok(marked)
    }

    /// Send the full marker state (no fallback) and fold it locally.
    /// Any member may call this — no admin gate (task-family model,
    /// ADR-0020). See [`crate::rooms::Room::set_task_list`] for the
    /// caller-facing contract.
    pub(crate) async fn set_task_list(
        &self,
        room_id: &str,
        marked: bool,
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(
            !room_id.is_empty(),
            "set_task_list: room_id must be non-empty"
        );
        self.send_task_list_marker_state(room_id, marked).await
    }

    /// Shared send tail for `set_task_list` and the invite
    /// re-broadcast (Step 75): encode the full marker state (no
    /// fallback, pinned at the proto builder), MLS-encrypt + upload,
    /// persist the `kind='task_list_marker'` row, emit the diff.
    pub(crate) async fn send_task_list_marker_state(
        &self,
        room_id: &str,
        marked: bool,
    ) -> Result<(), KupferdrahtError> {
        let group_id = self.resolve_room_group_id(room_id, "set_task_list").await?;
        let (message_id, plaintext) =
            crate::wire::proto::build_task_list_marker_generic_message(marked)?;
        // Step 97a: the row is written as `Sending` before the upload.
        self.send_content_row(
            room_id,
            &group_id,
            "set_task_list",
            crate::timeline::ContentRow {
                message_id: &message_id,
                plaintext: &plaintext,
                kind: "task_list_marker",
                body: &marker_body(marked),
            },
        )
        .await
        .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The body codec round-trips both states; malformed JSON folds
    /// to nothing instead of panicking (runtime data, never asserts).
    #[test]
    fn marker_body_round_trips_and_tolerates_garbage() {
        for marked in [true, false] {
            assert_eq!(parse_marker_body(&marker_body(marked)), Some(marked));
        }
        assert_eq!(parse_marker_body("not json"), None);
        assert_eq!(parse_marker_body("{}"), None);
    }
}
