// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Task sends (Steps 60a–c): full-state send, read-modify-write
//! update, and fractional-index move — every mutation leaves the
//! device as a complete `TaskPayload`.
use super::{
    Client, FoldedTask, KupferdrahtError, MessageHandle, TaskDraft, TaskItem, TaskPatch,
    TaskStatus, Timeline, apply_task_patch, fractional_between, payload_from_item,
    resolve_move_bounds, task_body, task_fallback, task_record_from_folded,
};

/// Resolve the effective fallback mode for a task send (Step 75,
/// ADR-0020 decision 4): an explicit caller choice always wins;
/// `None` derives the default from the room's task-list marker —
/// marked rooms suppress the fallback, unmarked rooms keep it.
fn resolve_task_fallback(explicit: Option<bool>, room_is_marked: bool) -> bool {
    explicit.unwrap_or(!room_is_marked)
}

impl Timeline {
    /// Send a task into this room (ADR-0014). `with_fallback` selects
    /// the encoding mode: `Some(true)` (normal chats) also sets the
    /// `Text` oneof as human-readable fallback for official Wire
    /// clients; `Some(false)` (dedicated task-list conversations)
    /// leaves the oneof unset. `None` derives the default from the
    /// room's task-list marker (Step 75, ADR-0020): marked rooms
    /// suppress the fallback, unmarked rooms keep it. An explicit
    /// choice always wins. Step 60a.
    ///
    /// # Errors
    /// Same downstream errors as [`Self::send_text`] (group resolve,
    /// encode, MLS encrypt, transport, storage).
    pub async fn send_task(
        &self,
        draft: TaskDraft,
        with_fallback: Option<bool>,
    ) -> Result<MessageHandle, KupferdrahtError> {
        self.client
            .send_task(&self.room_id, draft, with_fallback)
            .await
    }
}

impl Timeline {
    /// Change one task's state (Step 60b, ADR-0014): reads the current
    /// folded state, applies the set-only `patch`, and sends the
    /// resulting **full state** as a new task message (LWW on the
    /// receiver side; `Done → Open` reopens). Fails with
    /// `InvalidInput` when `task_id` is unknown in this room.
    /// `with_fallback` follows [`Self::send_task`]'s contract —
    /// `None` derives the default from the room's task-list marker.
    ///
    /// # Errors
    /// Same downstream errors as [`Self::send_task`], plus the
    /// unknown-`task_id` case.
    pub async fn update_task(
        &self,
        task_id: &str,
        patch: TaskPatch,
        with_fallback: Option<bool>,
    ) -> Result<MessageHandle, KupferdrahtError> {
        self.client
            .update_task(&self.room_id, task_id, patch, with_fallback)
            .await
    }
}

impl Timeline {
    /// Move a task in this room's manual order (Step 60c, ADR-0014):
    /// computes a fractional `sort_key` strictly between the two
    /// neighbors and sends it as an ordinary full-state update — a
    /// move touches exactly one task. `after`/`before` name neighbor
    /// `task_id`s; `None` means the corresponding end of the list;
    /// **both `None` moves the task to the front of the manual
    /// order** — the bootstrap for a room whose tasks carry no keys
    /// yet. Manual ordering opts in per task: keyless tasks sort
    /// after all keyed ones, and a referenced neighbor must already
    /// carry a key. `with_fallback` follows [`Self::send_task`]'s
    /// contract — `None` derives the default from the room's
    /// task-list marker.
    ///
    /// # Errors
    /// `InvalidInput` for unknown task/neighbor ids, a keyless
    /// neighbor, or an impossible slot; otherwise the same errors as
    /// [`Self::update_task`].
    pub async fn move_task(
        &self,
        task_id: &str,
        after: Option<String>,
        before: Option<String>,
        with_fallback: Option<bool>,
    ) -> Result<MessageHandle, KupferdrahtError> {
        self.client
            .move_task(&self.room_id, task_id, after, before, with_fallback)
            .await
    }
}

impl Client {
    /// See [`Timeline::move_task`] for the caller-facing contract. Step 60c.
    pub(crate) async fn move_task(
        &self,
        room_id: &str,
        task_id: &str,
        after: Option<String>,
        before: Option<String>,
        with_fallback: Option<bool>,
    ) -> Result<MessageHandle, KupferdrahtError> {
        crate::client::input::require_non_empty("move_task", &[("task_id", task_id)])?;
        let tasks = self.room_tasks(room_id).await?;
        if !tasks.iter().any(|t| t.item.task_id == task_id) {
            return Err(KupferdrahtError::InvalidInput(format!(
                "move_task: unknown task_id {task_id} in {room_id}"
            )));
        }
        // Keyed tasks excluding the moving one, in key (= display)
        // order — manual ordering opts in per task, so a referenced
        // neighbor must already carry a sort_key.
        let keyed: Vec<(&str, &str)> = tasks
            .iter()
            .filter(|t| t.item.task_id != task_id)
            .filter_map(|t| {
                t.item
                    .sort_key
                    .as_deref()
                    .map(|k| (t.item.task_id.as_str(), k))
            })
            .collect();
        let (prev_key, next_key) =
            resolve_move_bounds(&keyed, after.as_deref(), before.as_deref())?;
        let new_key = fractional_between(prev_key, next_key)?;
        let patch = TaskPatch {
            sort_key: Some(new_key),
            ..TaskPatch::default()
        };
        self.update_task(room_id, task_id, patch, with_fallback)
            .await
    }

    /// See [`Timeline::update_task`] for the caller-facing contract. Step 60b.
    pub(crate) async fn update_task(
        &self,
        room_id: &str,
        task_id: &str,
        patch: TaskPatch,
        with_fallback: Option<bool>,
    ) -> Result<MessageHandle, KupferdrahtError> {
        crate::client::input::require_non_empty("update_task", &[("task_id", task_id)])?;
        let current = self
            .room_tasks(room_id)
            .await?
            .into_iter()
            .find(|t| t.item.task_id == task_id)
            .ok_or_else(|| {
                KupferdrahtError::InvalidInput(format!(
                    "update_task: unknown task_id {task_id} in {room_id}"
                ))
            })?;
        let updated = apply_task_patch(&current.item, &patch);
        if updated.title.trim().is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "update_task: title must be non-empty".into(),
            ));
        }
        self.send_task_state(room_id, &updated, with_fallback).await
    }

    /// Re-send every folded task's current state into the room — the
    /// shared invite-hook task batch (Step 75, ADR-0020 decision 2).
    /// ALL tasks, done and cancelled included (the board is an audit
    /// trail); always `Some(false)` — state transfer, not user
    /// speech. Best-effort per task: a failed send warns and the
    /// batch continues (existing members fold the re-sends as
    /// no-ops; the board self-heals on later activity).
    pub(crate) async fn resend_folded_task_states(
        &self,
        room_id: &str,
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(
            !room_id.is_empty(),
            "resend_folded_task_states: room_id must be non-empty"
        );
        for task in self.room_tasks(room_id).await? {
            if let Err(e) = self.send_task_state(room_id, &task.item, Some(false)).await {
                tracing::warn!(
                    room_id,
                    task_id = %task.item.task_id,
                    error = %e,
                    "invite: task state re-broadcast failed"
                );
            }
        }
        Ok(())
    }

    /// Resolve the human-readable mention handle for the fallback text:
    /// the room member's `handle`, else `display_name`, else the raw
    /// qualified id (ugly but unambiguous). Step 60a.
    async fn assignee_mention_handle(&self, room_id: &str, qualified_user_id: &str) -> String {
        let member =
            match self.room(room_id).await {
                Some(room) => room.members().await.ok().and_then(|members| {
                    members.into_iter().find(|m| m.user_id == qualified_user_id)
                }),
                None => None,
            };
        member
            .and_then(|m| m.handle.or(m.display_name))
            .unwrap_or_else(|| qualified_user_id.to_string())
    }

    /// See [`Timeline::send_task`] for the caller-facing contract. Step 60a.
    pub(crate) async fn send_task(
        &self,
        room_id: &str,
        draft: TaskDraft,
        with_fallback: Option<bool>,
    ) -> Result<MessageHandle, KupferdrahtError> {
        debug_assert!(!room_id.is_empty(), "send_task: room_id must be non-empty");
        // A task without a title is unrenderable everywhere — reject at
        // the consumer boundary (the ADR marks `title` required).
        if draft.title.trim().is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "send_task: title must be non-empty".into(),
            ));
        }
        let item = TaskItem {
            task_id: uuid::Uuid::new_v4().to_string(),
            title: draft.title,
            description: draft.description,
            assignee: draft.assignee,
            due_at_ms: draft.due_at_ms,
            status: TaskStatus::Open,
            priority: draft.priority,
            sort_key: None,
        };
        self.send_task_state(room_id, &item, with_fallback).await
    }

    /// Shared send tail for task create + update (Step 60b): encode the
    /// full state (mode c/a per `with_fallback`), MLS-encrypt + upload,
    /// persist the `kind='task'` row, emit the diff.
    async fn send_task_state(
        &self,
        room_id: &str,
        item: &TaskItem,
        with_fallback: Option<bool>,
    ) -> Result<MessageHandle, KupferdrahtError> {
        let group_id = self.resolve_room_group_id(room_id, "send_task").await?;
        // ADR-0020 decision 4: an explicit choice never consults the
        // marker fold — only `None` pays the extra read.
        let room_is_marked = match with_fallback {
            Some(_) => false,
            None => self.folded_task_list_marker(room_id).await?,
        };
        let with_fallback = resolve_task_fallback(with_fallback, room_is_marked);
        let (fallback_text, fallback_mentions) = if with_fallback {
            let assignee = match item.assignee.as_deref() {
                Some(qid) => Some((self.assignee_mention_handle(room_id, qid).await, qid)),
                None => None,
            };
            let (text, mentions) = task_fallback(
                &item.title,
                item.status,
                assignee
                    .as_ref()
                    .map(|(handle, qid)| (handle.as_str(), *qid)),
            );
            (Some(text), mentions)
        } else {
            (None, Vec::new())
        };
        let payload = payload_from_item(item);
        let fallback = fallback_text
            .as_deref()
            .map(|text| crate::wire::proto::TaskFallback {
                text,
                mentions: &fallback_mentions,
            });
        let (message_id, plaintext) =
            crate::wire::proto::build_task_generic_message(payload, fallback)?;
        // Step 97a: the row (and the fold cache) are written before the
        // upload; the row starts `Sending`.
        let _in_flight = self.begin_send(&message_id);
        let sent_at_ms = crate::time::now_ms();
        self.persist_outgoing_task(room_id, &message_id, item, sent_at_ms)
            .await?;
        let upload = self
            .encrypt_and_upload(&group_id, "send_task", &plaintext)
            .await;
        self.finish_send(room_id, &message_id, upload.is_ok(), None)
            .await;
        upload?;
        Ok(MessageHandle {
            message_id,
            sent_at_ms,
        })
    }

    /// Persist an outgoing task row as `Sending`, emit it, and maintain
    /// the materialized fold cache (Step 60c). Creator is
    /// first-writer-wins in the upsert, so passing self only sticks when
    /// this client materializes the task first (its own create, or an
    /// update to a task whose create it never saw — the best local
    /// knowledge either way).
    async fn persist_outgoing_task(
        &self,
        room_id: &str,
        message_id: &str,
        item: &TaskItem,
        sent_at_ms: i64,
    ) -> Result<(), KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(());
        };
        let event = self.outgoing_record(
            message_id,
            room_id,
            "task",
            &task_body(item),
            sent_at_ms,
            crate::timeline::DeliveryStatus::Sending,
        );
        let fold = task_record_from_folded(
            room_id,
            &FoldedTask {
                item: item.clone(),
                creator_user_id: self.own_qualified_user_id(),
                created_at_ms: sent_at_ms,
                updated_at_ms: sent_at_ms,
                latest_message_id: message_id.to_string(),
            },
        );
        // Step 105: the task row and its fold row are one write.
        if !store.insert_timeline_event_with_task(&event, &fold).await? {
            return Err(crate::timeline::pipeline::outgoing_row_exists(message_id));
        }
        self.dispatch_timeline_diff(&crate::timeline::TimelineDiff::Append {
            room_id: room_id.to_string(),
            item: crate::timeline::TimelineItem::from_record(event),
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_task_fallback;

    /// ADR-0020 decision 4: an explicit caller choice always wins,
    /// marker or no marker.
    #[test]
    fn explicit_fallback_choice_always_wins() {
        assert!(resolve_task_fallback(Some(true), true));
        assert!(resolve_task_fallback(Some(true), false));
        assert!(!resolve_task_fallback(Some(false), true));
        assert!(!resolve_task_fallback(Some(false), false));
    }

    /// ADR-0020 decision 4: `None` derives from the room's marker —
    /// a marked room suppresses the fallback, an unmarked room keeps
    /// the pre-Step-75 default.
    #[test]
    fn absent_choice_derives_from_the_marker() {
        assert!(
            !resolve_task_fallback(None, true),
            "marked room must default to no fallback"
        );
        assert!(
            resolve_task_fallback(None, false),
            "unmarked room keeps the fallback default"
        );
    }
}
