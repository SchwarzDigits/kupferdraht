// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Tasks / todos (`TaskPayload`, `GenericMessage` field 262000) —
//! Step 60a, ADR-0014.
//!
//! A task is event-sourced: every task message carries the **full
//! state** of one task; receivers fold in event-stream server order
//! (LWW per `task_id` — folding lands in Step 60b). The payload is
//! persisted as a JSON `timeline_events.body` of kind `task`
//! (the `Location` small-payload precedent).
//!
//! Binding rules: `rules/0014-tasks-as-event-sourced-message-content.md`.

mod cache;
mod encode;
mod fold;
mod marker;
mod order;
mod send;
mod types;
mod views;

pub use types::{FoldedTask, RoomTask, TaskDraft, TaskItem, TaskPatch, TaskPriority, TaskStatus};

pub(crate) use cache::*;
pub(crate) use encode::*;
pub(crate) use fold::*;
pub(crate) use marker::*;
pub(crate) use order::*;
pub(crate) use views::*;

// Shared imports re-exported into the submodules' `use super::*`.
pub(crate) use crate::client::Client;
pub(crate) use crate::error::KupferdrahtError;
pub(crate) use crate::storage::{TaskRecord, TimelineEventRecord};
pub(crate) use crate::timeline::{MessageHandle, Timeline, TimelineItemKind};
pub(crate) use crate::wire::proto::MentionInput;

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_item() -> TaskItem {
        TaskItem {
            task_id: "7f9c0e5a-1111-4222-8333-444455556666".into(),
            title: "Prepare deploy".into(),
            description: Some("Ship it — see https://example.com/runbook".into()),
            assignee: Some("aaaa-bbbb@wire.com".into()),
            due_at_ms: Some(1_752_000_000_000),
            status: TaskStatus::InProgress,
            priority: Some(TaskPriority::High),
            sort_key: Some("m".into()),
        }
    }

    /// UTF-16 code-unit offset of `needle` inside `haystack` —
    /// independently computed so the test doesn't trust the
    /// implementation's own conversion.
    fn utf16_offset(haystack: &str, needle: &str) -> i32 {
        let byte_pos = haystack.find(needle).expect("needle present");
        i32::try_from(haystack[..byte_pos].encode_utf16().count()).expect("fits i32")
    }

    #[test]
    fn task_fallback_renders_title_and_utf16_assignee_mention() {
        let (text, mentions) = task_fallback(
            "Prepare deploy",
            TaskStatus::Open,
            Some(("@anna", "aaaa-bbbb@wire.com")),
        );
        assert!(
            text.contains("Prepare deploy"),
            "fallback must carry the title: {text}"
        );
        assert!(
            text.contains("@anna"),
            "fallback must carry the assignee handle: {text}"
        );
        assert_eq!(mentions.len(), 1, "exactly one mention for the assignee");
        let m = &mentions[0];
        // Offsets are UTF-16 code units, not bytes — the leading `☐`
        // (3 UTF-8 bytes, 1 UTF-16 unit) makes the two diverge.
        assert_eq!(m.start, utf16_offset(&text, "@anna"), "mention start");
        assert_eq!(
            m.length,
            i32::try_from("@anna".encode_utf16().count()).expect("fits"),
            "mention length"
        );
        assert_eq!(m.qualified_user_id, "aaaa-bbbb@wire.com");
    }

    #[test]
    fn task_fallback_without_assignee_has_no_mentions() {
        let (text, mentions) = task_fallback("Buy milk", TaskStatus::Open, None);
        assert!(text.contains("Buy milk"));
        assert!(mentions.is_empty(), "no assignee → no mention");
    }

    #[test]
    fn task_body_round_trips_through_task_kind() {
        let item = sample_item();
        let body = task_body(&item);
        match task_kind_from_body(&body) {
            Some(TimelineItemKind::Task(parsed)) => assert_eq!(*parsed, item),
            other => panic!("expected Task kind, got {other:?}"),
        }
    }

    #[test]
    fn task_body_round_trips_minimal_item() {
        let item = TaskItem {
            task_id: "id-1".into(),
            title: "Buy milk".into(),
            description: None,
            assignee: None,
            due_at_ms: None,
            status: TaskStatus::Open,
            priority: None,
            sort_key: None,
        };
        let body = task_body(&item);
        match task_kind_from_body(&body) {
            Some(TimelineItemKind::Task(parsed)) => assert_eq!(*parsed, item),
            other => panic!("expected Task kind, got {other:?}"),
        }
    }

    #[test]
    fn task_kind_from_body_maps_unknown_status_to_open() {
        // A newer Kupferdraht may write statuses this build doesn't know.
        // ADR-0014: unknown status → Open (visible; next update heals).
        let body = r#"{"task_id":"id-2","title":"From the future","description":null,"assignee":null,"due_at_ms":null,"status":"blocked","priority":null,"sort_key":null}"#;
        match task_kind_from_body(body) {
            Some(TimelineItemKind::Task(parsed)) => {
                assert_eq!(parsed.status, TaskStatus::Open, "unknown status → Open");
                assert_eq!(parsed.title, "From the future");
            }
            other => panic!("expected Task kind, got {other:?}"),
        }
    }

    #[test]
    fn task_kind_from_body_rejects_malformed_json() {
        assert_eq!(task_kind_from_body("not json at all"), None);
        assert_eq!(task_kind_from_body("{\"title\":\"half\""), None);
    }

    // ── Step 60b — folding / patching / ordering ────────────────────

    /// A `timeline_events` row carrying `item` as a task message.
    /// Slice order in the fold tests == arrival order.
    fn record_for(
        item: &TaskItem,
        sender: &str,
        sent_at_ms: i64,
        message_id: &str,
    ) -> crate::storage::TimelineEventRecord {
        crate::storage::TimelineEventRecord {
            message_id: message_id.into(),
            room_id: "room@wire.com".into(),
            sender_user_id: sender.into(),
            sender_device_id: "dev".into(),
            kind: "task".into(),
            body: task_body(item),
            sent_at_ms,
            delivery_status: "received".into(),
            reply_to_id: None,
            edited_at_ms: None,
            redacted_at_ms: None,
            expires_at_ms: None,
            ephemeral_ttl_ms: None,
        }
    }

    fn item_with(task_id: &str, title: &str, status: TaskStatus) -> TaskItem {
        TaskItem {
            task_id: task_id.into(),
            title: title.into(),
            description: None,
            assignee: None,
            due_at_ms: None,
            status,
            priority: None,
            sort_key: None,
        }
    }

    #[test]
    fn apply_task_patch_updates_only_set_fields() {
        let current = sample_item();
        let patch = TaskPatch {
            status: Some(TaskStatus::Done),
            due_at_ms: Some(1_755_000_000_000),
            ..TaskPatch::default()
        };
        let updated = apply_task_patch(&current, &patch);
        assert_eq!(updated.status, TaskStatus::Done);
        assert_eq!(updated.due_at_ms, Some(1_755_000_000_000));
        // Everything not in the patch stays byte-identical.
        assert_eq!(updated.task_id, current.task_id);
        assert_eq!(updated.title, current.title);
        assert_eq!(updated.description, current.description);
        assert_eq!(updated.assignee, current.assignee);
        assert_eq!(updated.priority, current.priority);
        assert_eq!(updated.sort_key, current.sort_key);
    }

    #[test]
    fn apply_task_patch_allows_any_status_transition() {
        // ADR-0014: no state machine — reopen (`Done → Open`) and any
        // other transition are ordinary full-state updates.
        let all = [
            TaskStatus::Open,
            TaskStatus::InProgress,
            TaskStatus::Done,
            TaskStatus::Cancelled,
        ];
        for from in all {
            for to in all {
                let mut current = sample_item();
                current.status = from;
                let updated = apply_task_patch(
                    &current,
                    &TaskPatch {
                        status: Some(to),
                        ..TaskPatch::default()
                    },
                );
                assert_eq!(updated.status, to, "{from:?} -> {to:?} must be legal");
            }
        }
    }

    #[test]
    fn fold_later_full_state_wins_entirely() {
        // Full-state LWW: the later message replaces the task wholesale
        // — fields absent in the later state must NOT bleed through
        // from the earlier one (ADR-0014).
        let v1 = sample_item(); // description, assignee, due, priority all set
        let mut v2 = item_with(
            &v1.task_id,
            "Prepare deploy (renamed)",
            TaskStatus::InProgress,
        );
        v2.sort_key = Some("m".into());
        let rows = vec![
            record_for(&v1, "alice@wire.com", 1000, "m1"),
            record_for(&v2, "bob@wire.com", 2000, "m2"),
        ];
        let folded = fold_task_rows(&rows);
        assert_eq!(folded.len(), 1);
        let f = &folded[0];
        assert_eq!(f.item, v2, "later full state wins entirely, no merge");
        assert_eq!(
            f.creator_user_id, "alice@wire.com",
            "creator = first sender"
        );
        assert_eq!(f.created_at_ms, 1000);
        assert_eq!(f.updated_at_ms, 2000);
        assert_eq!(f.latest_message_id, "m2");
    }

    #[test]
    fn fold_upserts_task_from_update_only_stream() {
        // A late joiner may see only an update — it must still create
        // local state (upsert, ADR-0014).
        let update = item_with("tid-up", "Seen only as update", TaskStatus::Done);
        let rows = vec![record_for(&update, "alice@wire.com", 5000, "m9")];
        let folded = fold_task_rows(&rows);
        assert_eq!(folded.len(), 1);
        assert_eq!(folded[0].item, update);
    }

    #[test]
    fn fold_falls_back_when_latest_row_is_redacted() {
        // A MessageDelete wipes the row's body — the deleted message
        // drops out of the fold and the prior state resurfaces.
        let v1 = item_with("tid-del", "Original", TaskStatus::Open);
        let v2 = item_with("tid-del", "Renamed", TaskStatus::InProgress);
        let mut deleted = record_for(&v2, "alice@wire.com", 2000, "m2");
        deleted.body = String::new();
        deleted.redacted_at_ms = Some(2500);
        let rows = vec![record_for(&v1, "alice@wire.com", 1000, "m1"), deleted];
        let folded = fold_task_rows(&rows);
        assert_eq!(folded.len(), 1);
        assert_eq!(folded[0].item, v1, "redacted latest → prior state");
        assert_eq!(folded[0].latest_message_id, "m1");
    }

    #[test]
    fn fold_drops_task_whose_only_row_is_redacted() {
        let v1 = item_with("tid-gone", "To be deleted", TaskStatus::Open);
        let mut deleted = record_for(&v1, "alice@wire.com", 1000, "m1");
        deleted.body = String::new();
        deleted.redacted_at_ms = Some(1500);
        assert!(fold_task_rows(&[deleted]).is_empty());
    }

    /// Step 95c: before 95c an inbound task with an empty `task_id` was
    /// stored as a timeline row before the task-cache upsert panicked —
    /// existing databases can hold such rows. The fold drops them, so a
    /// cache rebuild or an invite re-broadcast never feeds an empty id
    /// to the send-side builder (a release-build `assert!`).
    #[test]
    fn fold_drops_rows_with_an_empty_task_id() {
        let good = item_with("tid-ok", "Survives", TaskStatus::Open);
        let empty = item_with("", "No id", TaskStatus::Open);
        let rows = vec![
            record_for(&empty, "alice@wire.com", 1000, "m1"),
            record_for(&good, "alice@wire.com", 2000, "m2"),
        ];

        let folded = fold_task_rows(&rows);

        assert_eq!(folded.len(), 1);
        assert_eq!(folded[0].item.task_id, "tid-ok");
    }

    #[test]
    fn fold_skips_malformed_and_non_task_rows() {
        let v1 = item_with("tid-ok", "Survives", TaskStatus::Open);
        let mut garbled = record_for(&v1, "alice@wire.com", 2000, "m2");
        garbled.body = "{not json".into();
        let mut text_row = record_for(&v1, "alice@wire.com", 3000, "m3");
        text_row.kind = "text".into();
        text_row.body = "just a chat message".into();
        let rows = vec![
            record_for(&v1, "alice@wire.com", 1000, "m1"),
            garbled,
            text_row,
        ];
        let folded = fold_task_rows(&rows);
        assert_eq!(folded.len(), 1);
        assert_eq!(folded[0].item, v1);
        assert_eq!(folded[0].latest_message_id, "m1");
    }

    #[test]
    fn fold_handles_multiple_tasks_independently() {
        let a1 = item_with("tid-a", "A v1", TaskStatus::Open);
        let b1 = item_with("tid-b", "B v1", TaskStatus::Open);
        let a2 = item_with("tid-a", "A v2", TaskStatus::Done);
        let rows = vec![
            record_for(&a1, "alice@wire.com", 1000, "m1"),
            record_for(&b1, "bob@wire.com", 2000, "m2"),
            record_for(&a2, "alice@wire.com", 3000, "m3"),
        ];
        let folded = fold_task_rows(&rows);
        assert_eq!(folded.len(), 2);
        let a = folded
            .iter()
            .find(|f| f.item.task_id == "tid-a")
            .expect("a");
        let b = folded
            .iter()
            .find(|f| f.item.task_id == "tid-b")
            .expect("b");
        assert_eq!(a.item, a2);
        assert_eq!(b.item, b1);
    }

    #[test]
    fn fold_ignores_exact_redelivery() {
        // Redelivery of the same message must not change the fold
        // (the DB's UNIQUE(message_id) already dedupes; the fold
        // itself must also be stable under duplicates).
        let v1 = item_with("tid-r", "Once", TaskStatus::Open);
        let row = record_for(&v1, "alice@wire.com", 1000, "m1");
        let once = fold_task_rows(std::slice::from_ref(&row));
        let twice = fold_task_rows(&[row.clone(), row]);
        assert_eq!(once, twice);
    }

    #[test]
    fn sort_orders_due_then_priority_then_created() {
        fn folded(
            task_id: &str,
            due: Option<i64>,
            prio: Option<TaskPriority>,
            created: i64,
        ) -> FoldedTask {
            let mut item = item_with(task_id, task_id, TaskStatus::Open);
            item.due_at_ms = due;
            item.priority = prio;
            FoldedTask {
                item,
                creator_user_id: "alice@wire.com".into(),
                created_at_ms: created,
                updated_at_ms: created,
                latest_message_id: format!("m-{task_id}"),
            }
        }
        let mut tasks = vec![
            folded("no-due-low", None, Some(TaskPriority::Low), 1),
            folded("due-late", Some(9000), None, 2),
            folded("no-due-urgent", None, Some(TaskPriority::Urgent), 3),
            folded("due-early", Some(1000), None, 4),
            folded("no-due-none-old", None, None, 5),
            folded("no-due-none-new", None, None, 6),
        ];
        sort_folded_tasks(&mut tasks);
        let order: Vec<&str> = tasks.iter().map(|f| f.item.task_id.as_str()).collect();
        assert_eq!(
            order,
            vec![
                "due-early",     // earliest deadline first
                "due-late",      // deadlines before no-deadline
                "no-due-urgent", // then priority descending
                "no-due-low",
                "no-due-none-old", // then first-seen ascending
                "no-due-none-new",
            ]
        );
    }
    // ── Step 60c — fractional indexing / cross-room views ───────────

    fn folded_with_key(task_id: &str, sort_key: Option<&str>, created: i64) -> FoldedTask {
        let mut item = item_with(task_id, task_id, TaskStatus::Open);
        item.sort_key = sort_key.map(str::to_string);
        FoldedTask {
            item,
            creator_user_id: "alice@wire.com".into(),
            created_at_ms: created,
            updated_at_ms: created,
            latest_message_id: format!("m-{task_id}"),
        }
    }

    fn room_task(
        room_id: &str,
        task_id: &str,
        creator: &str,
        assignee: Option<&str>,
        status: TaskStatus,
    ) -> RoomTask {
        let mut item = item_with(task_id, task_id, status);
        item.assignee = assignee.map(str::to_string);
        RoomTask {
            room_id: room_id.into(),
            task: FoldedTask {
                item,
                creator_user_id: creator.into(),
                created_at_ms: 1000,
                updated_at_ms: 1000,
                latest_message_id: format!("m-{task_id}"),
            },
        }
    }

    #[test]
    fn fractional_between_seeds_a_key_for_the_empty_list() {
        let key = fractional_between(None, None).expect("seed key");
        assert!(!key.is_empty());
        assert!(
            key.chars().all(|c| c.is_ascii_lowercase()),
            "keys are lowercase a-z: {key}"
        );
    }

    #[test]
    fn fractional_between_orders_strictly() {
        let cases: [(Option<&str>, Option<&str>); 4] = [
            (None, Some("m")),
            (Some("m"), None),
            (Some("a"), Some("b")),
            (Some("ab"), Some("ac")),
        ];
        for (prev, next) in cases {
            let key = fractional_between(prev, next).expect("key");
            if let Some(p) = prev {
                assert!(p < key.as_str(), "{p} < {key} must hold");
            }
            if let Some(n) = next {
                assert!(key.as_str() < n, "{key} < {n} must hold");
            }
        }
    }

    #[test]
    fn fractional_between_chain_insertions_stay_ordered() {
        // Repeatedly squeeze a new task in at the same spot — the
        // renumbering-cascade failure mode fractional indexing exists
        // to avoid (ADR-0014). Keys must stay strictly ordered.
        let anchor = fractional_between(None, None).expect("anchor");
        let mut upper = fractional_between(Some(&anchor), None).expect("upper");
        let mut keys = vec![anchor.clone(), upper.clone()];
        for _ in 0..20 {
            let mid = fractional_between(Some(&anchor), Some(&upper)).expect("mid");
            assert!(anchor < mid && mid < upper, "{anchor} < {mid} < {upper}");
            keys.push(mid.clone());
            upper = mid;
        }
        let mut sorted = keys.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), keys.len(), "no key collisions");
    }

    #[test]
    fn fractional_between_never_emits_trailing_minimum() {
        // A trailing 'a' leaves no room before the key — the
        // invariant every fractional-indexing scheme needs.
        let cases: [(Option<&str>, Option<&str>); 3] =
            [(None, None), (None, Some("b")), (Some("a"), Some("b"))];
        for (prev, next) in cases {
            let key = fractional_between(prev, next).expect("key");
            assert!(
                !key.ends_with('a'),
                "key must not end with the minimal digit: {key}"
            );
        }
    }

    #[test]
    fn fractional_between_rejects_inverted_neighbors() {
        assert!(fractional_between(Some("m"), Some("m")).is_err());
        assert!(fractional_between(Some("n"), Some("m")).is_err());
    }

    #[test]
    fn sort_key_takes_precedence_over_default_order() {
        // Manual order opts in per task: keyed tasks come first in key
        // order; keyless ones follow in the default (due/prio/created)
        // order. A keyed task without due/prio still precedes a
        // keyless urgent one.
        let mut keyed_late = folded_with_key("keyed-z", Some("z"), 1);
        keyed_late.item.priority = None;
        let keyed_early = folded_with_key("keyed-b", Some("b"), 2);
        let mut keyless_urgent = folded_with_key("keyless-urgent", None, 3);
        keyless_urgent.item.priority = Some(TaskPriority::Urgent);
        let keyless_plain = folded_with_key("keyless-plain", None, 4);
        let mut tasks = vec![keyless_urgent, keyed_late, keyless_plain, keyed_early];
        sort_folded_tasks(&mut tasks);
        let order: Vec<&str> = tasks.iter().map(|f| f.item.task_id.as_str()).collect();
        assert_eq!(
            order,
            vec!["keyed-b", "keyed-z", "keyless-urgent", "keyless-plain"]
        );
    }

    #[test]
    fn filter_my_tasks_returns_open_work_across_rooms() {
        let me = "me@wire.com";
        let tasks = vec![
            room_task(
                "room-1@wire.com",
                "t-open",
                "boss@wire.com",
                Some(me),
                TaskStatus::Open,
            ),
            room_task(
                "room-2@wire.com",
                "t-progress",
                me,
                Some(me),
                TaskStatus::InProgress,
            ),
        ];
        let mine = filter_my_tasks(&tasks, me);
        let ids: Vec<&str> = mine.iter().map(|t| t.task.item.task_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["t-open", "t-progress"],
            "both rooms, both open states"
        );
    }

    #[test]
    fn filter_my_tasks_excludes_done_cancelled_and_foreign() {
        let me = "me@wire.com";
        let tasks = vec![
            room_task(
                "r@wire.com",
                "t-done",
                "boss@wire.com",
                Some(me),
                TaskStatus::Done,
            ),
            room_task(
                "r@wire.com",
                "t-cancelled",
                "boss@wire.com",
                Some(me),
                TaskStatus::Cancelled,
            ),
            room_task(
                "r@wire.com",
                "t-other",
                "boss@wire.com",
                Some("other@wire.com"),
                TaskStatus::Open,
            ),
            room_task(
                "r@wire.com",
                "t-unassigned",
                "boss@wire.com",
                None,
                TaskStatus::Open,
            ),
        ];
        assert!(filter_my_tasks(&tasks, me).is_empty());
    }

    #[test]
    fn filter_delegated_returns_others_assigned_any_status() {
        let me = "me@wire.com";
        let tasks = vec![
            room_task(
                "r1@wire.com",
                "d-open",
                me,
                Some("anna@wire.com"),
                TaskStatus::Open,
            ),
            room_task(
                "r2@wire.com",
                "d-done",
                me,
                Some("bob@wire.com"),
                TaskStatus::Done,
            ),
        ];
        let delegated = filter_delegated_tasks(&tasks, me);
        let ids: Vec<&str> = delegated
            .iter()
            .map(|t| t.task.item.task_id.as_str())
            .collect();
        assert_eq!(
            ids,
            vec!["d-open", "d-done"],
            "done stays visible — the creator watches progress"
        );
    }

    #[test]
    fn filter_delegated_excludes_self_assigned_and_unassigned() {
        let me = "me@wire.com";
        let tasks = vec![
            room_task(
                "r@wire.com",
                "self-assigned",
                me,
                Some(me),
                TaskStatus::Open,
            ),
            room_task("r@wire.com", "unassigned", me, None, TaskStatus::Open),
            room_task(
                "r@wire.com",
                "foreign",
                "boss@wire.com",
                Some("anna@wire.com"),
                TaskStatus::Open,
            ),
        ];
        assert!(filter_delegated_tasks(&tasks, me).is_empty());
    }

    #[test]
    fn resolve_move_bounds_bootstraps_the_empty_keyed_list() {
        let bounds = resolve_move_bounds(&[], None, None).expect("bootstrap");
        assert_eq!(bounds, (None, None), "first move seeds a fresh key");
    }

    #[test]
    fn resolve_move_bounds_no_neighbors_means_front() {
        let keyed = [("t1", "f"), ("t2", "p")];
        let bounds = resolve_move_bounds(&keyed, None, None).expect("front");
        assert_eq!(bounds, (None, Some("f")), "bare move goes before all keyed");
    }

    #[test]
    fn resolve_move_bounds_after_uses_next_neighbor_as_upper() {
        let keyed = [("t1", "f"), ("t2", "p")];
        assert_eq!(
            resolve_move_bounds(&keyed, Some("t1"), None).expect("after"),
            (Some("f"), Some("p"))
        );
        assert_eq!(
            resolve_move_bounds(&keyed, Some("t2"), None).expect("after last"),
            (Some("p"), None)
        );
    }

    #[test]
    fn resolve_move_bounds_before_uses_prev_neighbor_as_lower() {
        let keyed = [("t1", "f"), ("t2", "p")];
        assert_eq!(
            resolve_move_bounds(&keyed, None, Some("t2")).expect("before"),
            (Some("f"), Some("p"))
        );
        assert_eq!(
            resolve_move_bounds(&keyed, None, Some("t1")).expect("before first"),
            (None, Some("f"))
        );
    }

    #[test]
    fn resolve_move_bounds_rejects_unknown_or_keyless_neighbor() {
        let keyed = [("t1", "f")];
        assert!(resolve_move_bounds(&keyed, Some("ghost"), None).is_err());
        assert!(resolve_move_bounds(&keyed, None, Some("ghost")).is_err());
    }

    #[test]
    fn task_record_round_trips_through_folded() {
        let folded = FoldedTask {
            item: TaskItem {
                task_id: "t-1".into(),
                title: "Prepare deploy".into(),
                description: Some("staging first".into()),
                assignee: Some("anna@wire.com".into()),
                due_at_ms: Some(1_700_000_000_000),
                status: TaskStatus::InProgress,
                priority: Some(TaskPriority::High),
                sort_key: Some("hn".into()),
            },
            creator_user_id: "boss@wire.com".into(),
            created_at_ms: 1000,
            updated_at_ms: 2000,
            latest_message_id: "m-2".into(),
        };
        let rec = task_record_from_folded("r@wire.com", &folded);
        let back = folded_from_task_record(rec);
        assert_eq!(back.room_id, "r@wire.com");
        assert_eq!(back.task, folded, "cache write → read must be lossless");
    }

    #[test]
    fn task_record_rehydration_tolerates_unknown_labels() {
        let mut rec = task_record_from_folded("r@wire.com", &folded_with_key("t-1", None, 1000));
        rec.status = "blocked".into();
        rec.priority = Some("asap".into());
        let back = folded_from_task_record(rec);
        assert_eq!(back.task.item.status, TaskStatus::Open);
        assert_eq!(back.task.item.priority, None);
    }
}
