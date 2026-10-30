// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Redacted `Debug` output for secret fields (Step 101, review finding
//! H6). Types that carry a token, a password, a join or activation code
//! or key bytes implement `Debug` by hand and print these markers in the
//! secret fields — one `?value` log field or `{:?}` in an error text must
//! never put a secret into a log.

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
