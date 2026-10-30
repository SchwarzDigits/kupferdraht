// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The PIN (Step 112, ADR-0035): Unicode NFC before anything else, so the
//! same PIN typed on another keyboard is the same bytes; at least six
//! characters — characters, not bytes; digits only is fine, longer PINs of
//! any characters too; a short list of trivial PINs is refused.

use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

/// The fewest characters a new PIN has, counted after NFC.
pub(crate) const MIN_PIN_CHARS: usize = 6;

/// Whether a new PIN may be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PinCheck {
    Ok,
    TooShort,
    TooCommon,
}

impl PinCheck {
    /// The result as the browser API spells it.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::TooShort => "tooShort",
            Self::TooCommon => "tooCommon",
        }
    }
}

/// PINs too common to protect anything: a guesser tries them first.
const TOO_COMMON: &[&str] = &[
    "123456", "654321", "123123", "121212", "112233", "123321", "012345", "234567", "345678",
    "456789", "987654", "876543", "765432", "567890", "098765", "111222", "102030", "abcdef",
    "qwerty", "qwertz", "password", "passwort",
];

/// The PIN in Unicode NFC — the form every step of the scheme uses.
pub(crate) fn normalize_pin(pin: &str) -> Zeroizing<String> {
    Zeroizing::new(pin.nfc().collect())
}

/// Whether `pin` may become a new PIN: at least [`MIN_PIN_CHARS`]
/// characters after NFC; not one character repeated, not on the list of
/// trivial PINs.
pub(crate) fn validate_new_pin(pin: &str) -> PinCheck {
    let pin = normalize_pin(pin);
    let mut chars = pin.chars();
    if pin.chars().count() < MIN_PIN_CHARS {
        return PinCheck::TooShort;
    }
    let first = chars.next();
    let repeated = chars.all(|c| Some(c) == first);
    let lower = Zeroizing::new(pin.to_lowercase());
    if repeated || TOO_COMMON.contains(&lower.as_str()) {
        return PinCheck::TooCommon;
    }
    PinCheck::Ok
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "ü" composed or as "u" plus a combining diaeresis: the same PIN.
    #[test]
    fn composed_and_decomposed_are_the_same_pin() {
        let composed = normalize_pin("Gr\u{fc}\u{df}e 2026");
        let decomposed = normalize_pin("Gru\u{308}\u{df}e 2026");
        assert_eq!(*composed, *decomposed);
        assert_eq!(composed.as_bytes(), "Gr\u{fc}\u{df}e 2026".as_bytes());
    }

    #[test]
    fn six_characters_count_not_six_bytes() {
        assert_eq!(
            validate_new_pin("\u{e4}\u{f6}\u{fc}\u{e4}\u{f6}\u{fc}"),
            PinCheck::Ok
        );
        assert_eq!(
            validate_new_pin("\u{e4}\u{e4}\u{e4}\u{e4}\u{e4}"),
            PinCheck::TooShort
        );
        assert_eq!(validate_new_pin("12345"), PinCheck::TooShort);
        assert_eq!(validate_new_pin(""), PinCheck::TooShort);
    }

    /// Five characters as typed with combining marks are still five after
    /// NFC: the rule counts the normalized PIN.
    #[test]
    fn the_count_is_taken_after_normalization() {
        assert_eq!(
            validate_new_pin("u\u{308}u\u{308}u\u{308}u\u{308}u\u{308}"),
            PinCheck::TooShort
        );
    }

    #[test]
    fn trivial_pins_are_too_common() {
        for pin in ["123456", "000000", "111111", "654321", "123123"] {
            assert_eq!(validate_new_pin(pin), PinCheck::TooCommon, "{pin}");
        }
    }

    #[test]
    fn digits_and_longer_pins_are_fine() {
        for pin in [
            "482913",
            "Gr\u{fc}\u{df}e 2026",
            "correct horse battery staple",
        ] {
            assert_eq!(validate_new_pin(pin), PinCheck::Ok, "{pin}");
        }
    }

    #[test]
    fn the_results_are_spelled_for_the_browser() {
        assert_eq!(PinCheck::Ok.as_str(), "ok");
        assert_eq!(PinCheck::TooShort.as_str(), "tooShort");
        assert_eq!(PinCheck::TooCommon.as_str(), "tooCommon");
    }
}
