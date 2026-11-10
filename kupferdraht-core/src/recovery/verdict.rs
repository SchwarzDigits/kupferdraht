// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 147: what recovery data is for this login, decided before a PIN.

use super::data::invalid;
use crate::error::KupferdrahtError;

/// The verdict on recovery data before the PIN service is asked: `Ok`, or
/// the reason `recover` would refuse it with (`InvalidRecoveryData`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecoveryDataVerdict {
    /// The account's data for the device its storage slot names.
    Ok,
    /// Bytes that are not recovery data.
    NotRecoveryData,
    /// Another format version, or an Argon2id cost a browser cannot give.
    Unsupported,
    /// Another account's data.
    OtherAccount,
    /// Data of a device the storage slot no longer names.
    OldDevice,
}

impl RecoveryDataVerdict {
    /// The browser's name of the verdict: `recover`'s reason for it.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::NotRecoveryData => "notRecoveryData",
            Self::Unsupported => "unsupported",
            Self::OtherAccount => "otherAccount",
            Self::OldDevice => "oldDevice",
        }
    }

    /// `recover`'s error for a verdict other than `Ok`.
    pub(crate) fn refusal(self) -> KupferdrahtError {
        debug_assert_ne!(self, Self::Ok, "an accepted file is no refusal");
        invalid(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::RecoveryDataVerdict;

    #[test]
    fn the_names_are_the_reasons_recover_refuses_with() {
        for (verdict, name) in [
            (RecoveryDataVerdict::Ok, "ok"),
            (RecoveryDataVerdict::NotRecoveryData, "notRecoveryData"),
            (RecoveryDataVerdict::Unsupported, "unsupported"),
            (RecoveryDataVerdict::OtherAccount, "otherAccount"),
            (RecoveryDataVerdict::OldDevice, "oldDevice"),
        ] {
            assert_eq!(verdict.as_str(), name);
        }
    }
}
