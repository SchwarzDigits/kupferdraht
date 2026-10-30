// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Wire's `GenericMessage` Protobuf, generated from
//! `kupferdraht-core/proto/generic_message.proto` at build time by
//! `prost-build` (see `build.rs`).
//!
//! The schema is a minimal subset of Wire's full message vocabulary —
//! only the variants Kupferdraht emits today (Step 4: `Text`). CLAUDE.md
//! "Wire Message Types" pins the rule that any payload Kupferdraht sends
//! to or receives from Wire MUST be serialized as `GenericMessage`.

#![allow(clippy::doc_markdown, clippy::pedantic)]
// The generated `GenericMessage` / `Text` types and `build_text_generic_message`
// helper are exercised by this module's unit tests and consumed by
// cycle 4c (`MlsClient::encrypt_text_message`). Suppress dead_code
// for non-test builds until 4c lands.
#![cfg_attr(not(test), allow(dead_code))]

use prost::Message as _;
use uuid::Uuid;

use crate::error::KupferdrahtError;

include!(concat!(env!("OUT_DIR"), "/kupferdraht.messages.rs"));

/// One @-mention to embed in a `Text` body. Step 6-II-g.
/// Offsets are UTF-16 code units measured into the body
/// string (Wire's clients all index in UTF-16). The qualified
/// user id is `<uuid>@<domain>`.
#[derive(Debug, Clone)]
pub(crate) struct MentionInput {
    pub start: i32,
    pub length: i32,
    pub qualified_user_id: String,
}

/// Convert the public-API `Mention` shape (UTF-16 offsets +
/// qualified user id) into the protobuf `Mention` message,
/// splitting `qualified_user_id` into `(id, domain)` for the
/// federated `QualifiedUserId` submessage.
fn mention_to_proto(m: &MentionInput) -> Mention {
    let (id, domain) = match m.qualified_user_id.split_once('@') {
        Some((id, domain)) => (id.to_string(), domain.to_string()),
        // Tolerate unqualified ids on the way out — emit them
        // with a synthetic empty domain rather than dropping
        // the whole mention. Federated rooms reject these
        // anyway; same-domain rooms treat the empty domain as
        // local. Caller should pass qualified ids.
        None => (m.qualified_user_id.clone(), String::new()),
    };
    Mention {
        start: m.start,
        length: m.length,
        mention_type: None, // we never emit the legacy `user_id` oneof
        qualified_user_id: Some(QualifiedUserId { id, domain }),
    }
}

/// A reference to the parent message of a Quote / Reply.
/// `body` and `sent_at_ms` are the parent's plaintext body and
/// wall-clock timestamp in milliseconds since Unix epoch — the
/// inputs Wire's `Quote.quoted_message_sha256` digest is computed
/// over (see [`compute_quote_sha256`]).
#[derive(Debug, Clone)]
pub(crate) struct QuoteParent<'a> {
    pub message_id: &'a str,
    pub body: &'a str,
    pub sent_at_ms: i64,
}

/// prost-encode one assembled `GenericMessage`. `what` names the
/// variant for the error text.
fn encode_generic_message(msg: &GenericMessage, what: &str) -> Result<Vec<u8>, KupferdrahtError> {
    let mut bytes = Vec::with_capacity(msg.encoded_len());
    msg.encode(&mut bytes)
        .map_err(|e| KupferdrahtError::Encoding(format!("encode {what} GenericMessage: {e}")))?;
    Ok(bytes)
}

/// Build a `GenericMessage` carrying a single `Text` content variant
/// and emit `(message_id, prost_encoded_bytes)`.
///
/// The `message_id` is a freshly-generated UUID v4 per Wire's
/// convention — surfaced to the caller so `Timeline::send_text`
/// can return it as part of `MessageHandle` and persist it on the
/// `timeline_events` row keyed under the same id.
///
/// `quote_parent` populates `Text.quote` (Step 6-II-b) when `Some`;
/// receivers thread it as a quote / reply. The Quote submessage
/// carries both `quoted_message_id` and `quoted_message_sha256` —
/// the latter is `optional` per Wire's schema but Wire's clients
/// (web / iOS / android) refuse to render the reply as a quote
/// without it, so Kupferdraht always computes and emits it.
///
/// # Errors
/// `KupferdrahtError::Encoding` if `prost::Message::encode_to_vec` fails.
/// In practice `encode_to_vec` cannot fail for our schema (no
/// recursion, no unknown fields), but the `Result` shape future-
/// proofs against schema additions.
pub(crate) fn build_text_generic_message(
    body: &str,
    quote_parent: Option<QuoteParent<'_>>,
    mentions: &[MentionInput],
) -> Result<(String, Vec<u8>), KupferdrahtError> {
    let message_id = Uuid::new_v4().to_string();
    let bytes = encode_text_generic_message(&message_id, body, quote_parent, mentions)?;
    Ok((message_id, bytes))
}

/// [`build_text_generic_message`] under a caller-supplied `message_id`
/// — the resend path re-drives a failed row under its persisted id
/// (Step 79).
pub(crate) fn encode_text_generic_message(
    message_id: &str,
    body: &str,
    quote_parent: Option<QuoteParent<'_>>,
    mentions: &[MentionInput],
) -> Result<Vec<u8>, KupferdrahtError> {
    debug_assert!(
        !message_id.is_empty(),
        "encode_text_generic_message: message_id must be set"
    );
    let quote = quote_parent.as_ref().map(|p| Quote {
        quoted_message_id: p.message_id.to_string(),
        quoted_message_sha256: Some(compute_quote_sha256(p.body, p.sent_at_ms).to_vec()),
    });
    let msg = GenericMessage {
        task: None,
        case: None,
        task_list_marker: None,
        message_id: message_id.to_string(),
        content: Some(generic_message::Content::Text(Text {
            content: body.to_string(),
            mentions: mentions.iter().map(mention_to_proto).collect(),
            quote,
        })),
    };
    encode_generic_message(&msg, "Text")
}

/// Build a `GenericMessage` whose content is a `Knock` (nudge). Step 26.
///
/// # Errors
/// `KupferdrahtError::Encoding` if `prost::Message::encode` fails.
pub(crate) fn build_knock_generic_message(
    hot_knock: bool,
) -> Result<(String, Vec<u8>), KupferdrahtError> {
    let message_id = Uuid::new_v4().to_string();
    let bytes = encode_knock_generic_message(&message_id, hot_knock)?;
    Ok((message_id, bytes))
}

/// [`build_knock_generic_message`] under a caller-supplied `message_id`
/// (resend, Step 79).
pub(crate) fn encode_knock_generic_message(
    message_id: &str,
    hot_knock: bool,
) -> Result<Vec<u8>, KupferdrahtError> {
    debug_assert!(
        !message_id.is_empty(),
        "encode_knock_generic_message: message_id must be set"
    );
    let msg = GenericMessage {
        task: None,
        case: None,
        task_list_marker: None,
        message_id: message_id.to_string(),
        content: Some(generic_message::Content::Knock(Knock { hot_knock })),
    };
    encode_generic_message(&msg, "Knock")
}

/// Build a `GenericMessage` whose content is a `Location` (shared map
/// point). Step 26.
///
/// # Errors
/// `KupferdrahtError::Encoding` if `prost::Message::encode` fails.
pub(crate) fn build_location_generic_message(
    latitude: f32,
    longitude: f32,
    name: Option<&str>,
    zoom: Option<i32>,
) -> Result<(String, Vec<u8>), KupferdrahtError> {
    let message_id = Uuid::new_v4().to_string();
    let bytes = encode_location_generic_message(&message_id, latitude, longitude, name, zoom)?;
    Ok((message_id, bytes))
}

/// [`build_location_generic_message`] under a caller-supplied
/// `message_id` (resend, Step 79).
pub(crate) fn encode_location_generic_message(
    message_id: &str,
    latitude: f32,
    longitude: f32,
    name: Option<&str>,
    zoom: Option<i32>,
) -> Result<Vec<u8>, KupferdrahtError> {
    debug_assert!(
        !message_id.is_empty(),
        "encode_location_generic_message: message_id must be set"
    );
    let msg = GenericMessage {
        task: None,
        case: None,
        task_list_marker: None,
        message_id: message_id.to_string(),
        content: Some(generic_message::Content::Location(Location {
            longitude,
            latitude,
            name: name.map(str::to_string),
            zoom,
        })),
    };
    encode_generic_message(&msg, "Location")
}

/// Build a `GenericMessage` whose content is Wire's `Ephemeral`
/// wrapper (field 18) around a `Text`: the self-deleting text every
/// official client renders with a countdown. Step 89.
///
/// `expire_after_ms` is the TTL the sender chose, in milliseconds
/// (`Ephemeral.expire_after_millis`); range validation is the
/// caller's job (`timeline/ephemeral.rs`). The wrapper carries no
/// mentions and no quote (the OTP-shaped case).
///
/// # Errors
/// `KupferdrahtError::Encoding` if `prost::Message::encode` fails.
pub(crate) fn build_ephemeral_text_generic_message(
    body: &str,
    expire_after_ms: i64,
) -> Result<(String, Vec<u8>), KupferdrahtError> {
    let message_id = Uuid::new_v4().to_string();
    let bytes = encode_ephemeral_text_generic_message(&message_id, body, expire_after_ms)?;
    Ok((message_id, bytes))
}

/// [`build_ephemeral_text_generic_message`] under a caller-supplied
/// `message_id` — the resend path re-wraps a failed ephemeral row with
/// its persisted TTL (Step 79).
pub(crate) fn encode_ephemeral_text_generic_message(
    message_id: &str,
    body: &str,
    expire_after_ms: i64,
) -> Result<Vec<u8>, KupferdrahtError> {
    debug_assert!(
        !message_id.is_empty(),
        "encode_ephemeral_text_generic_message: message_id must be set"
    );
    debug_assert!(
        expire_after_ms > 0,
        "encode_ephemeral_text_generic_message: ttl must be validated by the caller"
    );
    let msg = GenericMessage {
        task: None,
        case: None,
        task_list_marker: None,
        message_id: message_id.to_string(),
        content: Some(generic_message::Content::Ephemeral(Ephemeral {
            expire_after_millis: expire_after_ms,
            content: Some(ephemeral::Content::Text(Text {
                content: body.to_string(),
                mentions: Vec::new(),
                quote: None,
            })),
        })),
    };
    encode_generic_message(&msg, "Ephemeral")
}

/// Mode-c fallback for a task message (ADR-0014): the human-readable
/// `Text` official Wire clients render, plus the assignee mention
/// span (UTF-16 offsets, like every `MentionInput`).
#[derive(Debug, Clone)]
pub(crate) struct TaskFallback<'a> {
    pub text: &'a str,
    pub mentions: &'a [MentionInput],
}

/// Build a `GenericMessage` carrying a `TaskPayload` in field 262000 —
/// deliberately OUTSIDE the `content` oneof (ADR-0014). Step 60a.
///
/// `fallback: Some(..)` is mode c (normal chats): the `Text` oneof
/// variant is ALSO set so official Wire clients render a readable
/// line. `fallback: None` is mode a (dedicated task-list rooms): the
/// oneof stays unset and official clients ignore the message like an
/// unknown variant.
///
/// # Errors
/// * `KupferdrahtError::InvalidInput` if `payload.task_id` is empty.
/// * `KupferdrahtError::Encoding` if `prost::Message::encode` fails.
pub(crate) fn build_task_generic_message(
    payload: TaskPayload,
    fallback: Option<TaskFallback<'_>>,
) -> Result<(String, Vec<u8>), KupferdrahtError> {
    if payload.task_id.is_empty() {
        return Err(KupferdrahtError::InvalidInput(
            "build_task_generic_message: task_id must be non-empty".into(),
        ));
    }
    let message_id = Uuid::new_v4().to_string();
    let content = fallback.map(|f| {
        generic_message::Content::Text(Text {
            content: f.text.to_string(),
            mentions: f.mentions.iter().map(mention_to_proto).collect(),
            quote: None,
        })
    });
    let msg = GenericMessage {
        message_id: message_id.clone(),
        content,
        task: Some(payload),
        case: None,
        task_list_marker: None,
    };
    let mut bytes = Vec::with_capacity(msg.encoded_len());
    msg.encode(&mut bytes)
        .map_err(|e| KupferdrahtError::Encoding(format!("encode GenericMessage: {e}")))?;
    Ok((message_id, bytes))
}

/// Build the `GenericMessage` bytes for one case snapshot (Step 72c,
/// ADR-0018). Cases NEVER carry a fallback: the `content` oneof and
/// the task field stay unset — plain Wire clients must see the same
/// shape as an unknown variant and drop it (pinned by
/// `build_case_leaves_content_oneof_and_task_unset`).
///
/// # Errors
/// * `KupferdrahtError::InvalidInput` if `payload.case_ref` is empty.
/// * `KupferdrahtError::Encoding` if `prost::Message::encode` fails.
pub(crate) fn build_case_generic_message(
    payload: CasePayload,
) -> Result<(String, Vec<u8>), KupferdrahtError> {
    if payload.case_ref.is_empty() {
        return Err(KupferdrahtError::InvalidInput(
            "build_case_generic_message: case_ref must be non-empty".into(),
        ));
    }
    let message_id = Uuid::new_v4().to_string();
    let msg = GenericMessage {
        message_id: message_id.clone(),
        content: None,
        task: None,
        case: Some(payload),
        task_list_marker: None,
    };
    let mut bytes = Vec::with_capacity(msg.encoded_len());
    msg.encode(&mut bytes)
        .map_err(|e| KupferdrahtError::Encoding(format!("encode GenericMessage: {e}")))?;
    Ok((message_id, bytes))
}

/// Build the `GenericMessage` bytes for one task-list-marker state
/// (Step 75, ADR-0020). Like the case, the marker NEVER carries a
/// fallback: the `content` oneof and both sibling fields stay unset —
/// official clients must see the same shape as an unknown variant
/// (pinned by `marker_builder_sets_bare_field_only`).
///
/// # Errors
/// `KupferdrahtError::Encoding` if `prost::Message::encode` fails.
pub(crate) fn build_task_list_marker_generic_message(
    marked: bool,
) -> Result<(String, Vec<u8>), KupferdrahtError> {
    let message_id = Uuid::new_v4().to_string();
    let msg = GenericMessage {
        message_id: message_id.clone(),
        content: None,
        task: None,
        case: None,
        task_list_marker: Some(TaskListMarkerPayload { marked }),
    };
    let mut bytes = Vec::with_capacity(msg.encoded_len());
    msg.encode(&mut bytes)
        .map_err(|e| KupferdrahtError::Encoding(format!("encode GenericMessage: {e}")))?;
    Ok((message_id, bytes))
}

/// Build a `GenericMessage` whose content is `MessageEdit`
/// replacing `replacing_message_id`'s body with `new_body`. Returns
/// `(edit_event_message_id, prost_encoded_bytes)`. Step 6-II-c.
///
/// The edit event itself has a fresh UUID — distinct from the
/// parent's `message_id`. Wire's UI uses the edit's `message_id`
/// for delivery-receipt tracking and the `replacing_message_id`
/// inside the body to find the row to mutate.
///
/// # Errors
/// `KupferdrahtError::Encoding` if `prost::Message::encode_to_vec` fails.
pub(crate) fn build_message_edit_generic_message(
    replacing_message_id: &str,
    new_body: &str,
    mentions: &[MentionInput],
) -> Result<(String, Vec<u8>), KupferdrahtError> {
    let edit_event_id = Uuid::new_v4().to_string();
    let msg = GenericMessage {
        task: None,
        case: None,
        task_list_marker: None,
        message_id: edit_event_id.clone(),
        content: Some(generic_message::Content::Edited(MessageEdit {
            replacing_message_id: replacing_message_id.to_string(),
            content: Some(message_edit::Content::Text(Text {
                content: new_body.to_string(),
                mentions: mentions.iter().map(mention_to_proto).collect(),
                quote: None,
            })),
        })),
    };
    let mut bytes = Vec::with_capacity(msg.encoded_len());
    msg.encode(&mut bytes)
        .map_err(|e| KupferdrahtError::Encoding(format!("encode MessageEdit: {e}")))?;
    Ok((edit_event_id, bytes))
}

/// Build a `GenericMessage` whose content is a `Confirmation
/// { type: READ }` carrying `message_ids` as the read batch.
/// Step 6-II-f.
///
/// `message_ids[0]` becomes `first_message_id`; the rest go
/// into `more_message_ids`. Returns `InvalidInput` if the
/// slice is empty (Wire's schema marks `first_message_id` as
/// required).
///
/// # Errors
/// `KupferdrahtError::InvalidInput` if `message_ids` is empty.
/// `KupferdrahtError::Encoding` if `prost::Message::encode_to_vec` fails.
pub(crate) fn build_read_confirmation_generic_message(
    message_ids: &[String],
) -> Result<(String, Vec<u8>), KupferdrahtError> {
    build_confirmation_generic_message(
        message_ids,
        confirmation::Type::Read,
        "build_read_confirmation",
    )
}

/// Build a `GenericMessage` whose content is a `Confirmation
/// { type: DELIVERED }` carrying `message_ids` as the delivered batch.
/// Step 25b — mirror of [`build_read_confirmation_generic_message`].
///
/// # Errors
/// `KupferdrahtError::InvalidInput` if `message_ids` is empty.
/// `KupferdrahtError::Encoding` if `prost::Message::encode` fails.
pub(crate) fn build_delivered_confirmation_generic_message(
    message_ids: &[String],
) -> Result<(String, Vec<u8>), KupferdrahtError> {
    build_confirmation_generic_message(
        message_ids,
        confirmation::Type::Delivered,
        "build_delivered_confirmation",
    )
}

/// Shared builder for a `Confirmation` `GenericMessage` of either type.
/// `message_ids[0]` becomes `first_message_id`; the rest go into
/// `more_message_ids`. `op` names the caller for the empty-slice error.
fn build_confirmation_generic_message(
    message_ids: &[String],
    conf_type: confirmation::Type,
    op: &str,
) -> Result<(String, Vec<u8>), KupferdrahtError> {
    let (first, rest) = message_ids.split_first().ok_or_else(|| {
        KupferdrahtError::InvalidInput(format!("{op}: message_ids must be non-empty"))
    })?;
    let event_id = Uuid::new_v4().to_string();
    let msg = GenericMessage {
        task: None,
        case: None,
        task_list_marker: None,
        message_id: event_id.clone(),
        content: Some(generic_message::Content::Confirmation(Confirmation {
            first_message_id: first.clone(),
            r#type: conf_type as i32,
            more_message_ids: rest.to_vec(),
        })),
    };
    let mut bytes = Vec::with_capacity(msg.encoded_len());
    msg.encode(&mut bytes)
        .map_err(|e| KupferdrahtError::Encoding(format!("encode Confirmation: {e}")))?;
    Ok((event_id, bytes))
}

/// Build a `GenericMessage` whose content is a `Reaction`
/// reporting the sender's full set of emojis on `target_message_id`
/// after the toggle. `emojis_csv` is comma-separated; an empty
/// string clears the sender's reactions on the parent. Step 6-II-e.
///
/// # Errors
/// `KupferdrahtError::Encoding` if `prost::Message::encode_to_vec` fails.
pub(crate) fn build_reaction_generic_message(
    target_message_id: &str,
    emojis_csv: &str,
) -> Result<(String, Vec<u8>), KupferdrahtError> {
    let event_id = Uuid::new_v4().to_string();
    let msg = GenericMessage {
        task: None,
        case: None,
        task_list_marker: None,
        message_id: event_id.clone(),
        content: Some(generic_message::Content::Reaction(Reaction {
            emoji: Some(emojis_csv.to_string()),
            message_id: target_message_id.to_string(),
        })),
    };
    let mut bytes = Vec::with_capacity(msg.encoded_len());
    msg.encode(&mut bytes)
        .map_err(|e| KupferdrahtError::Encoding(format!("encode Reaction: {e}")))?;
    Ok((event_id, bytes))
}

/// Build a `GenericMessage` whose content is `MessageDelete`
/// retiring `target_message_id`. Returns
/// `(delete_event_message_id, prost_encoded_bytes)`. Step 6-II-d.
///
/// Like the edit builder, the delete event's own `message_id` is
/// fresh; the payload references the parent. The `target_message_id`
/// here is the *original* message id — Wire-Web's chain semantics
/// for delete reach back through the chain pointer on the
/// receive side (`Store::apply_message_delete` matches via
/// `message_id` OR `current_edit_event_id`).
///
/// # Errors
/// `KupferdrahtError::Encoding` if `prost::Message::encode_to_vec` fails.
pub(crate) fn build_message_delete_generic_message(
    target_message_id: &str,
) -> Result<(String, Vec<u8>), KupferdrahtError> {
    let delete_event_id = Uuid::new_v4().to_string();
    let msg = GenericMessage {
        task: None,
        case: None,
        task_list_marker: None,
        message_id: delete_event_id.clone(),
        content: Some(generic_message::Content::Deleted(MessageDelete {
            message_id: target_message_id.to_string(),
        })),
    };
    let mut bytes = Vec::with_capacity(msg.encoded_len());
    msg.encode(&mut bytes)
        .map_err(|e| KupferdrahtError::Encoding(format!("encode MessageDelete: {e}")))?;
    Ok((delete_event_id, bytes))
}

/// Input bundle for [`build_asset_generic_message`]. Step 11f.
///
/// `original` fields populate `Asset.Original`; `remote` fields
/// populate `Asset.RemoteData.uploaded`. The receiver decodes both
/// halves to reconstruct the asset reference.
pub(crate) struct AssetMessageInput<'a> {
    pub mime_type: &'a str,
    pub size: u64,
    pub filename: Option<&'a str>,
    pub otr_key: &'a [u8],
    pub sha256: &'a [u8],
    pub asset_id: &'a str,
    pub asset_token: Option<&'a str>,
    pub asset_domain: &'a str,
}

impl std::fmt::Debug for AssetMessageInput<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Step 101: secrets redacted (`crate::redact`).
        f.debug_struct("AssetMessageInput")
            .field("mime_type", &self.mime_type)
            .field("size", &self.size)
            .field("filename", &self.filename)
            .field("otr_key", &crate::redact::byte_count(self.otr_key))
            .field("sha256", &self.sha256)
            .field("asset_id", &self.asset_id)
            .field(
                "asset_token",
                &crate::redact::hidden_option(self.asset_token.as_ref()),
            )
            .field("asset_domain", &self.asset_domain)
            .finish()
    }
}

/// Build a `GenericMessage` whose content is an `Asset` variant with
/// `Original` + `RemoteData.uploaded`. Step 11f. Mirrors the
/// shape every Wire client produces — see `docs/wire/asset-protocol.md`
/// §Q4 for the field-number authority.
///
/// `EncryptionAlgorithm::AES_CBC` (= 0) is emitted unconditionally
/// per ADR-0011 §AES-GCM toleration: outbound is always CBC.
/// `expects_read_confirmation` is set to `false` — Kupferdraht
/// does not request read receipts on asset sends (a future
/// enhancement could thread the user's preference through).
///
/// # Errors
/// `KupferdrahtError::Encoding` if `prost::Message::encode_to_vec`
/// fails (cannot happen for the constructed shape, but the
/// `Result` matches the other builders).
pub(crate) fn build_asset_generic_message(
    input: AssetMessageInput<'_>,
) -> Result<(String, Vec<u8>), KupferdrahtError> {
    let message_id = Uuid::new_v4().to_string();
    let bytes = encode_asset_generic_message(&message_id, input)?;
    Ok((message_id, bytes))
}

/// [`build_asset_generic_message`] under a caller-supplied `message_id`
/// — the resend path rebuilds the reference from the persisted
/// `attachments` row, the blob already sits on `/v14/assets` (Step 79).
pub(crate) fn encode_asset_generic_message(
    message_id: &str,
    input: AssetMessageInput<'_>,
) -> Result<Vec<u8>, KupferdrahtError> {
    debug_assert!(
        !message_id.is_empty(),
        "encode_asset_generic_message: message_id must be set"
    );
    let original = asset::Original {
        mime_type: input.mime_type.to_string(),
        size: input.size,
        name: input.filename.map(str::to_string),
        meta_data: None,
        source: None,
        caption: None,
    };
    let remote = asset::RemoteData {
        otr_key: input.otr_key.to_vec(),
        sha256: input.sha256.to_vec(),
        asset_id: Some(input.asset_id.to_string()),
        asset_token: input.asset_token.map(str::to_string),
        asset_domain: Some(input.asset_domain.to_string()),
        encryption: Some(EncryptionAlgorithm::AesCbc as i32),
    };
    let msg = GenericMessage {
        task: None,
        case: None,
        task_list_marker: None,
        message_id: message_id.to_string(),
        content: Some(generic_message::Content::Asset(Asset {
            original: Some(original),
            status: Some(asset::Status::Uploaded(remote)),
            expects_read_confirmation: Some(false),
        })),
    };
    encode_generic_message(&msg, "Asset")
}

/// Compute the digest Wire requires inside `Quote.quoted_message_sha256`:
///
/// ```text
///   sha256( 0xFE 0xFF || body_utf16_be || timestamp_seconds_be )
/// ```
///
/// Where `timestamp_seconds_be` is the parent message's
/// wall-clock send-time in seconds (NOT milliseconds), encoded as
/// a big-endian i64. The body is UTF-16 big-endian, no BOM
/// prefix beyond the leading `0xFE 0xFF` constant. This is the
/// digest format Wire's server and other Wire clients require so
/// the quote renders on receipt.
pub(crate) fn compute_quote_sha256(body: &str, sent_at_ms: i64) -> [u8; 32] {
    use sha2::{Digest as _, Sha256};
    let mut hasher = Sha256::new();
    hasher.update([0xFEu8, 0xFFu8]);
    for code_unit in body.encode_utf16() {
        hasher.update(code_unit.to_be_bytes());
    }
    let timestamp_seconds = sent_at_ms / 1000;
    hasher.update(timestamp_seconds.to_be_bytes());
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_quote() -> Option<QuoteParent<'static>> {
        None
    }

    const NO_MENTIONS: &[MentionInput] = &[];

    #[test]
    fn build_text_yields_uuid_message_id_and_round_trippable_bytes() {
        let (id, bytes) =
            build_text_generic_message("hello", no_quote(), NO_MENTIONS).expect("build");
        // UUID v4 hyphenated form is 36 chars.
        assert_eq!(id.len(), 36, "message_id must be a UUID v4: {id}");
        // Round-trip through prost.
        let parsed = GenericMessage::decode(bytes.as_slice()).expect("decode");
        assert_eq!(parsed.message_id, id);
        match parsed.content {
            Some(generic_message::Content::Text(t)) => {
                assert_eq!(t.content, "hello");
                assert!(t.quote.is_none(), "no reply_to → no Quote");
            }
            other => panic!("expected Text content, got {other:?}"),
        }
    }

    #[test]
    fn build_text_message_ids_are_unique_per_call() {
        let (a, _) = build_text_generic_message("x", no_quote(), NO_MENTIONS).expect("a");
        let (b, _) = build_text_generic_message("x", no_quote(), NO_MENTIONS).expect("b");
        assert_ne!(a, b, "fresh UUID per call");
    }

    #[test]
    fn build_text_carries_empty_body_verbatim() {
        let (_, bytes) =
            build_text_generic_message("", no_quote(), NO_MENTIONS).expect("empty body");
        let parsed = GenericMessage::decode(bytes.as_slice()).expect("decode");
        match parsed.content {
            Some(generic_message::Content::Text(t)) => assert_eq!(t.content, ""),
            _ => panic!("missing text content"),
        }
    }

    #[test]
    fn build_text_with_quote_parent_populates_quote_field_with_digest() {
        let parent = QuoteParent {
            message_id: "fa497604-8a7d-4e35-8603-d8987ec29f45",
            body: "Hi",
            sent_at_ms: 1_122_826_760,
        };
        let (_, bytes) =
            build_text_generic_message("re: hi", Some(parent.clone()), NO_MENTIONS).expect("build");
        let parsed = GenericMessage::decode(bytes.as_slice()).expect("decode");
        match parsed.content {
            Some(generic_message::Content::Text(t)) => {
                assert_eq!(t.content, "re: hi");
                let quote = t.quote.expect("quote must be Some when reply is supplied");
                assert_eq!(quote.quoted_message_id, parent.message_id);
                let digest = quote
                    .quoted_message_sha256
                    .expect("digest must be present so Wire renders the quote");
                assert_eq!(digest.len(), 32, "sha256 digest is 32 bytes");
                assert_eq!(
                    digest,
                    compute_quote_sha256("Hi", 1_122_826_760).to_vec(),
                    "digest must match compute_quote_sha256"
                );
            }
            other => panic!("expected Text content, got {other:?}"),
        }
    }

    #[test]
    fn build_text_without_quote_omits_quote_on_the_wire() {
        // Verify by parsing — Wire interprets a missing `quote` field
        // as "not a reply"; the Rust struct's `Option<Quote>::None`
        // emits no bytes for tag 5 (Quote field number).
        let (_, bytes) = build_text_generic_message("hi", no_quote(), NO_MENTIONS).expect("build");
        let parsed = GenericMessage::decode(bytes.as_slice()).expect("decode");
        match parsed.content {
            Some(generic_message::Content::Text(t)) => assert!(t.quote.is_none()),
            other => panic!("expected Text content, got {other:?}"),
        }
    }

    /// Pins the digest for an emoji-bearing body so the UTF-16 BE
    /// surrogate-pair encoding path cannot drift silently — a single
    /// wrong code unit would change the digest and Wire web would
    /// stop rendering the quote.
    #[test]
    fn compute_quote_sha256_pins_digest_for_emoji_body() {
        let body = "Hello \u{1F602}\u{1F622}!";
        let sent_at_ms: i64 = 1_122_826_760;
        let digest = compute_quote_sha256(body, sent_at_ms);
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex, "4f557917b54b72accdadf6ca1d948d4da2ab3b5f2fe4050de38af6fbac614a4d",
            "digest format (0xFE 0xFF || UTF-16 BE body || i64 BE seconds) must not drift"
        );
    }

    #[test]
    fn build_message_edit_carries_replacing_id_and_new_text() {
        let parent = "16ab51f7-4b4a-42ac-8aaf-d80c1a7e7928";
        let (edit_id, bytes) =
            build_message_edit_generic_message(parent, "edited body", NO_MENTIONS).expect("build");
        assert_eq!(
            edit_id.len(),
            36,
            "edit event message_id must itself be a UUID v4: {edit_id}"
        );
        assert_ne!(edit_id, parent, "edit event id is fresh, not the parent's");
        let parsed = GenericMessage::decode(bytes.as_slice()).expect("decode");
        match parsed.content {
            Some(generic_message::Content::Edited(edit)) => {
                assert_eq!(edit.replacing_message_id, parent);
                match edit.content {
                    Some(message_edit::Content::Text(t)) => {
                        assert_eq!(t.content, "edited body");
                        assert!(t.quote.is_none(), "edits do not carry a Quote");
                    }
                    other => panic!("expected MessageEdit::Text, got {other:?}"),
                }
            }
            other => panic!("expected Content::Edited, got {other:?}"),
        }
    }

    #[test]
    fn build_read_confirmation_single_message_round_trips() {
        let target = "abc-123-only";
        let (event_id, bytes) =
            build_read_confirmation_generic_message(&[target.into()]).expect("build");
        assert_eq!(event_id.len(), 36);
        let parsed = GenericMessage::decode(bytes.as_slice()).expect("decode");
        match parsed.content {
            Some(generic_message::Content::Confirmation(c)) => {
                assert_eq!(c.first_message_id, target);
                assert_eq!(c.r#type, confirmation::Type::Read as i32);
                assert!(c.more_message_ids.is_empty());
            }
            other => panic!("expected Content::Confirmation, got {other:?}"),
        }
    }

    #[test]
    fn build_delivered_confirmation_sets_delivered_type() {
        let ids = vec!["id-a".to_string(), "id-b".to_string()];
        let (event_id, bytes) = build_delivered_confirmation_generic_message(&ids).expect("build");
        assert_eq!(event_id.len(), 36);
        let parsed = GenericMessage::decode(bytes.as_slice()).expect("decode");
        match parsed.content {
            Some(generic_message::Content::Confirmation(c)) => {
                assert_eq!(c.r#type, confirmation::Type::Delivered as i32);
                assert_eq!(c.first_message_id, "id-a");
                assert_eq!(c.more_message_ids, vec!["id-b".to_string()]);
            }
            other => panic!("expected Content::Confirmation, got {other:?}"),
        }
    }

    #[test]
    fn build_delivered_confirmation_rejects_empty() {
        assert!(build_delivered_confirmation_generic_message(&[]).is_err());
    }

    #[test]
    fn build_knock_round_trips_hot_flag() {
        for hot in [false, true] {
            let (event_id, bytes) = build_knock_generic_message(hot).expect("build");
            assert_eq!(event_id.len(), 36);
            let parsed = GenericMessage::decode(bytes.as_slice()).expect("decode");
            match parsed.content {
                Some(generic_message::Content::Knock(k)) => assert_eq!(k.hot_knock, hot),
                other => panic!("expected Content::Knock, got {other:?}"),
            }
        }
    }

    #[test]
    fn build_location_round_trips_all_fields() {
        let (event_id, bytes) =
            build_location_generic_message(52.52, 13.405, Some("Berlin"), Some(12)).expect("build");
        assert_eq!(event_id.len(), 36);
        let parsed = GenericMessage::decode(bytes.as_slice()).expect("decode");
        match parsed.content {
            Some(generic_message::Content::Location(l)) => {
                assert!((l.latitude - 52.52).abs() < f32::EPSILON);
                assert!((l.longitude - 13.405).abs() < f32::EPSILON);
                assert_eq!(l.name.as_deref(), Some("Berlin"));
                assert_eq!(l.zoom, Some(12));
            }
            other => panic!("expected Content::Location, got {other:?}"),
        }
    }

    #[test]
    fn build_location_omits_optional_fields_when_none() {
        let (_, bytes) = build_location_generic_message(0.0, 0.0, None, None).expect("build");
        let parsed = GenericMessage::decode(bytes.as_slice()).expect("decode");
        match parsed.content {
            Some(generic_message::Content::Location(l)) => {
                assert!(l.name.is_none());
                assert!(l.zoom.is_none());
            }
            other => panic!("expected Content::Location, got {other:?}"),
        }
    }

    #[test]
    fn build_read_confirmation_batch_uses_first_plus_more() {
        let ids = vec!["id-a".to_string(), "id-b".to_string(), "id-c".to_string()];
        let (_, bytes) = build_read_confirmation_generic_message(&ids).expect("build");
        let parsed = GenericMessage::decode(bytes.as_slice()).expect("decode");
        match parsed.content {
            Some(generic_message::Content::Confirmation(c)) => {
                assert_eq!(c.first_message_id, "id-a");
                assert_eq!(
                    c.more_message_ids,
                    vec!["id-b".to_string(), "id-c".to_string()]
                );
            }
            other => panic!("expected Content::Confirmation, got {other:?}"),
        }
    }

    #[test]
    fn build_read_confirmation_rejects_empty_slice() {
        let err = build_read_confirmation_generic_message(&[])
            .expect_err("empty message_ids must be rejected client-side");
        assert!(matches!(err, KupferdrahtError::InvalidInput(_)));
    }

    #[test]
    fn build_reaction_carries_target_and_emoji_csv() {
        let target = "896aebdd-0fdd-416e-9178-aefd11146caf";
        let (event_id, bytes) = build_reaction_generic_message(target, "👍,❤️").expect("build");
        assert_eq!(event_id.len(), 36);
        let parsed = GenericMessage::decode(bytes.as_slice()).expect("decode");
        match parsed.content {
            Some(generic_message::Content::Reaction(r)) => {
                assert_eq!(r.message_id, target);
                assert_eq!(r.emoji.as_deref(), Some("👍,❤️"));
            }
            other => panic!("expected Content::Reaction, got {other:?}"),
        }
    }

    #[test]
    fn build_reaction_carries_empty_csv_for_clear_all() {
        let target = "abc-123";
        let (_, bytes) = build_reaction_generic_message(target, "").expect("build");
        let parsed = GenericMessage::decode(bytes.as_slice()).expect("decode");
        match parsed.content {
            Some(generic_message::Content::Reaction(r)) => {
                assert_eq!(r.emoji.as_deref(), Some(""));
                assert_eq!(r.message_id, target);
            }
            other => panic!("expected Content::Reaction, got {other:?}"),
        }
    }

    #[test]
    fn build_message_delete_carries_target_id() {
        let target = "896aebdd-0fdd-416e-9178-aefd11146caf";
        let (delete_id, bytes) = build_message_delete_generic_message(target).expect("build");
        assert_eq!(
            delete_id.len(),
            36,
            "delete event message_id must itself be a UUID v4: {delete_id}"
        );
        assert_ne!(
            delete_id, target,
            "delete event id is fresh, not the target's"
        );
        let parsed = GenericMessage::decode(bytes.as_slice()).expect("decode");
        match parsed.content {
            Some(generic_message::Content::Deleted(d)) => {
                assert_eq!(d.message_id, target);
            }
            other => panic!("expected Content::Deleted, got {other:?}"),
        }
    }

    /// Pins the digest for a plain ASCII URL body — same algorithm
    /// as the emoji vector but without surrogate pairs, so the two
    /// together cover both the simple and the multi-code-unit paths.
    #[test]
    fn compute_quote_sha256_pins_digest_for_ascii_url_body() {
        let body = "https://youtu.be/Y6ljFaKRTrI?si=ErRXaTIctWbga_Lp";
        let sent_at_ms: i64 = 1_122_826_760;
        let digest = compute_quote_sha256(body, sent_at_ms);
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "817f4ea1a2499469cf1c918591dd216386970b753e819cea277b0b06a1e6e326",
        );
    }

    // ---- Step 89: Ephemeral (field 18) codec pins ------------------
    //
    // The only enforceable place for the wire shape: integration
    // tests see MLS ciphertext. An implementation that set the
    // top-level Text instead of the wrapper would pass every HTTP-hit
    // assertion and render as a plain (never-deleting) text on
    // official clients.

    #[test]
    fn build_ephemeral_text_sets_field_18_with_ttl_and_inner_text() {
        let (id, bytes) =
            build_ephemeral_text_generic_message("Your one-time code: 123456", 300_000)
                .expect("build");
        assert_eq!(id.len(), 36, "message_id must be a UUID v4: {id}");
        let parsed = GenericMessage::decode(bytes.as_slice()).expect("decode");
        assert_eq!(parsed.message_id, id);
        let Some(generic_message::Content::Ephemeral(eph)) = parsed.content else {
            panic!(
                "expected Content::Ephemeral (field 18), got {:?}",
                parsed.content
            );
        };
        assert_eq!(eph.expire_after_millis, 300_000, "TTL rides the wrapper");
        match eph.content {
            Some(ephemeral::Content::Text(t)) => {
                assert_eq!(t.content, "Your one-time code: 123456");
                assert!(t.mentions.is_empty(), "V1 ephemeral carries no mentions");
                assert!(t.quote.is_none(), "V1 ephemeral carries no quote");
            }
            other => panic!("expected inner Text, got {other:?}"),
        }
        assert!(parsed.task.is_none(), "field 262000 stays unset");
        assert!(parsed.case.is_none(), "field 262001 stays unset");
        assert!(
            parsed.task_list_marker.is_none(),
            "field 262002 stays unset"
        );
    }

    #[test]
    fn build_ephemeral_text_message_ids_are_unique_per_call() {
        let (a, _) = build_ephemeral_text_generic_message("x", 1_000).expect("a");
        let (b, _) = build_ephemeral_text_generic_message("x", 1_000).expect("b");
        assert_ne!(a, b, "fresh UUID per call");
    }

    /// Kupferdraht declares only the `Text` inner arm of the wrapper.
    /// An undeclared inner arm must decode to `content = None` (the
    /// tolerated-unknown path) — never a decode error.
    #[test]
    fn ephemeral_with_undeclared_inner_arm_decodes_to_none_content() {
        // Hand-encoded: GenericMessage { 1: "m-1", 18: Ephemeral {
        // 1: 5000, 4: Knock { 1: false } } } — Knock is field 4 of
        // the wrapper's oneof in Wire's schema.
        let mut knock = Vec::new();
        knock.extend_from_slice(&[0x08, 0x00]); // field 1 varint 0
        let mut eph = Vec::new();
        eph.extend_from_slice(&[0x08, 0x88, 0x27]); // field 1 varint 5000
        eph.push(0x22); // field 4, length-delimited
        eph.push(knock.len() as u8);
        eph.extend_from_slice(&knock);
        let mut m = Vec::new();
        m.extend_from_slice(&[0x0a, 0x03]); // field 1 string "m-1"
        m.extend_from_slice(b"m-1");
        m.extend_from_slice(&[0x92, 0x01]); // field 18, length-delimited
        m.push(eph.len() as u8);
        m.extend_from_slice(&eph);

        let parsed = GenericMessage::decode(m.as_slice()).expect("decode must tolerate the arm");
        assert_eq!(parsed.message_id, "m-1");
        let Some(generic_message::Content::Ephemeral(eph)) = parsed.content else {
            panic!("expected Content::Ephemeral, got {:?}", parsed.content);
        };
        assert_eq!(eph.expire_after_millis, 5_000);
        assert!(
            eph.content.is_none(),
            "an undeclared inner arm decodes to None, got {:?}",
            eph.content
        );
    }

    // ---- Step 11a: Asset content variant round-trip --------------

    /// Build a minimally-populated `Asset` carrying both the
    /// `Original` metadata and an `uploaded` `RemoteData` block —
    /// the canonical send-side shape.
    fn asset_with_uploaded_remote() -> Asset {
        Asset {
            original: Some(asset::Original {
                mime_type: "image/png".to_string(),
                size: 1024,
                name: Some("photo.png".to_string()),
                meta_data: Some(asset::original::MetaData::Image(asset::ImageMetaData {
                    width: 800,
                    height: 600,
                    tag: None,
                })),
                source: None,
                caption: None,
            }),
            status: Some(asset::Status::Uploaded(asset::RemoteData {
                otr_key: vec![0xAA; 32],
                sha256: vec![0xBB; 32],
                asset_id: Some("asset-id-uuid".to_string()),
                asset_token: Some("asset-token-jwt".to_string()),
                asset_domain: Some("assets.example.org".to_string()),
                encryption: Some(EncryptionAlgorithm::AesCbc as i32),
            })),
            expects_read_confirmation: Some(true),
        }
    }

    #[test]
    fn asset_round_trips_through_generic_message() {
        let message_id = Uuid::new_v4().to_string();
        let outbound = GenericMessage {
            task: None,
            case: None,
            task_list_marker: None,
            message_id: message_id.clone(),
            content: Some(generic_message::Content::Asset(asset_with_uploaded_remote())),
        };
        let mut bytes = Vec::with_capacity(outbound.encoded_len());
        outbound.encode(&mut bytes).expect("encode");
        let parsed = GenericMessage::decode(bytes.as_slice()).expect("decode");
        assert_eq!(parsed.message_id, message_id);
        let Some(generic_message::Content::Asset(a)) = parsed.content else {
            panic!("expected Content::Asset");
        };
        let original = a.original.expect("original present");
        assert_eq!(original.mime_type, "image/png");
        assert_eq!(original.size, 1024);
        assert_eq!(original.name.as_deref(), Some("photo.png"));
        let Some(asset::original::MetaData::Image(img)) = original.meta_data else {
            panic!("expected ImageMetaData");
        };
        assert_eq!(img.width, 800);
        assert_eq!(img.height, 600);
        assert_eq!(a.expects_read_confirmation, Some(true));
        let Some(asset::Status::Uploaded(remote)) = a.status else {
            panic!("expected Status::Uploaded");
        };
        assert_eq!(remote.otr_key, vec![0xAA; 32]);
        assert_eq!(remote.sha256, vec![0xBB; 32]);
        assert_eq!(remote.asset_id.as_deref(), Some("asset-id-uuid"));
        assert_eq!(remote.asset_token.as_deref(), Some("asset-token-jwt"));
        assert_eq!(remote.asset_domain.as_deref(), Some("assets.example.org"));
        assert_eq!(remote.encryption, Some(EncryptionAlgorithm::AesCbc as i32));
    }

    #[test]
    fn asset_with_not_uploaded_status_round_trips() {
        // Wire's "upload cancelled / failed" sentinel — receivers
        // must decode without panic. Kupferdraht emits only `Uploaded`,
        // but a Wire-Web sender that aborts mid-upload writes
        // `not_uploaded = CANCELLED` into the MLS stream.
        let outbound = Asset {
            original: Some(asset::Original {
                mime_type: "application/pdf".to_string(),
                size: 0,
                name: None,
                meta_data: None,
                source: None,
                caption: None,
            }),
            status: Some(asset::Status::NotUploaded(
                asset::NotUploaded::Cancelled as i32,
            )),
            expects_read_confirmation: None,
        };
        let mut bytes = Vec::with_capacity(outbound.encoded_len());
        outbound.encode(&mut bytes).expect("encode");
        let parsed = Asset::decode(bytes.as_slice()).expect("decode");
        let Some(asset::Status::NotUploaded(reason)) = parsed.status else {
            panic!("expected Status::NotUploaded");
        };
        assert_eq!(reason, asset::NotUploaded::Cancelled as i32);
    }

    #[test]
    fn asset_remote_data_aes_gcm_decodes_for_warn_and_skip_path() {
        // ADR-0011 §AES-GCM toleration: inbound assets flagged
        // `AES_GCM` decode cleanly into the proto so the dispatch
        // layer can see them and surface an "unsupported" placeholder
        // rather than crashing the drain. This test pins the proto's
        // ability to round-trip the GCM enum value.
        let outbound = asset::RemoteData {
            otr_key: vec![0u8; 32],
            sha256: vec![0u8; 32],
            asset_id: Some("a".to_string()),
            asset_token: None,
            asset_domain: None,
            encryption: Some(EncryptionAlgorithm::AesGcm as i32),
        };
        let mut bytes = Vec::with_capacity(outbound.encoded_len());
        outbound.encode(&mut bytes).expect("encode");
        let parsed = asset::RemoteData::decode(bytes.as_slice()).expect("decode");
        assert_eq!(parsed.encryption, Some(EncryptionAlgorithm::AesGcm as i32));
    }

    #[test]
    fn asset_minimal_original_only_round_trips_with_no_status() {
        // Minimal shape: Original metadata present, no upload status
        // yet. Decodes cleanly; dispatch treats absent status as
        // "not yet uploaded" and skips.
        let outbound = Asset {
            original: Some(asset::Original {
                mime_type: "text/plain".to_string(),
                size: 42,
                name: Some("notes.txt".to_string()),
                meta_data: None,
                source: None,
                caption: None,
            }),
            status: None,
            expects_read_confirmation: None,
        };
        let mut bytes = Vec::with_capacity(outbound.encoded_len());
        outbound.encode(&mut bytes).expect("encode");
        let parsed = Asset::decode(bytes.as_slice()).expect("decode");
        assert!(
            parsed.status.is_none(),
            "no status set on outbound, must decode None"
        );
        assert_eq!(parsed.original.expect("original").size, 42);
    }
}

/// Step 60a (ADR-0014) — `TaskPayload` in `GenericMessage` field 262000,
/// outside the `content` oneof. Kept as its own module: the task wire
/// shape is a Kupferdraht extension with its own invariants (fallback
/// coexistence, unknown-status decoding) distinct from Wire's stock
/// vocabulary above.
#[cfg(test)]
mod task_tests {
    use super::*;

    fn full_payload() -> TaskPayload {
        TaskPayload {
            task_id: "7f9c0e5a-1111-4222-8333-444455556666".into(),
            title: "Prepare deploy".into(),
            status: TaskStatus::InProgress as i32,
            description: Some("Ship it — https://example.com/runbook".into()),
            assignee: Some(QualifiedUserId {
                id: "aaaa-bbbb".into(),
                domain: "wire.com".into(),
            }),
            due_at_ms: Some(1_752_000_000_000),
            priority: Some(TaskPriority::High as i32),
            sort_key: Some("m".into()),
        }
    }

    #[test]
    fn build_task_with_fallback_round_trips_payload_and_text() {
        // Mode c (ADR-0014): field 262000 AND the Text oneof are both set —
        // official clients render the text, Kupferdraht parses the payload.
        let mention = MentionInput {
            start: 18,
            length: 5,
            qualified_user_id: "aaaa-bbbb@wire.com".into(),
        };
        let (id, bytes) = build_task_generic_message(
            full_payload(),
            Some(TaskFallback {
                text: "☐ Prepare deploy — @anna",
                mentions: std::slice::from_ref(&mention),
            }),
        )
        .expect("build");
        assert_eq!(id.len(), 36, "message_id must be a UUID v4: {id}");
        let parsed = GenericMessage::decode(bytes.as_slice()).expect("decode");
        assert_eq!(parsed.message_id, id);
        // The task payload survives the round trip in full.
        let task = parsed.task.expect("field 262000 present");
        assert_eq!(task, full_payload());
        assert_eq!(task.status(), TaskStatus::InProgress);
        assert_eq!(task.priority(), TaskPriority::High);
        // The fallback rides in the ordinary Text variant, mention intact.
        match parsed.content {
            Some(generic_message::Content::Text(t)) => {
                assert_eq!(t.content, "☐ Prepare deploy — @anna");
                assert_eq!(t.mentions.len(), 1);
                assert_eq!(t.mentions[0].start, 18);
                assert_eq!(t.mentions[0].length, 5);
            }
            other => panic!("expected Text fallback, got {other:?}"),
        }
    }

    #[test]
    fn build_task_without_fallback_leaves_content_oneof_unset() {
        // Mode a (ADR-0014): dedicated task-list rooms send the bare
        // payload — official clients see the same shape as an unknown
        // variant and ignore it.
        let (_id, bytes) = build_task_generic_message(full_payload(), None).expect("build");
        let parsed = GenericMessage::decode(bytes.as_slice()).expect("decode");
        assert!(parsed.task.is_some(), "field 262000 present");
        assert!(
            parsed.content.is_none(),
            "mode a must leave the content oneof unset"
        );
    }

    /// Step 72c: the ONLY place the no-fallback
    /// contract is enforceable — the integration tests see MLS
    /// ciphertext. An implementation that also set the Text oneof
    /// would pass every HTTP-hit assertion and break ADR-0018.
    #[test]
    fn build_case_leaves_content_oneof_and_task_unset() {
        let payload = CasePayload {
            revision: 1,
            case_ref: "case-1".into(),
            external_ref: None,
            title: Some("Fall X".into()),
            case_type: None,
            state: CaseState::Active as i32,
            state_detail: None,
            assignee: None,
            due_at_ms: None,
            priority: None,
            opened_at_ms: None,
            closed_at_ms: None,
            authority_ref: None,
            extensions: std::collections::HashMap::new(),
        };
        let (id, bytes) = build_case_generic_message(payload).expect("build");
        assert!(!id.is_empty(), "a message_id was generated");
        let parsed = GenericMessage::decode(bytes.as_slice()).expect("decode");
        let case = parsed.case.expect("field 262001 present");
        assert_eq!(case.revision, 1);
        assert_eq!(case.title.as_deref(), Some("Fall X"));
        assert!(
            parsed.content.is_none(),
            "cases NEVER set the content oneof (no text fallback, ADR-0018)"
        );
        assert!(parsed.task.is_none(), "field 262000 stays unset");
        assert_eq!(parsed.message_id, id);
    }

    /// Step 75 (ADR-0020): same enforcement point as the case builder
    /// above — the integration tests see only MLS ciphertext, so the
    /// no-fallback / bare-field contract is pinned here.
    #[test]
    fn marker_builder_sets_bare_field_only() {
        for marked in [true, false] {
            let (id, bytes) = build_task_list_marker_generic_message(marked).expect("build");
            assert_eq!(id.len(), 36, "message_id must be a UUID v4: {id}");
            let parsed = GenericMessage::decode(bytes.as_slice()).expect("decode");
            assert_eq!(parsed.message_id, id);
            let marker = parsed.task_list_marker.expect("field 262002 present");
            assert_eq!(marker.marked, marked, "full marker state rides the payload");
            assert!(
                parsed.content.is_none(),
                "markers NEVER set the content oneof (official clients see nothing)"
            );
            assert!(parsed.task.is_none(), "field 262000 stays unset");
            assert!(parsed.case.is_none(), "field 262001 stays unset");
        }
    }

    #[test]
    fn build_task_minimal_payload_round_trips() {
        let minimal = TaskPayload {
            task_id: "id-1".into(),
            title: "Buy milk".into(),
            status: TaskStatus::Open as i32,
            description: None,
            assignee: None,
            due_at_ms: None,
            priority: None,
            sort_key: None,
        };
        let (_id, bytes) = build_task_generic_message(minimal.clone(), None).expect("build");
        let parsed = GenericMessage::decode(bytes.as_slice()).expect("decode");
        assert_eq!(parsed.task.expect("payload"), minimal);
    }

    #[test]
    fn task_status_unknown_wire_value_decodes_as_open() {
        // Regression guard on the enum numbering: OPEN must stay 0 so
        // prost's accessor default maps unknown future statuses to Open
        // (ADR-0014 — the task stays visible; the next update heals it).
        let mut payload = full_payload();
        payload.status = 99;
        let msg = GenericMessage {
            message_id: "m".into(),
            content: None,
            task: Some(payload),
            case: None,
            task_list_marker: None,
        };
        let mut bytes = Vec::new();
        prost::Message::encode(&msg, &mut bytes).expect("encode");
        let parsed = GenericMessage::decode(bytes.as_slice()).expect("decode");
        assert_eq!(
            parsed.task.expect("payload").status(),
            TaskStatus::Open,
            "unknown status wire value must decode as Open"
        );
    }
}
