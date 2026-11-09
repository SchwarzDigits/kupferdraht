// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Wire's `UTCTimeMillis` (Step 151).

/// `time` of an event or an answer as milliseconds since the UNIX epoch;
/// `None` for anything else, which a caller answers with this device's clock.
pub(crate) fn parse_time_ms(time: &str) -> Option<i64> {
    let parsed =
        time::OffsetDateTime::parse(time, &time::format_description::well_known::Rfc3339).ok()?;
    i64::try_from(parsed.unix_timestamp_nanos() / 1_000_000).ok()
}

#[cfg(test)]
mod tests {
    use super::parse_time_ms;

    #[test]
    fn wires_time_reads_to_the_millisecond() {
        assert_eq!(
            parse_time_ms("2026-09-28T10:00:01.000Z"),
            Some(1_790_589_601_000)
        );
        assert_eq!(
            parse_time_ms("2026-09-28T10:00:01.042Z"),
            Some(1_790_589_601_042)
        );
        assert_eq!(
            parse_time_ms("2026-09-28T10:00:01Z"),
            Some(1_790_589_601_000)
        );
        assert_eq!(
            parse_time_ms("2026-09-28T12:00:01.000+02:00"),
            Some(1_790_589_601_000)
        );
    }

    #[test]
    fn anything_else_is_none() {
        for time in ["", "yesterday-ish", "2026-09-28", "1790589601000"] {
            assert_eq!(parse_time_ms(time), None, "{time}");
        }
    }
}
