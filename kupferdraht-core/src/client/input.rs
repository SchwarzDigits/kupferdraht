// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 95c — validation of consumer arguments at the public entry
//! points. An argument the app passes is runtime data (CLAUDE.md
//! "Assertions"): an empty required value is `InvalidInput`, never a
//! `debug_assert!` further down (over FFI a panic in the app's process).

use crate::error::KupferdrahtError;

/// `Ok` when every `(name, value)` in `fields` is non-empty.
///
/// # Errors
/// `KupferdrahtError::InvalidInput` naming `op` and the first empty field.
pub(crate) fn require_non_empty(op: &str, fields: &[(&str, &str)]) -> Result<(), KupferdrahtError> {
    match fields.iter().find(|(_, value)| value.is_empty()) {
        Some((name, _)) => Err(KupferdrahtError::InvalidInput(format!(
            "{op}: {name} must be non-empty"
        ))),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::require_non_empty;
    use crate::error::KupferdrahtError;

    #[test]
    fn names_the_first_empty_field() {
        let err = require_non_empty("login", &[("email", "a@b"), ("password", "")])
            .expect_err("empty password");
        assert!(
            matches!(&err, KupferdrahtError::InvalidInput(msg) if msg == "login: password must be non-empty"),
            "{err:?}"
        );
        assert!(require_non_empty("login", &[("email", "a@b"), ("password", "pw")]).is_ok());
    }
}
