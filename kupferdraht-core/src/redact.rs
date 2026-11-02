// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Redacted `Debug` output for secret fields (Step 101, review finding
//! H6). Types that carry a token, a password, a join or activation code
//! or key bytes implement `Debug` by hand and print these markers in the
//! secret fields — one `?value` log field or `{:?}` in an error text must
//! never put a secret into a log. [`bounded_detail`] bounds what an error
//! carries of a backend's body (Step 137).

use std::fmt;

/// Prints `<redacted>` in place of a secret.
pub(crate) struct Hidden;

impl fmt::Debug for Hidden {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Prints `Some(<redacted>)` or `None` for an optional secret.
pub(crate) struct HiddenOption(bool);

impl fmt::Debug for HiddenOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0 { "Some(<redacted>)" } else { "None" })
    }
}

/// An optional secret, redacted.
pub(crate) fn hidden_option<T>(value: Option<&T>) -> HiddenOption
where
    T: ?Sized,
{
    HiddenOption(value.is_some())
}

/// Prints `<n bytes>` in place of key material.
pub(crate) struct ByteCount(usize);

impl fmt::Debug for ByteCount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{} bytes>", self.0)
    }
}

/// Key bytes, redacted to their length.
pub(crate) fn byte_count(value: &[u8]) -> ByteCount {
    ByteCount(value.len())
}

/// How much of a backend's error body an error carries (Step 137).
pub(crate) const DETAIL_CAP: usize = 1024;

/// The part of a backend's error body an error may carry: at most
/// [`DETAIL_CAP`] bytes of it, cut on a character boundary and marked
/// `[…]` when cut, with control characters dropped (a line break or a
/// tab becomes a space). The text crosses `UniFFI` and wasm into host
/// logs, where the backend must write neither escape sequences nor a
/// megabyte.
pub(crate) fn bounded_detail(body: &str) -> String {
    let mut out = String::with_capacity(body.len().min(DETAIL_CAP));
    for c in body.chars() {
        let c = match c {
            c if c.is_control() && c.is_whitespace() => ' ',
            c if c.is_control() => continue,
            c => c,
        };
        if out.len() + c.len_utf8() > DETAIL_CAP {
            out.push_str(" […]");
            break;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{DETAIL_CAP, bounded_detail};

    #[test]
    fn a_short_body_passes_unchanged() {
        assert_eq!(bounded_detail(r#"{"label":"x"}"#), r#"{"label":"x"}"#);
    }

    #[test]
    fn control_characters_are_dropped_and_line_breaks_become_spaces() {
        assert_eq!(bounded_detail("a\u{1b}[31mb\r\nc\td"), "a[31mb  c d");
    }

    #[test]
    fn a_long_body_is_cut_on_a_character_boundary_and_marked() {
        let body = "ä".repeat(DETAIL_CAP);
        let out = bounded_detail(&body);
        assert!(out.ends_with(" […]"), "{out}");
        assert!(
            out.len() <= DETAIL_CAP + " […]".len(),
            "{} bytes",
            out.len()
        );
        assert!(out.trim_end_matches(" […]").chars().all(|c| c == 'ä'));
    }
}
