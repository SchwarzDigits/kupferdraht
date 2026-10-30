// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Shared session / room plumbing reused by every CLI command domain.

use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::Arc;

use kupferdraht::keystore::{FileKeyStore, KeyringStore, SecureKeyStore};
use kupferdraht::storage::Store;
use kupferdraht::{Kupferdraht, KupferdrahtError, Room, SyncSettings};

pub(crate) mod access;
pub(crate) mod assets;
pub(crate) mod auth;
pub(crate) mod case;
pub(crate) mod config;
pub(crate) mod devices;
pub(crate) mod resend;
pub(crate) mod rooms;
pub(crate) mod sync;
pub(crate) mod task;
pub(crate) mod timeline;
pub(crate) fn kupferdraht_home() -> Result<PathBuf, KupferdrahtError> {
    if let Ok(home) = std::env::var("KUPFERDRAHT_HOME") {
        return Ok(PathBuf::from(home));
    }
    let base = dirs::data_local_dir()
        .ok_or_else(|| KupferdrahtError::Environment("no data_local_dir available".into()))?;
    Ok(base.join("kupferdraht"))
}

pub(crate) fn read_password(from_stdin: bool) -> io::Result<String> {
    if from_stdin {
        let mut buf = String::new();
        io::stdin().read_to_string(&mut buf)?;
        Ok(buf.trim_end_matches(['\n', '\r']).to_string())
    } else {
        rpassword::prompt_password("Account password: ")
    }
}

pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The device keystore the CLI hands to [`Kupferdraht`]: the OS keyring in
/// production, or a file-backed store when `KUPFERDRAHT_DEV_KEYSTORE` points
/// at a directory (mirrors the SDK's internal default for the
/// no-`permit_http` path). The file store announces itself with a
/// `WARN` on stderr (the default `RUST_LOG` filter is `warn`) — plaintext
/// key files must never be silent (Step 77).
pub(crate) fn cli_keystore() -> Arc<dyn SecureKeyStore> {
    if let Ok(dir) = std::env::var("KUPFERDRAHT_DEV_KEYSTORE") {
        return Arc::new(FileKeyStore::new(PathBuf::from(dir)));
    }
    Arc::new(KeyringStore::new("kupferdraht"))
}

pub(crate) async fn resume_client() -> Result<Arc<kupferdraht::Client>, KupferdrahtError> {
    let (client, _store) = resume_client_and_store().await?;
    Ok(client)
}

/// Open the device manager on the CLI's home with the CLI's keystore —
/// the one way every command reaches `Kupferdraht` (Step 93: the manager
/// resolves the home id and the cookie label the moment it opens).
pub(crate) fn open_manager() -> Result<Kupferdraht, KupferdrahtError> {
    Kupferdraht::open(kupferdraht_home()?, cli_keystore())
}

/// Resume the active account through `Kupferdraht` — the device `GlobalStore`
/// registry picks which account (`active_account`) — and hand the caller
/// back the SDK-opened store via `Client::store()`. Any subsequent direct
/// `store.*` call (`save_session`, `load_rooms`, `load_timeline_events`, …)
/// MUST go through this same handle so it sees the live, keyed connection.
pub(crate) async fn resume_client_and_store()
-> Result<(Arc<kupferdraht::Client>, Arc<dyn Store>), KupferdrahtError> {
    let app = open_manager()?;
    resume_active(&app).await
}

/// The resume half of [`resume_client_and_store`], on an already-open
/// manager — for commands that also read manager-level state (`status`).
pub(crate) async fn resume_active(
    app: &Kupferdraht,
) -> Result<(Arc<kupferdraht::Client>, Arc<dyn Store>), KupferdrahtError> {
    let account_id = app.active_account().await?.ok_or_else(|| {
        KupferdrahtError::InvalidInput(
            "no persisted account — run `kupferdraht-cli login` first".into(),
        )
    })?;
    let client = app.resume(&account_id).await?;
    client.load_rooms_from_store().await?;
    let store = client
        .store()
        .ok_or_else(|| KupferdrahtError::Storage("resume: default store missing".into()))?;
    Ok((client, store))
}

/// Standard prefix for room-targeted CLI commands: resume the client,
/// run `sync_metadata`, look up the requested room. Surfaces the same
/// "room not in cache" error every per-room handler used to inline.
/// `room_id` is echoed back in the return tuple so callers can print
/// it without keeping their own copy around.
pub(crate) async fn resume_with_room(
    room_id: String,
) -> Result<(Arc<kupferdraht::Client>, Arc<Room>, String), KupferdrahtError> {
    let client = resume_client().await?;
    client.sync_once(SyncSettings::default()).await?;
    let room = client
        .room(&room_id)
        .await
        .ok_or_else(|| KupferdrahtError::InvalidInput(format!("room {room_id} not in cache")))?;
    Ok((client, room, room_id))
}

/// Variant of [`resume_with_room`] for message-scoped CLI commands.
/// When the caller did not pass `--id <room>` explicitly, the room is
/// resolved from the message id via [`timeline::resolve_room_for_message`].
pub(crate) async fn resume_with_room_for_message(
    explicit_room_id: Option<String>,
    message_id: &str,
) -> Result<(Arc<kupferdraht::Client>, Arc<Room>, String), KupferdrahtError> {
    let client = resume_client().await?;
    client.sync_once(SyncSettings::default()).await?;
    let room_id = timeline::resolve_room_for_message(&client, explicit_room_id, message_id).await?;
    let room = client
        .room(&room_id)
        .await
        .ok_or_else(|| KupferdrahtError::InvalidInput(format!("room {room_id} not in cache")))?;
    Ok((client, room, room_id))
}

static LOCAL_OFFSET: std::sync::OnceLock<time::UtcOffset> = std::sync::OnceLock::new();

/// Cache the process-local UTC offset — MUST run before the tokio
/// runtime spawns worker threads (`time` refuses the Unix env lookup
/// in a multi-threaded process). Falls back to UTC when the offset
/// cannot be determined.
pub(crate) fn init_local_offset() {
    let offset = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);
    let _ = LOCAL_OFFSET.set(offset);
}

fn local_offset() -> time::UtcOffset {
    LOCAL_OFFSET.get().copied().unwrap_or(time::UtcOffset::UTC)
}

/// Render a Unix-ms timestamp as RFC 3339 in the process-local offset,
/// truncated to seconds — every human-facing CLI timestamp goes
/// through here. Falls back to the raw number on a (theoretical)
/// out-of-range value.
pub(crate) fn fmt_ms(ms: i64) -> String {
    let Ok(t) = time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000) else {
        return ms.to_string();
    };
    let t = t.to_offset(local_offset());
    let Ok(t) = t.replace_nanosecond(0) else {
        return ms.to_string();
    };
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| ms.to_string())
}

/// Parse a human `--due` value into Unix ms. Accepted forms:
/// RFC 3339 (explicit offset or `Z`), `yyyy-mm-ddThh:mm[:ss]` /
/// `yyyy-mm-dd hh:mm[:ss]` (interpreted in the process-local offset),
/// and bare `yyyy-mm-dd` — due by *end* of that local day (23:59:59).
pub(crate) fn parse_due(raw: &str) -> Result<i64, KupferdrahtError> {
    use time::macros::format_description;
    let s = raw.trim();
    assert!(!s.is_empty() || raw.is_empty(), "trim never grows input");
    if let Ok(t) = time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339) {
        return unix_ms(t);
    }
    let datetime_formats = [
        format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]"),
        format_description!("[year]-[month]-[day]T[hour]:[minute]"),
        format_description!("[year]-[month]-[day] [hour]:[minute]:[second]"),
        format_description!("[year]-[month]-[day] [hour]:[minute]"),
    ];
    for format in datetime_formats {
        if let Ok(t) = time::PrimitiveDateTime::parse(s, format) {
            return unix_ms(t.assume_offset(local_offset()));
        }
    }
    if let Ok(d) = time::Date::parse(s, format_description!("[year]-[month]-[day]")) {
        let end_of_day = d
            .with_hms(23, 59, 59)
            .map_err(|e| KupferdrahtError::InvalidInput(format!("--due: {e}")))?;
        return unix_ms(end_of_day.assume_offset(local_offset()));
    }
    Err(KupferdrahtError::InvalidInput(format!(
        "--due: cannot parse {raw:?} — use ISO 8601: yyyy-mm-dd, \
         yyyy-mm-ddThh:mm[:ss], or RFC 3339 with offset/Z"
    )))
}

/// Resolve the two due-date CLI inputs (`--due` ISO / `--due-ms` raw)
/// into the library's Unix-ms form. clap guarantees mutual exclusion.
pub(crate) fn resolve_due(
    due: Option<String>,
    due_ms: Option<i64>,
) -> Result<Option<i64>, KupferdrahtError> {
    match due {
        Some(s) => parse_due(&s).map(Some),
        None => Ok(due_ms),
    }
}

fn unix_ms(t: time::OffsetDateTime) -> Result<i64, KupferdrahtError> {
    i64::try_from(t.unix_timestamp_nanos() / 1_000_000)
        .map_err(|_| KupferdrahtError::InvalidInput("--due: timestamp out of range".into()))
}

/// Process exit code for a failed command: `AccountLocked` (Step 85) —
/// another Kupferdraht process is inside the account's critical section for a
/// few milliseconds — is retryable and gets its own code (3);
/// `KeyUnavailable` (Step 96, ADR-0029) — a database exists but the
/// keystore returns no key for it — gets 4: retry after unlocking the
/// keystore, or recover by hand. Everything else is 1.
pub(crate) fn exit_code_for(err: &KupferdrahtError) -> u8 {
    match err {
        KupferdrahtError::AccountLocked(_) => 3,
        KupferdrahtError::KeyUnavailable(_) => 4,
        _ => 1,
    }
}

pub(crate) fn parse_csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

pub(crate) fn prefix(s: &str, n: usize) -> String {
    let take = s.chars().take(n).collect::<String>();
    if s.chars().count() > n {
        format!("{take}…")
    } else {
        take
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    // LOCAL_OFFSET stays unset in tests, so `local_offset()` falls
    // back to UTC — naive inputs are deterministic here.

    fn ms(t: time::OffsetDateTime) -> i64 {
        i64::try_from(t.unix_timestamp_nanos() / 1_000_000).expect("in range")
    }

    #[test]
    fn parse_due_accepts_rfc3339_with_zone() {
        assert_eq!(
            parse_due("2026-07-20T15:00:00Z").expect("zulu"),
            ms(datetime!(2026-07-20 15:00 UTC))
        );
        assert_eq!(
            parse_due("2026-07-20T15:00:00+02:00").expect("offset"),
            ms(datetime!(2026-07-20 13:00 UTC))
        );
    }

    #[test]
    fn parse_due_naive_datetime_uses_process_offset() {
        for input in [
            "2026-07-20T15:00",
            "2026-07-20T15:00:00",
            "2026-07-20 15:00",
            "2026-07-20 15:00:00",
        ] {
            assert_eq!(
                parse_due(input).expect(input),
                ms(datetime!(2026-07-20 15:00 UTC)),
                "{input}"
            );
        }
    }

    #[test]
    fn parse_due_bare_date_means_end_of_day() {
        assert_eq!(
            parse_due("2026-07-20").expect("date"),
            ms(datetime!(2026-07-20 23:59:59 UTC))
        );
    }

    #[test]
    fn parse_due_rejects_garbage() {
        for input in ["soon", "20.07.2026", "2026-13-01", "1753000000000"] {
            assert!(parse_due(input).is_err(), "{input} must not parse");
        }
    }

    #[test]
    fn account_locked_exits_3_key_unavailable_4_everything_else_1() {
        assert_eq!(
            exit_code_for(&KupferdrahtError::AccountLocked(
                "account alice (pid 1)".into()
            )),
            3
        );
        assert_eq!(
            exit_code_for(&KupferdrahtError::KeyUnavailable("kupferdraht.db".into())),
            4
        );
        assert_eq!(exit_code_for(&KupferdrahtError::Storage("x".into())), 1);
        assert_eq!(exit_code_for(&KupferdrahtError::SessionExpired), 1);
    }

    #[test]
    fn fmt_ms_renders_rfc3339_seconds() {
        assert_eq!(fmt_ms(0), "1970-01-01T00:00:00Z");
        let t = ms(datetime!(2026-07-20 13:00 UTC));
        assert_eq!(fmt_ms(t), "2026-07-20T13:00:00Z");
        // sub-second precision is truncated for display
        assert_eq!(fmt_ms(t + 828), "2026-07-20T13:00:00Z");
    }

    #[test]
    fn fmt_ms_round_trips_through_parse_due() {
        let t = ms(datetime!(2026-07-20 13:00 UTC));
        assert_eq!(parse_due(&fmt_ms(t)).expect("round trip"), t);
    }
}
