// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Task ordering (Step 60c): fractional `sort_key` computation
//! (`LexoRank` principle), move-bound resolution, and the ADR-0014
//! sort precedence (keyed, then due, then priority, then first-seen).
use super::{FoldedTask, KupferdrahtError, RoomTask, TaskPriority};

/// Compute a fractional index strictly between two neighbors
/// (`LexoRank` principle, ADR-0014): keys are lowercase `a`–`z`
/// strings compared lexicographically; a manual move touches exactly
/// one task. `None` bounds mean "before everything" / "after
/// everything". Generated keys never end in `a` (the minimal digit —
/// a trailing `a` would leave no room before the key). Step 60c.
///
/// # Errors
/// `InvalidInput` when `prev >= next` (inverted or colliding
/// neighbors — the caller passed an impossible slot).
pub(crate) fn fractional_between(
    prev: Option<&str>,
    next: Option<&str>,
) -> Result<String, KupferdrahtError> {
    const BASE: u32 = 26;
    fn validate(key: &str, side: &str) -> Result<(), KupferdrahtError> {
        if key.is_empty() || !key.bytes().all(|b| b.is_ascii_lowercase()) {
            return Err(KupferdrahtError::InvalidInput(format!(
                "fractional_between: {side} key must be non-empty lowercase a-z, got {key:?}"
            )));
        }
        Ok(())
    }
    fn digit(key: Option<&str>, i: usize, default: u32) -> u32 {
        key.and_then(|k| k.as_bytes().get(i))
            .map_or(default, |b| u32::from(b - b'a'))
    }
    if let Some(p) = prev {
        validate(p, "prev")?;
    }
    if let Some(n) = next {
        validate(n, "next")?;
    }
    if let (Some(p), Some(n)) = (prev, next)
        && p >= n
    {
        return Err(KupferdrahtError::InvalidInput(format!(
            "fractional_between: prev {p:?} must sort strictly before next {n:?}"
        )));
    }
    // Walk digit positions. A bound stays "tight" while the key built
    // so far equals its prefix; once we emit a digit below the next
    // bound's digit, everything deeper is unconstrained on that side.
    let mut key = String::new();
    let mut prev_tight = true;
    let mut next_tight = true;
    for i in 0.. {
        let p = if prev_tight { digit(prev, i, 0) } else { 0 };
        let n = if next_tight {
            digit(next, i, BASE)
        } else {
            BASE
        };
        if next_tight && next.is_some_and(|k| i >= k.len()) {
            // The key so far equals `next` — there is no room before a
            // key that our own generator would never emit (an all-`a`
            // extension). Only reachable with foreign keys.
            return Err(KupferdrahtError::InvalidInput(format!(
                "fractional_between: no room before {:?} at depth {i}",
                next.unwrap_or_default()
            )));
        }
        if n - p > 1 {
            // Strictly between: p < mid < n, so the key never ends in
            // the minimal digit `a` (mid >= p + 1 >= 1).
            let mid = p + (n - p) / 2;
            key.push(char::from(b'a' + u8::try_from(mid).unwrap_or(13)));
            debug_assert!(!key.ends_with('a'), "generated key must not end in 'a'");
            return Ok(key);
        }
        // Digits equal (n == p) or adjacent (n == p + 1): copy the
        // lower digit and descend one position deeper.
        key.push(char::from(b'a' + u8::try_from(p).unwrap_or(0)));
        if prev_tight && prev.is_some_and(|k| i >= k.len()) {
            prev_tight = false;
        }
        if p < n {
            next_tight = false;
        }
    }
    unreachable!("fractional_between: the digit walk always terminates")
}

/// Resolve a move's fractional bounds from its named neighbors
/// (Step 60c). `keyed` is the room's keyed tasks minus the moving
/// one, in key order. `after`/`before` must name keyed tasks; **both
/// absent means "move to the front of the manual order"** — with an
/// empty keyed list this seeds the room's first `sort_key` (the
/// bootstrap: without it manual ordering could never start, since a
/// referenced neighbor must already carry a key).
///
/// # Errors
/// `InvalidInput` when a named neighbor is unknown or keyless.
pub(crate) fn resolve_move_bounds<'a>(
    keyed: &'a [(&'a str, &'a str)],
    after: Option<&str>,
    before: Option<&str>,
) -> Result<(Option<&'a str>, Option<&'a str>), KupferdrahtError> {
    let position_of = |neighbor: &str| -> Result<usize, KupferdrahtError> {
        keyed
            .iter()
            .position(|(id, _)| *id == neighbor)
            .ok_or_else(|| {
                KupferdrahtError::InvalidInput(format!(
                    "move_task: neighbor {neighbor} is unknown or has no sort_key yet — \
                     move it first or reference a keyed task"
                ))
            })
    };
    Ok(match (after, before) {
        (Some(a), Some(b)) => (
            Some(keyed[position_of(a)?].1),
            Some(keyed[position_of(b)?].1),
        ),
        (Some(a), None) => {
            let i = position_of(a)?;
            (Some(keyed[i].1), keyed.get(i + 1).map(|(_, k)| *k))
        }
        (None, Some(b)) => {
            let i = position_of(b)?;
            (
                i.checked_sub(1).and_then(|p| keyed.get(p)).map(|(_, k)| *k),
                Some(keyed[i].1),
            )
        }
        (None, None) => (None, keyed.first().map(|(_, k)| *k)),
    })
}

/// Ordering (ADR-0014): tasks carrying a `sort_key` come first, in
/// lexicographic key order (the manual order opts in per task via a
/// move — Step 60c); tasks without a key follow in the default
/// order — due date ascending (deadlines before none), then priority
/// descending (`Urgent` first, none last), then first-seen
/// (`created_at_ms`) ascending.
fn priority_rank(priority: Option<TaskPriority>) -> u8 {
    match priority {
        Some(TaskPriority::Urgent) => 0,
        Some(TaskPriority::High) => 1,
        Some(TaskPriority::Medium) => 2,
        Some(TaskPriority::Low) => 3,
        None => 4,
    }
}

fn default_task_order(a: &FoldedTask, b: &FoldedTask) -> std::cmp::Ordering {
    let key = |f: &FoldedTask| {
        (
            f.item.due_at_ms.is_none(),           // deadlines first
            f.item.due_at_ms.unwrap_or(i64::MAX), // earliest deadline first
            priority_rank(f.item.priority),       // then priority descending
            f.created_at_ms,                      // then first-seen ascending
        )
    };
    key(a).cmp(&key(b))
}

fn compare_tasks(a: &FoldedTask, b: &FoldedTask) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (&a.item.sort_key, &b.item.sort_key) {
        (Some(x), Some(y)) => x.cmp(y).then_with(|| default_task_order(a, b)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => default_task_order(a, b),
    }
}

pub(crate) fn sort_folded_tasks(tasks: &mut [FoldedTask]) {
    tasks.sort_by(compare_tasks);
}

/// Same ordering over the cross-room view elements. Step 60c.
pub(crate) fn sort_room_tasks(tasks: &mut [RoomTask]) {
    tasks.sort_by(|a, b| compare_tasks(&a.task, &b.task));
}
