// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Case state — ADR-0018 (Step 72). Per-room case snapshots as
//! `CasePayload` messages in `GenericMessage` field 262001: full
//! snapshots, LWW fold by `revision`, admin-gated core fields, no
//! text fallback. 72a: types + codec; 72b: fold + store + read API;
//! 72c: send + invite re-broadcast + CLI.

mod body;
mod encode;
mod fold;
mod send;
mod types;

pub(crate) use body::{case_body, case_kind_from_body, parse_case_body};
pub(crate) use encode::{payload_from_snapshot, snapshot_from_payload};
pub(crate) use fold::apply_case_event;
pub use types::{CasePatch, CasePriority, CaseSnapshot, CaseState};

#[cfg(test)]
mod tests {
    use super::encode::{payload_from_snapshot, snapshot_from_payload};
    use super::*;
    use prost::Message as _;

    fn sample_snapshot() -> CaseSnapshot {
        let mut extensions = std::collections::HashMap::new();
        extensions.insert("cost_center".to_string(), "4711".to_string());
        extensions.insert("origin".to_string(), "portal".to_string());
        CaseSnapshot {
            revision: 7,
            case_ref: "case-2026-000123".into(),
            external_ref: Some("REF 12/2026-K".into()),
            title: Some("Building permit, 1 Sample Street".into()),
            case_type: Some("building-permit".into()),
            state: CaseState::Waiting,
            state_detail: Some("awaiting the environmental agency's statement".into()),
            assignee: Some("aaaa-bbbb@wire.com".into()),
            due_at_ms: Some(1_760_000_000_000),
            priority: Some(CasePriority::High),
            opened_at_ms: Some(1_755_000_000_000),
            closed_at_ms: None,
            authority_ref: Some("BA-Central".into()),
            extensions,
        }
    }

    /// Step 72a: a full snapshot survives the `GenericMessage` round
    /// trip byte-for-byte, and the `content` oneof stays unset (no
    /// text fallback — ADR-0018).
    #[test]
    fn case_payload_round_trips_through_generic_message() {
        let snapshot = sample_snapshot();
        let generic = crate::wire::proto::GenericMessage {
            message_id: "m-1".into(),
            content: None,
            task: None,
            case: Some(payload_from_snapshot(&snapshot)),
            task_list_marker: None,
        };
        let bytes = generic.encode_to_vec();
        let decoded = crate::wire::proto::GenericMessage::decode(bytes.as_slice()).expect("decode");
        assert!(
            decoded.content.is_none(),
            "no content-oneof fallback for cases"
        );
        assert!(decoded.task.is_none());
        let payload = decoded.case.expect("case payload present");
        assert_eq!(snapshot_from_payload(&payload), snapshot);
    }

    /// Step 72a: forward compatibility — an unknown `state` wire value
    /// (a newer Kupferdraht's enum) decodes as `Open`, and an unknown
    /// `priority` degrades to `None`, instead of failing the decode.
    /// Built as a RAW `CasePayload` (not via `payload_from_snapshot`)
    /// so the decoder contract is pinned independently of the encoder
    /// (the task path's `item_from_payload` precedent).
    #[test]
    fn unknown_case_state_decodes_to_open() {
        let raw = crate::wire::proto::CasePayload {
            revision: 7,
            case_ref: "case-2026-000123".into(),
            external_ref: Some("REF 12/2026-K".into()),
            title: Some("Building permit".into()),
            case_type: None,
            state: 42,
            state_detail: None,
            assignee: None,
            due_at_ms: None,
            priority: Some(99),
            opened_at_ms: None,
            closed_at_ms: None,
            authority_ref: Some("BA-Central".into()),
            extensions: std::collections::HashMap::new(),
        };
        let bytes = raw.encode_to_vec();
        let reparsed = crate::wire::proto::CasePayload::decode(bytes.as_slice())
            .expect("unknown enum values must not fail the decode");
        let snapshot = snapshot_from_payload(&reparsed);
        assert_eq!(snapshot.state, CaseState::Open);
        assert_eq!(snapshot.priority, None);
        assert_eq!(snapshot.revision, 7, "the rest of the snapshot is intact");
        assert_eq!(snapshot.authority_ref.as_deref(), Some("BA-Central"));
    }
}
